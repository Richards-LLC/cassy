//! Regression coverage for two factory sessions sharing one CAS project.
//!
//! This drives the store layer and `DirectorData::load` directly. No live tmux
//! or daemon process is required.

use cas_factory::DirectorData;
use cas_store::{
    AgentStore, EventStore, PromptQueueStore, SpawnAction, SpawnQueueStore, SqliteAgentStore,
    SqliteEventStore, SqlitePromptQueueStore, SqliteSpawnQueueStore, SqliteTaskStore, TaskStore,
};
use cas_types::{Agent, AgentRole, AgentStatus};
use tempfile::TempDir;

#[path = "../../../cas-cli/src/test_env_guard.rs"]
mod test_env_guard;
use test_env_guard::TestEnvGuard;

struct Stores {
    _temp: TempDir,
    cas_dir: std::path::PathBuf,
    agents: SqliteAgentStore,
    spawn_queue: SqliteSpawnQueueStore,
    prompt_queue: SqlitePromptQueueStore,
}

impl Stores {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp dir");
        let cas_dir = temp.path().to_path_buf();

        let tasks = SqliteTaskStore::open(&cas_dir).expect("task store");
        tasks.init().expect("init task store");
        let agents = SqliteAgentStore::open(&cas_dir).expect("agent store");
        agents.init().expect("init agent store");
        let events = SqliteEventStore::open(&cas_dir).expect("event store");
        events.init().expect("init event store");
        let spawn_queue = SqliteSpawnQueueStore::open(&cas_dir).expect("spawn queue");
        spawn_queue.init().expect("init spawn queue");
        let prompt_queue = SqlitePromptQueueStore::open(&cas_dir).expect("prompt queue");
        prompt_queue.init().expect("init prompt queue");

        Self {
            _temp: temp,
            cas_dir,
            agents,
            spawn_queue,
            prompt_queue,
        }
    }

    fn register_factory_agent(
        &self,
        id: &str,
        name: &str,
        role: AgentRole,
        factory_session: Option<&str>,
    ) {
        let mut agent = Agent::new(id.to_string(), name.to_string());
        agent.role = role;
        agent.status = AgentStatus::Active;
        agent.cc_session_id = Some(id.to_string());
        agent.factory_session = factory_session.map(str::to_string);
        self.agents.register(&agent).expect("register agent");

        if factory_session.is_none() {
            let mut stored = self.agents.get(id).expect("registered agent");
            stored.factory_session = None;
            self.agents
                .update(&stored)
                .expect("clear legacy factory_session");
        }
    }
}

fn agent_ids(data: &DirectorData) -> Vec<String> {
    let mut ids: Vec<String> = data.agents.iter().map(|agent| agent.id.clone()).collect();
    ids.sort();
    ids
}

fn load_director_for_session(
    env: &mut TestEnvGuard,
    cas_dir: &std::path::Path,
    factory_session: &str,
) -> DirectorData {
    env.set("CAS_FACTORY_SESSION", factory_session);
    let data = DirectorData::load_fast(cas_dir).expect("load director data");
    env.remove("CAS_FACTORY_SESSION");
    data
}

