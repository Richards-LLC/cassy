//! cas-2a5a: one user's supervisors message each other across projects and
//! machines, never across users.
//!
//! `FakeUserScopeCloud` is the contract of petra-stella-cloud#162 in memory:
//! a send in the `user` scope is queued only for a supervisor of the
//! caller's own user, a claim returns `user` rows only when it asks for that
//! scope, and `GET /api/agents` without a project lists the caller's agents.
use super::*;
use crate::cloud::coordinator::AgentInfo;
use crate::cloud::peers::{caller_user_id, user_peers};
use crate::store::{init_cas_dir, open_agent_store, open_prompt_queue_store};
use crate::types::{Agent, AgentRole};
use cas_store::QueueOrigin;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const CASSY: &str = "github.com/acme/cassy";
const VIOLET: &str = "github.com/acme/violet";

#[derive(Clone)]
struct Row {
    id: &'static str,
    name: &'static str,
    user: &'static str,
    role: &'static str,
    project: &'static str,
    machine: &'static str,
    session: &'static str,
}

#[derive(Clone)]
struct Stored {
    send: PeerSend,
    status: &'static str,
    id: String,
}

/// The cloud, shared by every client. Each client is one user's token.
#[derive(Default)]
struct FakeUserScopeCloud {
    agents: Mutex<Vec<Row>>,
    messages: Mutex<Vec<Stored>>,
    sends: Mutex<usize>,
}

impl FakeUserScopeCloud {
    fn new(rows: &[Row]) -> Arc<Self> {
        Arc::new(Self {
            agents: Mutex::new(rows.to_vec()),
            ..Default::default()
        })
    }

    fn row(&self, id: &str) -> Option<Row> {
        self.agents.lock().unwrap().iter().find(|row| row.id == id).cloned()
    }

    /// `GET /api/agents` without `project`: the caller's own agents only.
    fn list_agents(&self, user: &str) -> Vec<AgentInfo> {
        self.all_agents()
            .into_iter()
            .filter(|agent| agent.user_id.as_deref() == Some(user))
            .collect()
    }

    /// A server that leaked other users' rows (a teammate on a shared team
    /// project): the client must still list none of them.
    fn all_agents(&self) -> Vec<AgentInfo> {
        self.agents
            .lock()
            .unwrap()
            .iter()
            .map(|row| AgentInfo {
                id: row.id.into(),
                name: row.name.into(),
                agent_type: "primary".into(),
                status: "active".into(),
                pid: None,
                session_id: None,
                machine_id: Some(format!("id-{}", row.machine)),
                last_heartbeat: chrono::Utc::now().to_rfc3339(),
                active_tasks: 0,
                metadata: HashMap::from([
                    ("role".to_string(), row.role.to_string()),
                    ("canonical_id".to_string(), row.project.to_string()),
                    ("hostname".to_string(), row.machine.to_string()),
                    ("factory_session".to_string(), row.session.to_string()),
                ]),
                user_id: Some(row.user.into()),
                team_id: Some("shared-team".into()),
                project_id: None,
            })
            .collect()
    }

    /// `POST /api/peer-messages`, as #162 specifies.
    fn send_as(&self, user: &str, message: &PeerSend) -> Result<SendReceipt, String> {
        *self.sends.lock().unwrap() += 1;
        let sender = self.row(&message.sender_agent_id);
        if sender.as_ref().is_none_or(|row| row.user != user) {
            return Err("403 sender_not_owned".into());
        }
        if let Some(existing) = self
            .messages
            .lock()
            .unwrap()
            .iter()
            .find(|stored| stored.send.dedupe_key == message.dedupe_key)
        {
            return Ok(SendReceipt { id: existing.id.clone(), status: "duplicate".into() });
        }
        let recipient = self.row(&message.recipient_agent_id);
        let addressable = recipient.as_ref().is_some_and(|row| match message.scope {
            // Team membership is never consulted for the user scope.
            PeerScope::User => row.user == user && row.role == "supervisor",
            PeerScope::Project => row.user == user && row.project == message.project_id,
        });
        if !addressable {
            return Err("404 recipient_not_found".into());
        }
        let mut messages = self.messages.lock().unwrap();
        let id = format!("pm-{}", messages.len() + 1);
        messages.push(Stored { send: message.clone(), status: "queued", id: id.clone() });
        Ok(SendReceipt { id, status: "queued".into() })
    }

