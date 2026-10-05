//! In-process contract server. It proves adapter choreography only, not PSC
//! authorization, real database transactions, encryption or browser commits.
use super::*;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
};
const ID: &str = "0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210";
fn pos(n: u64) -> Position {
    Position::new(n).unwrap()
}
fn event(id: &str) -> SealedEvent {
    let bytes = b"opaque fixture, not encryption proof";
    SealedEvent {
        event_id: id.into(),
        hub_id: "hub".into(),
        project_id: "canonical-project".into(),
        session_id: "opaque-session".into(),
        key_epoch: pos(1),
        ciphertext: URL_SAFE_NO_PAD.encode(bytes),
        digest: wire::digest(bytes),
        attachment_ids: vec![],
    }
}
fn reply(status: u16, body: Value) -> Response {
    Response {
        status,
        body: serde_json::to_vec(&body).unwrap(),
    }
}
fn stored(e: &SealedEvent, seq: usize, outcome: &str) -> Value {
    json!({"event_id":e.event_id,"outcome":outcome,"sequence":seq.to_string(),"digest":e.digest,"key_epoch":e.key_epoch,"stored_at":"2026-10-05T15:00:00Z","expires_at":"2027-01-03T15:00:00Z"})
}
fn row(id: &str, seq: &str) -> Value {
    let mut v = serde_json::to_value(event(id)).unwrap();
    v["sequence"] = json!(seq);
    v["stored_at"] = json!("2026-10-05T15:00:00Z");
    v["expires_at"] = json!("2027-01-03T15:00:00Z");
    v
}
fn page(events: Vec<Value>, gaps: Vec<Value>, head: &str, next: &str) -> Value {
    json!({"wire_version":1,"feed_generation":"1","retained_floor":"1","head":head,"events":events,"expired_intervals":gaps,"next_cursor":next,"has_more":next!=head,"poll_after_ms":5000})
}
#[derive(Default)]
struct Server {
    feeds: HashMap<String, Vec<SealedEvent>>,
    acks: HashSet<(String, String, String)>,
    cursors: HashMap<(String, String), Cursor>,
    read: HashMap<(String, String, String, String), u64>,
    revoked: HashSet<(String, String)>,
    expired: HashSet<(String, String)>,
    lose_next_append: bool,
    requests: Vec<(Role, Method, String, Value)>,
}
#[derive(Clone)]
struct Fake {
    state: Arc<Mutex<Server>>,
    account: String,
    device: String,
    role: Role,
}
impl Fake {
    fn new(state: &Arc<Mutex<Server>>, account: &str, device: &str, role: Role) -> Self {
        Self {
            state: state.clone(),
            account: account.into(),
            device: device.into(),
            role,
        }
    }
}
impl AuthenticatedTransport for Fake {
    fn exchange(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send + '_>> {
        Box::pin(async move {
            assert_eq!(
                request.body_digest,
                URL_SAFE_NO_PAD.encode(Sha256::digest(&request.body))
            );
            let mut s = self.state.lock().unwrap();
            if self.role != request.role {
                return Ok(reply(403, json!({"error":"capability_required"})));
            }
            if s.revoked
                .contains(&(self.account.clone(), self.device.clone()))
            {
                return Ok(reply(401, json!({"error":"grant_revoked"})));
            }
            let body: Value = if request.body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&request.body).unwrap()
            };
            s.requests.push((
                request.role,
                request.method,
                request.path.clone(),
                body.clone(),
            ));
            let result = match (request.method, request.path.as_str()) {
                (Method::Post, "/api/operator/feed/events") => {
                    let events: Vec<SealedEvent> =
                        serde_json::from_value(body["events"].clone()).unwrap();
                    let mut rows = vec![];
                    for e in events {
                        if e.hub_id != "hub" || e.project_id != "canonical-project" {
                            rows.push(json!({"event_id":e.event_id,"outcome":"rejected","error":"project_not_granted"}));
                            continue;
                        }
                        let expired = s
                            .expired
                            .contains(&(self.account.clone(), e.event_id.clone()));
                        let feed = s.feeds.entry(self.account.clone()).or_default();
                        if let Some(i) = feed.iter().position(|old| old.event_id == e.event_id) {
                            if feed[i] != e {
                                rows.push(json!({"event_id":e.event_id,"outcome":"rejected","error":"event_conflict"}));
                            } else if expired {
                                rows.push(json!({"event_id":e.event_id,"outcome":"expired","sequence":(i+1).to_string(),"digest":e.digest,"expired_at":"2027-01-03T15:00:00Z"}));
                            } else {
                                rows.push(stored(&e, i + 1, "duplicate"));
                            }
                        } else {
                            rows.push(stored(&e, feed.len() + 1, "stored"));
                            feed.push(e);
                        }
                    }
                    reply(
                        200,
                        json!({"wire_version":1,"feed_generation":"1","rows":rows}),
                    )
                }
                (Method::Get, path) if path.starts_with("/api/operator/feed?") => {
                    let q: HashMap<_, _> = path
                        .split_once('?')
                        .unwrap()
                        .1
                        .split('&')
                        .map(|p| p.split_once('=').unwrap())
                        .collect();
                    let after: usize = q["after"].parse().unwrap();
                    let limit: usize = q["limit"].parse().unwrap();
                    let feed = s.feeds.get(&self.account).cloned().unwrap_or_default();
                    let end = (after + limit).min(feed.len());
                    let rows = feed
                        .iter()
                        .enumerate()
                        .take(end)
                        .skip(after)
                        .map(|(i, e)| row(&e.event_id, &(i + 1).to_string()))
                        .collect();
                    reply(
                        200,
                        page(rows, vec![], &feed.len().to_string(), &end.to_string()),
                    )
                }
                (Method::Post, "/api/operator/feed/acks") => {
                    let acks: Vec<PersistedEvent> =
                        serde_json::from_value(body["acks"].clone()).unwrap();
                    let mut rows = vec![];
                    for ack in acks {
                        let found = s
                            .feeds
                            .get(&self.account)
                            .and_then(|f| f.iter().find(|e| e.event_id == ack.event_id));
                        let error = match found {
                            None => Some("event_unknown"),
                            Some(e) if e.digest != ack.digest => Some("digest_mismatch"),
                            _ => None,
                        };
                        if let Some(error) = error {
                            rows.push(
                                json!({"event_id":ack.event_id,"outcome":"rejected","error":error}),
                            );
                        } else {
                            let new = s.acks.insert((
                                self.account.clone(),
                                self.device.clone(),
                                ack.event_id.clone(),
                            ));
                            rows.push(json!({"event_id":ack.event_id,"outcome":if new {"acked"}else{"already_acked"}}));
                        }
                    }
                    reply(200, json!({"wire_version":1,"rows":rows}))
                }
                (Method::Get, "/api/operator/devices/me/cursor") => {
                    let cursor = s
                        .cursors
                        .get(&(self.account.clone(), self.device.clone()))
                        .cloned()
                        .unwrap_or(Cursor {
                            wire_version: 1,
                            feed_generation: pos(1),
                            cursor: None,
                            accepted_expired_through: pos(0),
                            updated_at: None,
                        });
                    reply(200, serde_json::to_value(cursor).unwrap())
                }
                (Method::Put, "/api/operator/devices/me/cursor") => {
                    let cursor: Cursor = serde_json::from_value(body).unwrap();
                    if s.cursors
                        .get(&(self.account.clone(), self.device.clone()))
                        .is_some_and(|old| cursor.cursor < old.cursor)
                    {
                        reply(409, json!({"error":"cursor_regression"}))
                    } else {
                        s.cursors
                            .insert((self.account.clone(), self.device.clone()), cursor.clone());
                        reply(200, serde_json::to_value(cursor).unwrap())
                    }
                }
                (Method::Put, "/api/operator/read-marks") => {
                    let mut mark: ReadMark = serde_json::from_value(body).unwrap();
                    let max = s
                        .feeds
                        .get(&self.account)
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .filter(|(_, e)| {
                            e.hub_id == mark.hub_id
                                && e.project_id == mark.project_id
                                && e.session_id == mark.session_id
                        })
                        .map(|(i, _)| i as u64 + 1)
                        .max()
                        .unwrap_or(0);
                    if mark.sequence.value() > max {
                        reply(422, json!({"error":"read_mark_out_of_range"}))
                    } else {
                        let value = s
                            .read
                            .entry((
                                self.account.clone(),
                                mark.hub_id.clone(),
                                mark.project_id.clone(),
                                mark.session_id.clone(),
                            ))
                            .or_default();
                        *value = (*value).max(mark.sequence.value());
                        mark.sequence = pos(*value);
                        reply(200, serde_json::to_value(mark).unwrap())
                    }
                }
                _ => reply(403, json!({"error":"capability_required"})),
            };
            if request.path == "/api/operator/feed/events" && s.lose_next_append {
                s.lose_next_append = false;
                return Err(Failure::Unavailable);
            }
            Ok(result)
        })
    }
}
struct Scripted(Mutex<VecDeque<Response>>);
impl Scripted {
    fn new(status: u16, value: Value) -> Self {
        Self(Mutex::new(VecDeque::from([reply(status, value)])))
    }
}
impl AuthenticatedTransport for Scripted {
    fn exchange(&self, _: Request) -> Pin<Box<dyn Future<Output = Result<Response>> + Send + '_>> {
        Box::pin(async { Ok(self.0.lock().unwrap().pop_front().unwrap()) })
    }
}
#[test]
fn decimal_strings_preserve_bigints_and_reject_alternate_forms() {
    let raw = "\"9007199254740993\"";
    let p: Position = serde_json::from_str(raw).unwrap();
    assert_eq!(p.value(), 9_007_199_254_740_993);
    assert_eq!(serde_json::to_string(&p).unwrap(), raw);
    for raw in [
        "1",
        "\"01\"",
        "\"-1\"",
        "\"1.0\"",
        "\"1e3\"",
        "\"10000000000000000000\"",
    ] {
        assert!(
            serde_json::from_str::<Position>(raw).is_err(),
            "accepted {raw}"
        );
    }
}
#[tokio::test]
async fn lost_receipt_duplicate_conflict_and_expired_retry_preserve_identity() {
    let state = Arc::new(Mutex::new(Server {
        lose_next_append: true,
        ..Server::default()
    }));
    let machine = MachineRelay::new(Fake::new(&state, "a", "m", Role::Machine));
    assert_eq!(
        machine.append(pos(1), &[event(ID)]).await.unwrap_err(),
        Failure::Unavailable
    );
    assert!(
        matches!(machine.append(pos(1),&[event(ID)]).await.unwrap().rows[0].outcome,AppendOutcome::Duplicate{sequence,..}if sequence==pos(1))
    );
    let mut changed = event(ID);
    changed.session_id = "other-session".into();
    assert!(matches!(
        machine.append(pos(1), &[changed]).await.unwrap().rows[0].outcome,
        AppendOutcome::Rejected { .. }
    ));
    state
        .lock()
        .unwrap()
        .expired
        .insert(("a".into(), ID.into()));
    assert!(matches!(
        machine.append(pos(1), &[event(ID)]).await.unwrap().rows[0].outcome,
        AppendOutcome::Expired { .. }
    ));
    let s = state.lock().unwrap();
    assert_eq!(s.feeds["a"].len(), 1);
    assert_eq!(s.requests[0].3, s.requests[1].3);
}
#[tokio::test]
async fn device_ack_cursor_and_shared_read_are_independent() {
    let state = Arc::new(Mutex::new(Server::default()));
    MachineRelay::new(Fake::new(&state, "a", "m", Role::Machine))
        .append(pos(1), &[event(ID)])
        .await
        .unwrap();
    let desktop = DeviceInbox::new(Fake::new(&state, "a", "desktop", Role::Device));
    let phone = DeviceInbox::new(Fake::new(&state, "a", "phone", Role::Device));
    let page = desktop.replay(pos(1), pos(0), 200).await.unwrap();
    let acks = [PersistedEvent {
        event_id: ID.into(),
        digest: page.events[0].event.digest.clone(),
    }];
    assert_eq!(
        desktop.ack_persisted(pos(1), &acks).await.unwrap().rows[0].outcome,
        AckOutcome::Acked
    );
    desktop
        .mark_read(&ReadMark {
            wire_version: 1,
            feed_generation: pos(1),
            hub_id: "hub".into(),
            project_id: "canonical-project".into(),
            session_id: "opaque-session".into(),
            sequence: pos(1),
            updated_at: None,
            updated_by_device_id: None,
        })
        .await
        .unwrap();
    assert_eq!(
        phone
            .replay(pos(1), pos(0), 200)
            .await
            .unwrap()
            .events
            .len(),
        1
    );
    assert_eq!(desktop.cursor(pos(1)).await.unwrap().cursor, None);
    assert_eq!(phone.cursor(pos(1)).await.unwrap().cursor, None);
    desktop
        .save_cursor(&Cursor {
            wire_version: 1,
            feed_generation: pos(1),
            cursor: Some(pos(1)),
            accepted_expired_through: pos(0),
            updated_at: None,
        })
        .await
        .unwrap();
    assert_eq!(desktop.cursor(pos(1)).await.unwrap().cursor, Some(pos(1)));
    assert_eq!(phone.cursor(pos(1)).await.unwrap().cursor, None);
    assert_eq!(
        phone.ack_persisted(pos(1), &acks).await.unwrap().rows[0].outcome,
        AckOutcome::Acked
    );
    assert_eq!(
        desktop.ack_persisted(pos(1), &acks).await.unwrap().rows[0].outcome,
        AckOutcome::AlreadyAcked
    );
}
#[tokio::test]
async fn authenticated_scope_and_current_revocation_choose_authority() {
    let state = Arc::new(Mutex::new(Server::default()));
    let machine = MachineRelay::new(Fake::new(&state, "a", "m", Role::Machine));
    machine.append(pos(1), &[event(ID)]).await.unwrap();
    let foreign = DeviceInbox::new(Fake::new(&state, "b", "phone", Role::Device));
    assert!(
        foreign
            .replay(pos(1), pos(0), 200)
            .await
            .unwrap()
            .events
            .is_empty()
    );
    assert!(matches!(
        foreign
            .ack_persisted(
                pos(1),
                &[PersistedEvent {
                    event_id: ID.into(),
                    digest: event(ID).digest
                }]
            )
            .await
            .unwrap()
            .rows[0]
            .outcome,
        AckOutcome::Rejected { .. }
    ));
    assert!(matches!(
        DeviceInbox::new(Fake::new(&state, "a", "m", Role::Machine))
            .replay(pos(1), pos(0), 200)
            .await,
        Err(Failure::Http { status: 403, .. })
    ));
    let mut wrong = event(OTHER);
    wrong.project_id = "not-granted".into();
    assert!(matches!(
        machine.append(pos(1), &[wrong]).await.unwrap().rows[0].outcome,
        AppendOutcome::Rejected { .. }
    ));
    state
        .lock()
        .unwrap()
        .revoked
        .insert(("b".into(), "phone".into()));
    assert!(
        matches!(foreign.cursor(pos(1)).await,Err(Failure::Http{status:401,code,..})if code=="grant_revoked")
    );
}
#[tokio::test]
async fn append_receipts_bind_identity_digest_epoch_and_closed_outcome() {
    for (id, digest, epoch, outcome) in [
        (OTHER, event(ID).digest, "1", "stored"),
        (ID, "sha256:wrong".into(), "1", "stored"),
        (ID, event(ID).digest, "2", "stored"),
        (ID, event(ID).digest, "1", "invented"),
    ] {
        let t = Scripted::new(
            200,
            json!({"wire_version":1,"feed_generation":"1","rows":[{"event_id":id,"outcome":outcome,"sequence":"1","digest":digest,"key_epoch":epoch,"stored_at":"date","expires_at":"later"}]}),
        );
        assert!(
            MachineRelay::new(t)
                .append(pos(1), &[event(ID)])
                .await
                .is_err()
        );
    }
    let mut row = stored(&event(ID), 1, "stored");
    row["future_warning"] = json!(true);
    assert!(
        MachineRelay::new(Scripted::new(
            200,
            json!({"wire_version":1,"feed_generation":"1","future":true,"rows":[row]})
        ))
        .append(pos(1), &[event(ID)])
        .await
        .is_ok()
    );
}
#[tokio::test]
async fn replay_requires_ordered_exact_nonoverlapping_coverage() {
    let gap = json!({"from":"2","to":"2","reason":"retention"});
    assert_eq!(
        DeviceInbox::new(Scripted::new(
            200,
            page(
                vec![row(ID, "1"), row(OTHER, "3")],
                vec![gap.clone()],
                "3",
                "3"
            )
        ))
        .replay(pos(1), pos(0), 200)
        .await
        .unwrap()
        .next_cursor,
        pos(3)
    );
    for bad in [
        page(vec![row(ID, "1"), row(OTHER, "3")], vec![], "3", "3"),
        page(
            vec![row(ID, "1")],
            vec![json!({"from":"1","to":"3","reason":"retention"})],
            "3",
            "3",
        ),
        page(
            vec![row(ID, "1"), row(ID, "3")],
            vec![gap.clone()],
            "3",
            "3",
        ),
        page(vec![], vec![], "3", "3"),
        page(vec![row(ID, "3"), row(OTHER, "1")], vec![gap], "3", "3"),
    ] {
        assert!(
            DeviceInbox::new(Scripted::new(200, bad))
                .replay(pos(1), pos(0), 200)
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn huge_gap_checks_intervals_without_sequence_expansion() {
    let max = "9007199254740993";
    let p = page(
        vec![],
        vec![json!({"from":"1","to":max,"reason":"account_deleted"})],
        max,
        max,
    );
    assert_eq!(
        DeviceInbox::new(Scripted::new(200, p))
            .replay(pos(1), pos(0), 200)
            .await
            .unwrap()
            .next_cursor,
        pos(9_007_199_254_740_993)
    );
}
#[tokio::test]
async fn expiry_and_generation_recovery_are_explicit_and_validated() {
    let d = DeviceInbox::new(Scripted::new(
        410,
        json!({"error":"history_expired","feed_generation":"1","retained_floor":"5","head":"9","expired_through":"4"}),
    ));
    assert!(
        matches!(d.replay(pos(1),pos(0),200).await,Err(Failure::Http{recovery:Some(Recovery::HistoryExpired{expired_through,..}),..})if expired_through==pos(4))
    );
    let d = DeviceInbox::new(Scripted::new(
        410,
        json!({"error":"history_expired","feed_generation":"1","retained_floor":"5","head":"9","expired_through":"8"}),
    ));
    assert!(matches!(
        d.replay(pos(1), pos(0), 200).await,
        Err(Failure::Http { recovery: None, .. })
    ));
    let d = DeviceInbox::new(Scripted::new(
        409,
        json!({"error":"feed_generation_changed","feed_generation":"2","start_sequence":"10"}),
    ));
    assert!(
        matches!(d.replay(pos(1),pos(0),200).await,Err(Failure::Http{recovery:Some(Recovery::GenerationChanged{generation,..}),..})if generation==pos(2))
    );
}
#[tokio::test]
async fn malformed_and_oversized_inputs_refuse_before_transport() {
    let state = Arc::new(Mutex::new(Server::default()));
    let m = MachineRelay::new(Fake::new(&state, "a", "m", Role::Machine));
    let mut bad = event(ID);
    bad.digest = wire::digest(b"wrong");
    assert!(m.append(pos(1), &[bad]).await.is_err());
    let mut large = event(ID);
    let bytes = vec![0; 65_537];
    large.ciphertext = URL_SAFE_NO_PAD.encode(&bytes);
    large.digest = wire::digest(&bytes);
    assert!(m.append(pos(1), &[large]).await.is_err());
    assert!(m.append(pos(1), &[]).await.is_err());
    let mut readable = event(ID);
    readable.session_id = "readable name with spaces".into();
    assert!(m.append(pos(1), &[readable]).await.is_err());
    assert!(state.lock().unwrap().requests.is_empty());
}
#[tokio::test]
async fn ack_receipt_cannot_substitute_another_event() {
    assert!(matches!(
        DeviceInbox::new(Scripted::new(
            200,
            json!({"wire_version":1,"rows":[{"event_id":OTHER,"outcome":"acked"}]})
        ))
        .ack_persisted(
            pos(1),
            &[PersistedEvent {
                event_id: ID.into(),
                digest: event(ID).digest
            }]
        )
        .await,
        Err(Failure::Protocol("ack receipt binding"))
    ));
}
#[tokio::test]
async fn custody_outage_is_retryable_but_unknown_forbidden_error_is_not() {
    let e = MachineRelay::new(Scripted::new(
        503,
        json!({"error":"uploads_frozen","frozen_reason":"key_custody_unavailable"}),
    ))
    .append(pos(1), &[event(ID)])
    .await
    .unwrap_err();
    assert!(e.retryable());
    assert!(matches!(e, Failure::Http { status: 503, .. }));
    assert!(
        !Failure::Http {
            status: 403,
            code: "new_refusal".into(),
            recovery: None
        }
        .retryable()
    );
}
struct Hanging;
impl AuthenticatedTransport for Hanging {
    fn exchange(&self, _: Request) -> Pin<Box<dyn Future<Output = Result<Response>> + Send + '_>> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test(start_paused = true)]
async fn whole_request_deadline_bounds_a_never_settling_transport() {
    assert!(matches!(
        DeviceInbox::new(Hanging).cursor(pos(1)).await,
        Err(Failure::Deadline)
    ));
}

#[tokio::test]
async fn cursor_receipt_cannot_reset_generation_or_regress_commit() {
    let committed = Cursor {
        wire_version: 1,
        feed_generation: pos(1),
        cursor: Some(pos(5)),
        accepted_expired_through: pos(2),
        updated_at: None,
    };
    for receipt in [
        json!({"wire_version":1,"feed_generation":"2","cursor":"5","accepted_expired_through":"2"}),
        json!({"wire_version":1,"feed_generation":"1","cursor":"4","accepted_expired_through":"2"}),
        json!({"wire_version":1,"feed_generation":"1","cursor":"5","accepted_expired_through":"1"}),
        json!({"wire_version":1,"feed_generation":"1","cursor":null,"accepted_expired_through":"2"}),
        json!({"wire_version":1,"feed_generation":"1","cursor":"5","accepted_expired_through":"6"}),
    ] {
        assert!(
            DeviceInbox::new(Scripted::new(200, receipt))
                .save_cursor(&committed)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn read_receipt_cannot_mark_another_conversation() {
    let mark = ReadMark {
        wire_version: 1,
        feed_generation: pos(1),
        hub_id: "hub".into(),
        project_id: "canonical-project".into(),
        session_id: "opaque-session".into(),
        sequence: pos(5),
        updated_at: None,
        updated_by_device_id: None,
    };
    for (session, sequence, generation) in [
        ("other-session", "5", "1"),
        ("opaque-session", "4", "1"),
        ("opaque-session", "5", "2"),
    ] {
        let response = json!({"wire_version":1,"feed_generation":generation,"hub_id":"hub","project_id":"canonical-project","session_id":session,"sequence":sequence});
        assert!(
            DeviceInbox::new(Scripted::new(200, response))
                .mark_read(&mark)
                .await
                .is_err()
        );
    }
}

struct Oversized;
impl AuthenticatedTransport for Oversized {
    fn exchange(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send + '_>> {
        Box::pin(async move {
            Ok(Response {
                status: 200,
                body: vec![b' '; request.max_response_bytes + 1],
            })
        })
    }
}
#[tokio::test]
async fn oversized_response_refuses_before_json_decoding() {
    assert!(matches!(
        DeviceInbox::new(Oversized).cursor(pos(1)).await,
        Err(Failure::Protocol("response size"))
    ));
}

#[tokio::test]
async fn replay_cannot_change_generation_or_silently_adopt_a_lower_head() {
    let mut changed = page(vec![row(ID, "1")], vec![], "1", "1");
    changed["feed_generation"] = json!("2");
    assert!(
        DeviceInbox::new(Scripted::new(200, changed))
            .replay(pos(1), pos(0), 200)
            .await
            .is_err()
    );
    assert!(
        DeviceInbox::new(Scripted::new(200, page(vec![], vec![], "1", "1")))
            .replay(pos(1), pos(2), 200)
            .await
            .is_err()
    );
    let error = DeviceInbox::new(Scripted::new(
        409,
        json!({"error":"cursor_ahead","head":"1"}),
    ))
    .replay(pos(1), pos(2), 200)
    .await;
    assert!(
        matches!(error, Err(Failure::Http { recovery: Some(Recovery::CursorAhead { head }), .. }) if head == pos(1))
    );
}

#[tokio::test]
async fn expired_persistence_ack_is_a_noop_not_retained_history() {
    let device = DeviceInbox::new(Scripted::new(
        200,
        json!({"wire_version":1,"rows":[{"event_id":ID,"outcome":"rejected","error":"event_expired"}]}),
    ));
    let receipt = device
        .ack_persisted(
            pos(1),
            &[PersistedEvent {
                event_id: ID.into(),
                digest: event(ID).digest,
            }],
        )
        .await
        .unwrap();
    assert!(receipt.rows[0].outcome.is_acknowledged());
    assert!(
        matches!(&receipt.rows[0].outcome, AckOutcome::Rejected { error } if error == "event_expired")
    );
    assert!(
        !AckOutcome::Rejected {
            error: "event_unknown".into()
        }
        .is_acknowledged()
    );
}
