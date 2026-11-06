//! The client against a mock Cloud implementing the agreed contract exactly.
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cas_store::{InterventionKind, NewQualityEvent, QualityPayload};
use wiremock::{Match, Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, SyncQueue};

const TEAM: &str = "team-ledger";

#[derive(Default)]
struct CloudState {
    /// Ingest order; idempotent on id, first write wins.
    rows: Vec<serde_json::Value>,
    posts: Vec<usize>,
    gets: usize,
    /// Status to answer instead of serving, consumed front first.
    forced: Vec<u16>,
    /// Any POST over this many events answers 413.
    max_batch: Option<usize>,
    reject_turns: bool,
    reject_estimates: bool,
    turn_error: Option<(u16, serde_json::Value)>,
}

#[derive(Clone, Default)]
struct MockCloud(Arc<Mutex<CloudState>>);

impl Respond for MockCloud {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        if !state.forced.is_empty() {
            return ResponseTemplate::new(state.forced.remove(0));
        }
        if request.method.as_str() == "GET" {
            state.gets += 1;
            let query: HashMap<_, _> = request.url.query_pairs().into_owned().collect();
            let cursor = query.get("cursor");
            let start: usize = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);
            let limit: usize = query
                .get("limit")
                .and_then(|l| l.parse().ok())
                .unwrap_or(500);
            let page: Vec<_> = state.rows.iter().skip(start).take(limit).cloned().collect();
            // Always resumable: the last row's seq, the request's cursor echoed
            // on an empty page, and null only with no cursor and no rows.
            let next = if page.is_empty() {
                cursor.cloned()
            } else {
                Some((start + page.len()).to_string())
            };
            return ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "events": page,
                "next_cursor": next,
            }));
        }
        let mut body = String::new();
        flate2::read::GzDecoder::new(request.body.as_slice())
            .read_to_string(&mut body)
            .unwrap();
        let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(
            envelope["project_canonical_id"].is_string(),
            "a bad envelope is a 400 for the whole batch"
        );
        let events = envelope["events"].as_array().unwrap();
        assert!(events.len() <= 200, "over 200 events is a 400");
        if state.max_batch.is_some_and(|max| events.len() > max) {
            return ResponseTemplate::new(413);
        }
        state.posts.push(events.len());
        if state.reject_estimates && events.iter().any(|event| event["kind"]=="estimate" || event["kind"]=="estimate_outcome") {
            return ResponseTemplate::new(422).set_body_json(serde_json::json!({"error":"unsupported event kind: estimate"}));
        }
        if events.iter().any(|event| event["kind"] == "turn") {
            if let Some((status, body)) = &state.turn_error {
                return ResponseTemplate::new(*status).set_body_json(body);
            }
        }
        let (mut accepted, mut duplicates) = (0, 0);
        let mut rejected = Vec::new();
        for event in events {
            assert!(
                event["project_id"].is_string(),
                "project_id is required on every event"
            );
            if state.reject_turns && event["kind"] == "turn" {
                rejected.push(serde_json::json!({"id": event["id"], "reason":"unknown kind"}));
                continue;
            }
            if state.rows.iter().any(|row| row["id"] == event["id"]) {
                duplicates += 1;
            } else {
                accepted += 1;
                let mut stored = event.clone();
                stored["received_at"] = serde_json::json!("2026-10-31T19:00:00Z");
                stored["reporter_user_id"] = serde_json::json!("user-from-token");
                state.rows.push(stored);
            }
        }
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accepted": accepted, "duplicates": duplicates, "rejected": rejected,
        }))
    }
}

struct TeamRoute;

impl Match for TeamRoute {
    fn matches(&self, request: &Request) -> bool {
        request.url.path() == format!("/api/teams/{TEAM}/quality-events")
    }
}

fn machine(root: &Path, endpoint: &str, project: &str) -> CloudSyncer {
    crate::cloud::set_canonical_id_in_config_toml(root, project).unwrap();
    let queue = Arc::new(SyncQueue::open(root).unwrap());
    queue.init().unwrap();
    let config = CloudConfig {
        endpoint: endpoint.to_string(),
        token: Some("test-token".to_string()),
        team_id: Some(TEAM.to_string()),
        ..Default::default()
    };
    CloudSyncer::new_for_project(
        queue,
        config,
        CloudSyncerConfig::default(),
        project.to_string(),
        root,
    )
}