    /// `POST /api/peer-messages/claim` with explicit `scopes`.
    fn claim_as(
        &self,
        user: &str,
        project_ids: &[String],
        recipients: &[String],
        scopes: &[PeerScope],
    ) -> Vec<ClaimedPeerMessage> {
        let mut claimed = Vec::new();
        for stored in self.messages.lock().unwrap().iter_mut() {
            let owned = recipients.contains(&stored.send.recipient_agent_id)
                && self.row(&stored.send.recipient_agent_id).is_some_and(|row| row.user == user);
            let in_scope = match stored.send.scope {
                PeerScope::Project => {
                    scopes.contains(&PeerScope::Project)
                        && project_ids.contains(&stored.send.project_id)
                }
                PeerScope::User => scopes.contains(&PeerScope::User),
            };
            if !owned || !in_scope || stored.status != "queued" {
                continue;
            }
            stored.status = "leased";
            let sender = self.row(&stored.send.sender_agent_id).expect("sender row");
            claimed.push(ClaimedPeerMessage {
                id: stored.id.clone(),
                project_id: stored.send.project_id.clone(),
                recipient_agent_id: stored.send.recipient_agent_id.clone(),
                sender_agent_id: sender.id.into(),
                sender_name: sender.name.into(),
                sender_machine_id: Some(format!("id-{}", sender.machine)),
                sender_session: Some(sender.session.into()),
                body: stored.send.body.clone(),
                summary: stored.send.summary.clone(),
                in_reply_to: stored.send.in_reply_to.clone(),
                created_at: "2026-10-16T13:30:00Z".into(),
                attempts: 1,
                sender_label: None,
                scope: stored.send.scope,
            });
        }
        claimed
    }

    /// A lapsed lease, or an ack the cloud never received: the message is
    /// claimable again.
    fn expire_leases(&self) {
        for stored in self.messages.lock().unwrap().iter_mut() {
            if matches!(stored.status, "leased" | "delivered") {
                stored.status = "queued";
            }
        }
    }
}

/// One user's authenticated client, as the daemon's `HttpPeerMailbox`.
struct Client {
    cloud: Arc<FakeUserScopeCloud>,
    user: &'static str,
}

impl PeerMailbox for Client {
    fn send(&self, message: &PeerSend) -> Result<SendReceipt, String> {
        self.cloud.send_as(self.user, message)
    }
    fn claim(
        &self,
        project_ids: &[String],
        recipient_agent_ids: &[String],
        _consumer_id: &str,
    ) -> Result<Vec<ClaimedPeerMessage>, String> {
        // HttpPeerMailbox always asks for both scopes.
        Ok(self.cloud.claim_as(
            self.user,
            project_ids,
            recipient_agent_ids,
            &[PeerScope::Project, PeerScope::User],
        ))
    }
    fn ack(&self, _consumer_id: &str, acks: &[(String, PeerAckOutcome)]) -> Result<(), String> {
        for stored in self.cloud.messages.lock().unwrap().iter_mut() {
            if let Some((_, outcome)) = acks.iter().find(|(id, _)| *id == stored.id) {
                stored.status = match outcome {
                    PeerAckOutcome::Delivered => "delivered",
                    PeerAckOutcome::Rejected => "rejected",
                };
            }
        }
        Ok(())
    }
    fn status(&self, _id: &str) -> Result<PeerMessageStatus, String> {
        unreachable!("not used")
    }
    fn sender_labels(&self, project_id: &str, sender_ids: &[String]) -> HashMap<String, String> {
        self.cloud
            .agents
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row.project == project_id && sender_ids.iter().any(|id| id == row.id))
            .map(|row| (row.id.to_string(), row.machine.to_string()))
            .collect()
    }
}

const fn sup(
    id: &'static str,
    name: &'static str,
    user: &'static str,
    project: &'static str,
    machine: &'static str,
) -> Row {
    Row { id, name, user, role: "supervisor", project, machine, session: id }
}

/// The receiving project's clone: a live supervisor in factory `session`.
struct Receiver {
    dir: tempfile::TempDir,
    cas: std::path::PathBuf,
}