#[test]
fn two_concurrent_factory_sessions_are_isolated_with_legacy_null_compatibility() {
    let mut env = TestEnvGuard::temp_home();
    let stores = Stores::new();
    env.set("CAS_ROOT", &stores.cas_dir);

    // 1. Spawn/shutdown queue isolation: a request tagged for A is invisible
    // to B, then visible to A. Legacy NULL rows remain processable by either.
    stores
        .spawn_queue
        .enqueue_spawn(1, &[], false, None, Some("session-a"), None)
        .expect("enqueue session-a spawn");
    assert!(
        stores
            .spawn_queue
            .poll("session-b", 10)
            .expect("poll session-b spawn")
            .is_empty(),
        "session B must not execute session A spawn requests"
    );
    let a_spawn = stores
        .spawn_queue
        .poll("session-a", 10)
        .expect("poll session-a spawn");
    assert_eq!(a_spawn.len(), 1);
    assert_eq!(a_spawn[0].action, SpawnAction::Spawn);
    assert_eq!(a_spawn[0].factory_session.as_deref(), Some("session-a"));

    stores
        .spawn_queue
        .enqueue_shutdown(Some(1), &[], true, Some("session-a"))
        .expect("enqueue session-a shutdown");
    assert!(
        stores
            .spawn_queue
            .poll("session-b", 10)
            .expect("poll session-b shutdown")
            .is_empty(),
        "session B must not execute session A shutdown requests"
    );
    let a_shutdown = stores
        .spawn_queue
        .poll("session-a", 10)
        .expect("poll session-a shutdown");
    assert_eq!(a_shutdown.len(), 1);
    assert_eq!(a_shutdown[0].action, SpawnAction::Shutdown);
    assert_eq!(a_shutdown[0].factory_session.as_deref(), Some("session-a"));

    stores
        .spawn_queue
        .enqueue_spawn(2, &[], false, None, None, None)
        .expect("enqueue legacy spawn");
    let legacy_spawn = stores
        .spawn_queue
        .poll("session-b", 10)
        .expect("poll legacy spawn");
    assert_eq!(legacy_spawn.len(), 1);
    assert_eq!(legacy_spawn[0].action, SpawnAction::Spawn);
    assert!(legacy_spawn[0].factory_session.is_none());

    // 2. Agent visibility: each director sees only its own session's factory
    // agents. Plain NULL-session agents are hidden from factory directors.
    stores.register_factory_agent(
        "supervisor-a-id",
        "supervisor-a",
        AgentRole::Supervisor,
        Some("session-a"),
    );
    stores.register_factory_agent(
        "worker-a-id",
        "same-worker-name",
        AgentRole::Worker,
        Some("session-a"),
    );
    stores.register_factory_agent(
        "supervisor-b-id",
        "supervisor-b",
        AgentRole::Supervisor,
        Some("session-b"),
    );
    stores.register_factory_agent(
        "worker-b-id",
        "same-worker-name",
        AgentRole::Worker,
        Some("session-b"),
    );
    stores.register_factory_agent("plain-id", "plain-agent", AgentRole::Standard, None);

    let director_a = load_director_for_session(&mut env, &stores.cas_dir, "session-a");
    assert_eq!(
        agent_ids(&director_a),
        vec!["supervisor-a-id".to_string(), "worker-a-id".to_string()]
    );
    let director_b = load_director_for_session(&mut env, &stores.cas_dir, "session-b");
    assert_eq!(
        agent_ids(&director_b),
        vec!["supervisor-b-id".to_string(), "worker-b-id".to_string()]
    );

    // 3. Prompt delivery: target-name collisions do not cross session
    // boundaries, and all_workers tagged by A is only visible to A.
    stores
        .prompt_queue
        .enqueue_with_session(
            "supervisor-a",
            "same-worker-name",
            "direct to A worker",
            "session-a",
        )
        .expect("enqueue direct prompt");
    stores
        .prompt_queue
        .enqueue_with_session(
            "supervisor-a",
            "all_workers",
            "broadcast to A workers",
            "session-a",
        )
        .expect("enqueue all_workers prompt");

    let b_targets = ["supervisor-b", "same-worker-name", "all_workers"];
    let prompts_for_b = stores
        .prompt_queue
        .peek_for_targets(&b_targets, Some("session-b"), 10)
        .expect("peek prompts for session b");
    assert!(
        prompts_for_b.is_empty(),
        "same worker pane names must not leak A prompts to B"
    );

    let a_targets = ["supervisor-a", "same-worker-name", "all_workers"];
    let prompts_for_a = stores
        .prompt_queue
        .peek_for_targets(&a_targets, Some("session-a"), 10)
        .expect("peek prompts for session a");
    assert_eq!(prompts_for_a.len(), 2);
    assert!(
        prompts_for_a
            .iter()
            .all(|prompt| prompt.factory_session.as_deref() == Some("session-a"))
    );
    assert!(
        prompts_for_a
            .iter()
            .any(|prompt| prompt.target == "all_workers")
    );
    assert!(
        prompts_for_a
            .iter()
            .any(|prompt| prompt.target == "same-worker-name")
    );

    stores
        .prompt_queue
        .enqueue_with_session(
            "supervisor-b",
            "same-worker-name",
            "direct to B worker",
            "session-b",
        )
        .expect("enqueue session-b direct prompt");

    let director_a_with_messages =
        load_director_for_session(&mut env, &stores.cas_dir, "session-a");
    let worker_a_summary = director_a_with_messages
        .agents
        .iter()
        .find(|agent| agent.id == "worker-a-id")
        .expect("session-a worker summary");
    assert_eq!(
        worker_a_summary.pending_messages, 1,
        "session-a same-name worker should count only session-a direct messages"
    );

    let director_b_with_messages =
        load_director_for_session(&mut env, &stores.cas_dir, "session-b");
    let worker_b_summary = director_b_with_messages
        .agents
        .iter()
        .find(|agent| agent.id == "worker-b-id")
        .expect("session-b worker summary");
    assert_eq!(
        worker_b_summary.pending_messages, 1,
        "session-b same-name worker should count only session-b direct messages"
    );

    // 4. Legacy NULL prompt rows keep the historical target/all_workers
    // delivery behavior even for a session-scoped daemon.
    stores
        .prompt_queue
        .clear()
        .expect("clear session prompts before legacy assertion");
    stores
        .prompt_queue
        .enqueue("legacy-supervisor", "same-worker-name", "legacy direct")
        .expect("enqueue legacy direct prompt");
    stores
        .prompt_queue
        .enqueue("legacy-supervisor", "all_workers", "legacy broadcast")
        .expect("enqueue legacy broadcast prompt");
    let legacy_prompts = stores
        .prompt_queue
        .poll_for_target_with_session("same-worker-name", Some("session-b"), 10)
        .expect("poll legacy prompts");
    assert_eq!(legacy_prompts.len(), 2);
    assert!(
        legacy_prompts
            .iter()
            .all(|prompt| prompt.factory_session.is_none())
    );
    assert!(
        legacy_prompts
            .iter()
            .any(|prompt| prompt.target == "same-worker-name")
    );
    assert!(
        legacy_prompts
            .iter()
            .any(|prompt| prompt.target == "all_workers")
    );
}