fn emit(root: &Path, key: &str) {
    let mut event = NewQualityEvent::new(
        key,
        QualityPayload::Intervention {
            intervention: InterventionKind::OverrideClose,
            minutes: None,
        },
    );
    event.scope = "scope".to_string();
    event.task_id = Some("cas-1".to_string());
    event.title = Some("Fix `rm -rf /secret-path` flow".to_string());
    event.local_note = Some("the secret reason".to_string());
    assert!(cas_store::record_quality_event(root, &event).unwrap());
}

/// Machine A's event reaches machine B in one batch, carrying scope A only;
/// a teammate's project arrives attributed; re-syncing moves nothing.
#[tokio::test(flavor = "multi_thread")]
async fn events_reach_a_second_machine_in_batches_under_scope_a() {
    let server = MockServer::start().await;
    let cloud = MockCloud::default();
    Mock::given(TeamRoute).respond_with(cloud.clone()).mount(&server).await;
    let (a_dir, b_dir, c_dir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (a_root, b_root, c_root) =
        (a_dir.path().to_path_buf(), b_dir.path().to_path_buf(), c_dir.path().to_path_buf());
    let endpoint = server.uri();

    let (a_summary, b_summary, resync, stored) = tokio::task::spawn_blocking({
        let cloud = cloud.clone();
        move || {
            let a = machine(&a_root, &endpoint, "acme/this");
            let b = machine(&b_root, &endpoint, "acme/this");
            let teammate = machine(&c_root, &endpoint, "acme/other");
            for key in ["one", "two", "three"] {
                emit(&a_root, key);
            }
            emit(&c_root, "from another project");

            let a_summary = a.sync_quality_events(true).unwrap();
            teammate.sync_quality_events(true).unwrap();
            let b_summary = b.sync_quality_events(true).unwrap();
            let resync = a.sync_quality_events(true).unwrap();

            let stored = cas_store::quality_events_between(&b_root, None, None).unwrap();
            assert_eq!(cloud.0.lock().unwrap().posts, [3, 1], "one request per batch");
            (a_summary, b_summary, resync, stored)
        }
    })
    .await
    .unwrap();

    assert_eq!(a_summary.pushed, 3);
    assert_eq!(b_summary.pulled, 4, "{stored:?}");
    assert_eq!(resync.pushed, 0, "the push cursor advanced");
    // A's own rows came back as its own project; the teammate's stay attributed.
    assert_eq!(resync.pulled, 1);
    assert_eq!(stored.iter().filter(|event| event.project_id == "acme/other").count(), 1);
    assert_eq!(stored.iter().filter(|event| event.project_id.is_empty()).count(), 3);
    assert!(stored.iter().all(|event| event.from_cloud && event.local_note.is_none()));

    let rows = serde_json::to_string(&cloud.0.lock().unwrap().rows).unwrap();
    assert!(!rows.contains("secret"), "scope A: {rows}");
    assert!(rows.contains("\"title\":\"Fix flow\""), "{rows}");
    assert!(rows.contains("\"project_id\":\"acme/this\""), "{rows}");
}

/// Until the route is live nothing is lost or noisy; 413 splits the batch.
#[tokio::test(flavor = "multi_thread")]
async fn route_not_live_holds_the_cursor_and_413_splits_the_batch() {
    let server = MockServer::start().await;
    let cloud = MockCloud::default();
    Mock::given(TeamRoute)
        .respond_with(cloud.clone())
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let endpoint = server.uri();

    let state = cloud.clone();
    tokio::task::spawn_blocking(move || {
        let syncer = machine(&root, &endpoint, "acme/this");
        for key in ["one", "two", "three"] {
            emit(&root, key);
        }
        state.0.lock().unwrap().forced = vec![404];
        assert!(syncer.sync_quality_events(true).unwrap().capability_absent);
        assert!(state.0.lock().unwrap().rows.is_empty());
        // The daemon's paced call does not ask again while the route is absent.
        state.0.lock().unwrap().forced = vec![500];
        assert!(syncer.sync_quality_events(false).unwrap().capability_absent);
        assert_eq!(state.0.lock().unwrap().forced, [500], "no request was made");

        // Live, with a size limit: the held events all arrive, split.
        {
            let mut cloud = state.0.lock().unwrap();
            cloud.forced.clear();
            cloud.max_batch = Some(2);
        }
        let summary = syncer.sync_quality_events(true).unwrap();
        assert!(!summary.capability_absent);
        assert_eq!(summary.pushed, 3);
        {
            let cloud = state.0.lock().unwrap();
            assert_eq!(
                cloud.posts,
                [1, 1, 1],
                "halved from 3 events to 1, then held there"
            );
            assert_eq!(cloud.rows.len(), 3);
        }
    })
    .await
    .unwrap();
}

/// A newer kind cannot poison the shared ledger; pending Turns can be replayed
/// when Cloud supports them, independently of normal rows and pull progress.
#[tokio::test(flavor = "multi_thread")]
async fn rejected_turns_do_not_block_other_quality_events_or_pull() {
    for status in [None, Some(400), Some(422)] {
        let server = MockServer::start().await;
        let cloud = MockCloud::default();
        Mock::given(TeamRoute)
            .respond_with(cloud.clone())
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let peer = remote.path().to_path_buf();
        let endpoint = server.uri();
        tokio::task::spawn_blocking(move || {
            let syncer = machine(&root, &endpoint, "acme/this");
            emit(&peer, "from teammate");
            let mut incoming = cas_store::quality_events_after(&peer, 0, 1).unwrap()[0].to_wire();
            incoming.project_id = Some("acme/other".into());
            {
                let mut state = cloud.0.lock().unwrap();
                state.rows.push(serde_json::to_value(incoming).unwrap());
                state.reject_turns = true;
                state.turn_error = status.map(|status| {
                    (
                        status,
                        serde_json::json!({
                            "error": "unsupported event kind: turn"
                        }),
                    )
                });
            }
            emit(&root, "before turn");
            // Friction now has its own rollout lane with estimate kinds.
            let friction = NewQualityEvent::new(
                "friction|task|cas-87a2",
                QualityPayload::Friction {
                    source: cas_store::FrictionSource::Task,
                    reference: "cas-87a2".into(),
                    severity: None,
                    project: None,
                },
            );
            cas_store::record_quality_event(&root, &friction).unwrap();
            let turn = NewQualityEvent::new(
                "native turn",
                QualityPayload::Turn {
                    woken_by: cas_store::WakeChannel::Director,
                    actions: vec![],
                    no_op: true,
                    input_tokens: Some(30),
                    output_tokens: Some(5),
                    minutes: Some(0.05),
                },
            );
            cas_store::record_quality_event(&root, &turn).unwrap();
            emit(&root, "after turn");

            let summary = syncer.sync_quality_events(true).unwrap();
            assert_eq!(
                summary.pulled, 1,
                "pull must run despite Turn rejection {status:?}"
            );
            assert!(
                !summary.capability_absent,
                "the ordinary route is available"
            );
            assert_eq!(summary.pushed, 3, "ordinary and Friction rows must push");
            assert_eq!(cloud.0.lock().unwrap().rows.len(), 4);
            assert!(
                cloud.0.lock().unwrap().rows.iter().any(|row| row["kind"] == "friction"),
                "Friction is synced independently of unsupported Turns"
            );
            let pending = cas_store::quality_events_after(&root, 0, 10).unwrap();
            assert_eq!(pending.len(), 4, "the local outbox loses no events");
            assert_eq!(
                pending
                    .iter()
                    .filter(|event| event.payload.kind() == "turn")
                    .count(),
                1
            );
            assert!(
                cas_store::quality_events_between(&root, None, None)
                    .unwrap()
                    .iter()
                    .any(|event| event.from_cloud && event.project_id == "acme/other")
            );

            emit(&root, "later intervention");
            // Deferring the unsupported kind must not activate the route's
            // six-hour cooldown. Advance only the ordinary 15-minute pacing.
            syncer
                .queue()
                .delete_metadata(super::LAST_ROUND_KEY)
                .unwrap();
            let later = syncer.sync_quality_events(false).unwrap();
            assert_eq!(
                later.pushed, 1,
                "later kinds progress while Turn is pending"
            );
            assert_eq!(cloud.0.lock().unwrap().gets, 2);
            {
                let mut state = cloud.0.lock().unwrap();
                state.reject_turns = false;
                state.turn_error = None;
            }
            let recovered = syncer.sync_quality_events(true).unwrap();
            assert_eq!(
                recovered.pushed, 1,
                "the held Turn retries after support arrives"
            );
            assert_eq!(cloud.0.lock().unwrap().rows.len(), 6);
            assert_eq!(syncer.sync_quality_events(true).unwrap().pushed, 0);
        })
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unrelated_turn_batch_errors_surface_after_pull() {
    for status in [400, 422] {
        let server = MockServer::start().await;
        let cloud = MockCloud::default();
        Mock::given(TeamRoute)
            .respond_with(cloud.clone())
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let endpoint = server.uri();
        tokio::task::spawn_blocking(move || {
            let syncer = machine(&root, &endpoint, "acme/this");
            let turn = NewQualityEvent::new(
                "native turn",
                QualityPayload::Turn {
                    woken_by: cas_store::WakeChannel::Director,
                    actions: vec![],
                    no_op: true,
                    input_tokens: None,
                    output_tokens: None,
                    minutes: None,
                },
            );
            cas_store::record_quality_event(&root, &turn).unwrap();
            let message = if status == 400 {
                "invalid project_canonical_id"
            } else {
                "unsupported project for event kind turn"
            };
            cloud.0.lock().unwrap().turn_error = Some((
                status,
                serde_json::json!({
                    "error": message
                }),
            ));
            let error = syncer
                .sync_quality_events(true)
                .expect_err("validation is not missing capability");
            assert!(error.to_string().contains(&status.to_string()), "{error}");
            assert!(error.to_string().contains(message), "{error}");
            assert_eq!(
                cloud.0.lock().unwrap().gets,
                1,
                "pull runs even when push errors"
            );
            assert_eq!(
                cas_store::quality_events_after(&root, 0, 10).unwrap().len(),
                1
            );
        })
        .await
        .unwrap();
    }
}

#[tokio::test(flavor="multi_thread")]
async fn unsupported_estimates_hold_their_cursor_without_blocking_other_events() {
    let server=MockServer::start().await;
    let cloud=MockCloud::default();
    Mock::given(TeamRoute).respond_with(cloud.clone()).mount(&server).await;
    let dir=tempfile::tempdir().unwrap(); let root=dir.path().to_path_buf(); let endpoint=server.uri();
    tokio::task::spawn_blocking(move || {
        let syncer=machine(&root,&endpoint,"acme/estimates");
        let event=NewQualityEvent::new("estimate",QualityPayload::Estimate {
            subject_kind:"pipeline".into(),subject:"merge_queue:abc1234".into(),typical_minutes:16.0,bad_day_minutes:31.0,
            evidence_count:1,references:vec![cas_store::EstimateReference { id:"run:1".into(),minutes:16.0 }],recipient:"client".into() });
        cas_store::record_quality_event(&root,&event).unwrap();
        cloud.0.lock().unwrap().reject_estimates=true;
        emit(&root,"ordinary");
        let first=syncer.sync_quality_events(true).unwrap();
        assert_eq!(first.pushed,1); assert_eq!(first.refused,1);
        assert_eq!(cloud.0.lock().unwrap().gets,1,"pull still runs");
        assert!(syncer.queue().get_metadata(super::EXTENDED_PUSH_CURSOR_KEY).unwrap().is_none());
        emit(&root,"later");
        syncer.queue().delete_metadata(super::LAST_ROUND_KEY).unwrap();
        assert_eq!(syncer.sync_quality_events(false).unwrap().pushed,1);
        cloud.0.lock().unwrap().reject_estimates=false;
        assert_eq!(syncer.sync_quality_events(true).unwrap().pushed,1);
        assert_eq!(syncer.sync_quality_events(true).unwrap().pushed,0);
        assert_eq!(cloud.0.lock().unwrap().rows.len(),3);
    }).await.unwrap();
}