impl Receiver {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cas = init_cas_dir(dir.path()).unwrap();
        Self { dir, cas }
    }
    fn register(&self, id: &str, name: &str, role: AgentRole, session: &str) {
        let mut agent = Agent::new(id.to_string(), name.to_string());
        agent.role = role;
        agent.factory_session = Some(session.to_string());
        open_agent_store(&self.cas).unwrap().register(&agent).unwrap();
    }
    fn deliver(&self, mailbox: &dyn PeerMailbox, project: &str, session: &str) -> DeliveryReport {
        deliver_claimed(
            mailbox,
            open_prompt_queue_store(&self.cas).unwrap().as_ref(),
            open_agent_store(&self.cas).unwrap().as_ref(),
            project,
            &[],
            session,
            "consumer",
        )
    }
    fn rows(&self) -> Vec<cas_store::QueuedPrompt> {
        let mut rows = open_prompt_queue_store(&self.cas).unwrap().peek_all(100).unwrap();
        rows.sort_by_key(|row| row.id);
        rows
    }
}

/// What `send_to_user_peer` hands the outbox for `peer`.
fn user_send(sender: &str, peer: &Peer, body: &str, key: &str) -> PeerSend {
    PeerSend {
        project_id: CASSY.into(),
        sender_agent_id: sender.into(),
        recipient_agent_id: peer.agent_id.clone(),
        body: body.into(),
        summary: Some("how did you build X?".into()),
        in_reply_to: None,
        dedupe_key: key.into(),
        scope: PeerScope::User,
        recipient_project_id: peer.canonical_id.clone(),
    }
}

fn directory(cloud: &FakeUserScopeCloud, user: &str, self_id: &str, leaky: bool) -> Vec<Peer> {
    let agents = if leaky { cloud.all_agents() } else { cloud.list_agents(user) };
    let owner = caller_user_id(&agents, Some(self_id)).expect("caller's user");
    user_peers(&agents, &owner, Some(self_id), chrono::Utc::now())
}

#[test]
fn cas_2a5a_one_users_supervisor_lists_and_messages_another_project_once() {
    // Across machines, and both supervisors on one machine.
    for violet_machine in ["prowl", "soundwave"] {
        let rows = [
            sup("sup-cassy", "happy-kestrel-54", "u1", CASSY, "soundwave"),
            sup("sup-violet", "bright-newt-56", "u1", VIOLET, violet_machine),
        ];
        let cloud = FakeUserScopeCloud::new(&rows);
        let peers = directory(&cloud, "u1", "sup-cassy", false);
        assert_eq!(peers.len(), 1, "{peers:?}");
        assert_eq!(peers[0].project_name().as_deref(), Some("violet"));
        let peer = resolve_user_peer_target(&peers, "supervisor:violet").unwrap().unwrap();
        assert_eq!(peer.agent_id, "sup-violet");

        let sender = Client { cloud: cloud.clone(), user: "u1" };
        let receipt = sender
            .send(&user_send("sup-cassy", peer, "Is the hub retry reusable?", "k1"))
            .expect("queued for the same user's supervisor in another project");
        assert_eq!(receipt.status, "queued");

        let violet = Receiver::new();
        violet.register("sup-violet", "bright-newt-56", AgentRole::Supervisor, "sup-violet");
        let recipient = Client { cloud: cloud.clone(), user: "u1" };
        let report = violet.deliver(&recipient, VIOLET, "sup-violet");
        assert_eq!((report.claimed, report.delivered), (1, 1), "{report:?}");
        let rows = violet.rows();
        assert_eq!(rows.len(), 1);
        let envelope = parse_envelope(&rows[0].prompt).expect("daemon-written envelope");
        assert_eq!(envelope.scope, PeerScope::User);
        assert_eq!(envelope.repo, CASSY, "the sender's project");
        assert_eq!(envelope.machine, "soundwave", "the sender's machine");
        assert_eq!(envelope.sender_name, "happy-kestrel-54");
        // pin: the recipient is told the sender is another project of the same account.
        assert!(rows[0].prompt.contains("another project of your account"));
        // pin: delivered wording the recipient supervisor acts on.
        assert!(rows[0].prompt.ends_with("Is the hub retry reusable?"));

        // A lost ack redelivers: admitted once, acked again.
        cloud.expire_leases();
        let again = violet.deliver(&recipient, VIOLET, "sup-violet");
        assert_eq!((again.delivered, again.duplicates), (0, 1));
        assert_eq!(violet.rows().len(), 1, "received once");
    }
}

#[test]
fn cas_2a5a_another_users_supervisor_is_never_listed_or_addressable() {
    let rows = [
        sup("sup-cassy", "happy-kestrel-54", "u1", CASSY, "soundwave"),
        // Another person's supervisor, on the same team and project name.
        sup("sup-theirs", "true-dragon-23", "u2", VIOLET, "prowl"),
    ];
    let cloud = FakeUserScopeCloud::new(&rows);

    // Even a server that leaks another user's rows lists none of them.
    let peers = directory(&cloud, "u1", "sup-cassy", true);
    assert!(peers.is_empty(), "{peers:?}");
    assert_eq!(resolve_user_peer_target(&peers, "supervisor:violet"), Ok(None));
    assert_eq!(resolve_user_peer_target(&peers, "true-dragon-23"), Ok(None));
    assert_eq!(resolve_user_peer_target(&peers, "sup-theirs"), Ok(None));

    // Mixed accounts with no registered caller: list nobody.
    assert!(caller_user_id(&cloud.all_agents(), None).is_err());
    assert!(caller_user_id(&cloud.all_agents(), Some("unknown")).is_err());

    // Cassy refuses before any network call: the cas root does not exist,
    // so reaching the outbox or the cloud would fail differently.
    let theirs = crate::cloud::peers::user_peers(
        &cloud.all_agents(),
        "u2",
        None,
        chrono::Utc::now(),
    )
    .into_iter()
    .find(|peer| peer.agent_id == "sup-theirs")
    .unwrap();
    let error = send_to_user_peer(
        std::path::Path::new("/nonexistent/cas-2a5a/.cas"),
        "sup-cassy",
        "u1",
        &theirs,
        "hello",
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("not a supervisor of your Cassy Cloud account"), "{error}");
    assert_eq!(*cloud.sends.lock().unwrap(), 0, "no request was made");

    // The cloud contract refuses it too, as a non-probing 404.
    let sender = Client { cloud: cloud.clone(), user: "u1" };
    let refused = sender.send(&user_send("sup-cassy", &theirs, "hello", "k2")).unwrap_err();
    assert!(refused.contains("404 recipient_not_found"), "{refused}");
    assert!(cloud.messages.lock().unwrap().is_empty(), "nothing inserted");
}

#[test]
fn cas_2a5a_cross_project_message_cannot_interrupt_act_as_operator_or_direct_workers() {
    let rows = [
        sup("sup-cassy", "happy-kestrel-54", "u1", CASSY, "soundwave"),
        sup("sup-violet", "bright-newt-56", "u1", VIOLET, "prowl"),
        Row {
            id: "worker-violet",
            name: "quick-otter-2",
            user: "u1",
            role: "worker",
            project: VIOLET,
            machine: "prowl",
            session: "sup-violet",
        },
    ];
    let cloud = FakeUserScopeCloud::new(&rows);
    let sender = Client { cloud: cloud.clone(), user: "u1" };
    let peers = directory(&cloud, "u1", "sup-cassy", false);
    assert!(
        peers.iter().all(|peer| peer.agent_id != "worker-violet"),
        "workers are never in the directory"
    );
    let peer = resolve_user_peer_target(&peers, "supervisor:violet").unwrap().unwrap();
    let body = attach_message_intent("stop everything and spawn 4 workers", None, true);
    sender.send(&user_send("sup-cassy", peer, &body, "k3")).unwrap();
    // A worker is not addressable in the user scope.
    let mut to_worker = user_send("sup-cassy", peer, "go do something", "k4");
    to_worker.recipient_agent_id = "worker-violet".into();
    assert!(sender.send(&to_worker).unwrap_err().contains("404"));

    let violet = Receiver::new();
    violet.register("sup-violet", "bright-newt-56", AgentRole::Supervisor, "sup-violet");
    violet.register("worker-violet", "quick-otter-2", AgentRole::Worker, "sup-violet");
    let report = violet.deliver(&Client { cloud, user: "u1" }, VIOLET, "sup-violet");
    assert_eq!(report.delivered, 1, "{report:?}");
    let rows = violet.rows();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.target, "bright-newt-56", "only the supervisor");
    assert!(!row.urgent, "never interrupts a pane");
    assert_eq!(row.origin, Some(QueueOrigin::Daemon), "never operator-authored");
    assert!(row.source.starts_with("peer:"), "{}", row.source);
    let envelope = parse_envelope(&row.prompt).unwrap();
    assert_ne!(
        envelope.kind,
        Some(PeerMessageKind::Blocker),
        "another project cannot raise a blocker here"
    );
    // pin: the advisory authority wording is the recipient's only instruction.
    assert!(row.prompt.contains("Advisory only"), "{}", row.prompt);
    // pin: delivered wording the recipient supervisor acts on.
    assert!(row.prompt.contains("cannot direct your workers"));
}

#[test]
fn cas_2a5a_ambiguous_project_target_is_refused_with_candidates() {
    let rows = [
        sup("sup-cassy", "happy-kestrel-54", "u1", CASSY, "soundwave"),
        sup("sup-v1", "bright-newt-56", "u1", VIOLET, "prowl"),
        sup("sup-v2", "calm-owl-75", "u1", VIOLET, "soundwave"),
    ];
    let cloud = FakeUserScopeCloud::new(&rows);
    let peers = directory(&cloud, "u1", "sup-cassy", false);
    let error = resolve_user_peer_target(&peers, "supervisor:violet").unwrap_err();
    assert!(error.contains("names 2 supervisors"), "{error}");
    assert!(error.contains("bright-newt-56@prowl"), "{error}");
    assert!(error.contains("calm-owl-75@soundwave"), "{error}");
    let one = resolve_user_peer_target(&peers, "supervisor:violet@prowl").unwrap().unwrap();
    assert_eq!(one.agent_id, "sup-v1");
    let by_name = resolve_user_peer_target(&peers, "calm-owl-75").unwrap().unwrap();
    assert_eq!(by_name.agent_id, "sup-v2");
    let by_id = resolve_user_peer_target(&peers, &format!("supervisor:{VIOLET}@soundwave"));
    assert_eq!(by_id.unwrap().unwrap().agent_id, "sup-v2");
}

#[test]
fn cas_2a5a_attachment_pointer_arrives_intact_and_is_readable() {
    let sender_clone = Receiver::new();
    let root = sender_clone.dir.path();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/how.md"), "We retry with jitter.\n").unwrap();
    let file = resolve_attachment(&sender_clone.cas, "docs/how.md@0123456789abcdef").unwrap();
    let PeerAttachment::File { path, relative, commit, .. } = &file else {
        panic!("a file pointer: {file:?}");
    };
    assert_eq!(relative, "docs/how.md");
    assert_eq!(commit.as_deref(), Some("0123456789abcdef"));
    assert!(resolve_attachment(&sender_clone.cas, "/etc/hostname").is_err(), "outside the repo");
    assert!(resolve_attachment(&sender_clone.cas, "docs/missing.md").is_err());

    let artifact = PeerAttachment::Artifact {
        id: "art-1".into(),
        name: Some("retry.pdf".into()),
        sha256: Some("ab".repeat(32)),
        url: Some("https://cloud.example/a/art-1".into()),
    };
    let body = attach_message_intent(
        &attach_pointers("Here's how we did it.", &[file.clone(), artifact.clone()]),
        Some("ask"),
        false,
    );
    let (_, rest) = split_message_intent(&body);
    let (pointers, text) = split_attachments(rest);
    assert_eq!(pointers, [file.clone(), artifact], "both pointers round-trip");
    assert_eq!(text, "Here's how we did it.");

    let rows = [
        sup("sup-gabber", "true-dragon-23", "u1", CASSY, "soundwave"),
        sup("sup-pulse", "wild-dolphin-72", "u1", VIOLET, "soundwave"),
    ];
    let cloud = FakeUserScopeCloud::new(&rows);
    let peers = directory(&cloud, "u1", "sup-gabber", false);
    let peer = resolve_user_peer_target(&peers, "wild-dolphin-72").unwrap().unwrap();
    Client { cloud: cloud.clone(), user: "u1" }
        .send(&user_send("sup-gabber", peer, &body, "k5"))
        .unwrap();
    let pulse = Receiver::new();
    pulse.register("sup-pulse", "wild-dolphin-72", AgentRole::Supervisor, "sup-pulse");
    pulse.deliver(&Client { cloud, user: "u1" }, VIOLET, "sup-pulse");
    let prompt = &pulse.rows()[0].prompt;
    let shown = format!("readable here at {path}");
    assert!(prompt.contains(&shown), "{prompt}");
    // pin: the attachment pointer and the body survive delivery verbatim.
    assert_eq!(std::fs::read_to_string(path).unwrap(), "We retry with jitter.\n");
    // pin: delivered wording the recipient supervisor acts on.
    assert!(prompt.contains("published artifact art-1 (retry.pdf)"), "{prompt}");
    // pin: delivered wording the recipient supervisor acts on.
    assert!(prompt.ends_with("Here's how we did it."), "{prompt}");
}

#[test]
fn cas_2a5a_project_scope_wire_and_delivery_are_unchanged() {
    let project = PeerSend {
        project_id: CASSY.into(),
        sender_agent_id: "a".into(),
        recipient_agent_id: "b".into(),
        body: "x".into(),
        summary: None,
        in_reply_to: None,
        dedupe_key: "k".into(),
        scope: PeerScope::Project,
        recipient_project_id: None,
    };
    let wire = serde_json::to_value(&project).unwrap();
    assert!(wire.get("scope").is_none(), "older servers see today's request: {wire}");
    assert!(wire.get("recipient_project_id").is_none());
    let user = serde_json::to_value(PeerSend { scope: PeerScope::User, ..project }).unwrap();
    assert_eq!(user["scope"], "user");

    // A project-scope message from another repo is still rejected.
    let rows = [
        sup("sup-cassy", "happy-kestrel-54", "u1", CASSY, "soundwave"),
        sup("sup-violet", "bright-newt-56", "u1", VIOLET, "prowl"),
    ];
    let cloud = FakeUserScopeCloud::new(&rows);
    cloud.messages.lock().unwrap().push(Stored {
        send: PeerSend {
            project_id: CASSY.into(),
            sender_agent_id: "sup-cassy".into(),
            recipient_agent_id: "sup-violet".into(),
            body: "wrong repo".into(),
            summary: None,
            in_reply_to: None,
            dedupe_key: "k7".into(),
            scope: PeerScope::Project,
            recipient_project_id: None,
        },
        status: "queued",
        id: "pm-x".into(),
    });
    let violet = Receiver::new();
    violet.register("sup-violet", "bright-newt-56", AgentRole::Supervisor, "sup-violet");
    let mailbox = Client { cloud, user: "u1" };
    let report = deliver_claimed(
        &OnlyProject(&mailbox),
        open_prompt_queue_store(&violet.cas).unwrap().as_ref(),
        open_agent_store(&violet.cas).unwrap().as_ref(),
        VIOLET,
        &[],
        "sup-violet",
        "consumer",
    );
    assert_eq!((report.rejected, report.delivered), (1, 0), "{report:?}");
    assert!(violet.rows().is_empty());
}

/// Returns every queued row regardless of project, so the daemon's own
/// same-repo check is what rejects a project-scope row from another repo.
struct OnlyProject<'a>(&'a Client);

impl PeerMailbox for OnlyProject<'_> {
    fn send(&self, message: &PeerSend) -> Result<SendReceipt, String> {
        self.0.send(message)
    }
    fn claim(&self, _: &[String], recipients: &[String], _: &str) -> Result<Vec<ClaimedPeerMessage>, String> {
        Ok(self.0.cloud.claim_as(
            self.0.user,
            &[CASSY.to_string(), VIOLET.to_string()],
            recipients,
            &[PeerScope::Project],
        ))
    }
    fn ack(&self, consumer: &str, acks: &[(String, PeerAckOutcome)]) -> Result<(), String> {
        self.0.ack(consumer, acks)
    }
    fn status(&self, id: &str) -> Result<PeerMessageStatus, String> {
        self.0.status(id)
    }
}

#[test]
fn cas_2a5a_project_names_come_from_canonical_ids() {
    use crate::cloud::peers::project_name;
    assert_eq!(project_name("github.com/Richards-LLC/violet").as_deref(), Some("violet"));
    assert_eq!(project_name("git@github.com:acme/pulse-card.git").as_deref(), Some("pulse-card"));
    assert_eq!(project_name("  "), None);
}

#[test]
fn cas_2a5a_directory_opens_with_a_verdict_and_collapses_stale_rows() {
    use crate::cloud::peers::render_user_peers;
    let now = chrono::Utc::now();
    let at = |name: &str, project: &str, machine: &str, session: &str, age: i64, focus: Option<&str>| Peer {
        agent_id: format!("{name}-{session}-{age}"),
        name: name.into(),
        machine: machine.into(),
        machine_id: None,
        session: Some(session.into()),
        canonical_id: Some(format!("github.com/acme/{project}")),
        focus: focus.map(str::to_string),
        focus_title: None,
        last_heartbeat: now - chrono::Duration::seconds(age),
        heartbeat_age_secs: age,
        live: age <= crate::cloud::peers::PEER_LIVE_SECS,
        user_id: Some("u1".into()),
        owner: None,
        branch: None,
        own: None,
        harness_auth_failed: None,
    };
    let mut peers = [
        at("bright-newt-56", "gabber", "prowl", "s1", 13, None),
        at("fierce-shark-94", "cloud", "soundwave", "s2", 16, Some("cas-8cb0")),
        at("true-dragon-23", "gabber", "soundwave", "s3", 3_600, None),
        // A second registration of the same supervisor collapses.
        at("true-dragon-23", "gabber", "soundwave", "s3", 7_200, None),
        at("swift-owl-75", "mines", "soundwave", "s4", 50_000, None),
    ];
    peers[1].focus_title = Some("Cloud rollout".into());
    let text = render_user_peers(&peers, false);
    let first = text.lines().next().unwrap();
    // pin: the verdict line in the sibling `cas peers` grammar.
    assert_eq!(first, "[OK] 4 supervisors in 3 projects | 2 live");
    // cas-8522: every live row declares its owned epic, or none.
    assert!(text.contains("| epic cas-8cb0 \"Cloud rollout\""), "{text}");
    assert!(text.contains("session s1 | epic none"), "{text}");
    assert!(!text.contains("true-dragon-23"), "stale rows collapse: {text}");
    // pin: stale supervisors are a count per project.
    assert!(text.contains("Stale, seen in the last 24h (2): gabber 1, mines 1."), "{text}");
    // pin: one copyable command to a live supervisor whose project is unique.
    assert!(text.contains("target=supervisor:gabber"), "{text}");
    assert!(text.lines().all(|line| line.chars().count() <= 80), "{text}");

    let verbose = render_user_peers(&peers, true);
    assert_eq!(verbose.matches("true-dragon-23").count(), 1, "{verbose}");
    assert!(verbose.contains("swift-owl-75"));

    // pin: the empty directory still opens with a verdict.
    assert!(render_user_peers(&[], false).starts_with("[OK] 0 supervisors in 0 projects | 0 live"));
}

/// cas-8522 (GH #1289): two live supervisors of one project. The project
/// address is refused with what each owns; its declared epic picks one.
#[test]
fn cas_8522_project_address_names_owned_epics_and_an_epic_resolves_its_owner() {
    let supervisor = |name: &str, epic: &str, title: &str| Peer {
        agent_id: format!("{name}-id"),
        name: name.into(),
        machine: "soundwave".into(),
        machine_id: None,
        session: Some(format!("{name}-session")),
        canonical_id: Some(CASSY.into()),
        focus: Some(epic.into()),
        focus_title: Some(title.into()),
        last_heartbeat: chrono::Utc::now(),
        heartbeat_age_secs: 5,
        live: true,
        user_id: Some("u1".into()),
        owner: None,
        branch: None,
        own: Some(true),
        harness_auth_failed: None,
    };
    let peers = [
        supervisor("zen-hound-24", "cas-fcf9", "Burn-down v50"),
        supervisor("swift-heron-11", "cas-2570", "Cloud rollout"),
    ];
    let refusal = resolve_user_peer_target(&peers, "supervisor:cassy").unwrap_err();
    assert!(refusal.contains("supervisor:cas-fcf9 (zen-hound-24@soundwave, cassy, owns epic cas-fcf9 \"Burn-down v50\")"), "{refusal}");
    assert!(refusal.contains("supervisor:cas-2570 (swift-heron-11@soundwave"), "{refusal}");
    let owner = resolve_user_peer_target(&peers, "supervisor:cas-2570").unwrap().unwrap();
    assert_eq!(owner.name, "swift-heron-11");
}
