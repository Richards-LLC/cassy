//! Integration tests for DirectorData loading from CAS stores.
//!
//! These tests verify:
//! - DirectorData loads correctly from SQLite stores
//! - Task filtering by status works correctly
//! - Agent summary generation works
//! - Epic grouping logic is correct
//! - Fast loading mode skips git changes

use cas_factory::{AgentSummary, DirectorData, EpicGroup, TaskSummary};
use cas_store::{
    AgentStore, EventStore, SqliteAgentStore, SqliteEventStore, SqliteTaskStore, TaskStore,
};
use cas_types::{
    Agent, AgentRole, AgentStatus, AgentType, Dependency, DependencyType, Priority, Task,
    TaskStatus, TaskType,
};
use tempfile::TempDir;

/// Helper to create a test CAS directory with initialized stores
fn setup_test_cas_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp directory")
}

/// Initialize task store with test schema
fn init_task_store(cas_dir: &std::path::Path) -> SqliteTaskStore {
    let store = SqliteTaskStore::open(cas_dir).expect("Failed to open task store");
    store.init().expect("Failed to init task store");
    store
}

/// Initialize agent store with test schema
fn init_agent_store(cas_dir: &std::path::Path) -> SqliteAgentStore {
    let store = SqliteAgentStore::open(cas_dir).expect("Failed to open agent store");
    store.init().expect("Failed to init agent store");
    store
}

/// Initialize event store with test schema
fn init_event_store(cas_dir: &std::path::Path) -> SqliteEventStore {
    let store = SqliteEventStore::open(cas_dir).expect("Failed to open event store");
    store.init().expect("Failed to init event store");
    store
}

/// Create a test task with given parameters
fn create_test_task(
    id: &str,
    title: &str,
    status: TaskStatus,
    task_type: TaskType,
    priority: Priority,
    assignee: Option<&str>,
) -> Task {
    let mut task = Task::new(id.to_string(), title.to_string());
    task.status = status;
    task.task_type = task_type;
    task.priority = priority;
    task.assignee = assignee.map(|s| s.to_string());
    task
}

/// Create a test agent with given parameters
fn create_test_agent(id: &str, name: &str, role: AgentRole, status: AgentStatus) -> Agent {
    let now = chrono::Utc::now();
    Agent {
        id: id.to_string(),
        name: name.to_string(),
        agent_type: AgentType::Primary,
        role,
        status,
        pid: None,
        ppid: None,
        cc_session_id: Some(id.to_string()),
        factory_session: None,
        parent_id: None,
        machine_id: None,
        registered_at: now,
        last_heartbeat: now,
        active_tasks: 0,
        pid_starttime: None,
        metadata: std::collections::HashMap::new(),
    }
}

// =============================================================================
// DirectorData Loading Tests
// =============================================================================

#[test]
fn test_director_data_load_empty_stores() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    // Initialize stores (creates tables)
    init_task_store(cas_dir);
    init_agent_store(cas_dir);
    init_event_store(cas_dir);

    // Load DirectorData from empty stores
    let result = DirectorData::load(cas_dir, None);
    assert!(
        result.is_ok(),
        "Should load from empty stores: {:?}",
        result.err()
    );

    let data = result.unwrap();
    assert!(data.ready_tasks.is_empty());
    assert!(data.in_progress_tasks.is_empty());
    assert!(data.epic_tasks.is_empty());
    assert!(data.agents.is_empty());
    assert!(data.activity.is_empty());
}

#[test]
fn test_director_data_load_fast() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    // Initialize stores
    init_task_store(cas_dir);
    init_agent_store(cas_dir);
    init_event_store(cas_dir);

    // Fast load should skip git changes
    let result = DirectorData::load_fast(cas_dir);
    assert!(result.is_ok());

    let data = result.unwrap();
    assert!(!data.git_loaded, "Fast load should not load git changes");
    assert!(data.changes.is_empty());
}

#[test]
fn test_director_data_loads_ready_tasks() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    let task_store = init_task_store(cas_dir);
    init_agent_store(cas_dir);
    init_event_store(cas_dir);

    // Add some open (ready) tasks
    let task1 = create_test_task(
        "cas-0001",
        "Task 1",
        TaskStatus::Open,
        TaskType::Task,
        Priority::HIGH,
        None,
    );
    let task2 = create_test_task(
        "cas-0002",
        "Task 2",
        TaskStatus::Open,
        TaskType::Feature,
        Priority::MEDIUM,
        None,
    );

    task_store.add(&task1).expect("Failed to add task1");
    task_store.add(&task2).expect("Failed to add task2");

    // Load DirectorData
    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");

    assert_eq!(data.ready_tasks.len(), 2, "Should have 2 ready tasks");
    assert!(data.ready_tasks.iter().any(|t| t.id == "cas-0001"));
    assert!(data.ready_tasks.iter().any(|t| t.id == "cas-0002"));
}

#[test]
fn test_director_data_loads_in_progress_tasks() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    let task_store = init_task_store(cas_dir);
    init_agent_store(cas_dir);
    init_event_store(cas_dir);

    // Add an in-progress task
    let task = create_test_task(
        "cas-0001",
        "In Progress Task",
        TaskStatus::InProgress,
        TaskType::Task,
        Priority::HIGH,
        Some("agent-1"),
    );

    task_store.add(&task).expect("Failed to add task");

    // Load DirectorData
    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");

    assert_eq!(data.in_progress_tasks.len(), 1);
    assert_eq!(data.in_progress_tasks[0].id, "cas-0001");
    assert_eq!(
        data.in_progress_tasks[0].assignee,
        Some("agent-1".to_string())
    );
}

#[test]
fn test_director_data_active_lease_resolves_awaiting_merge_via_assignee_after_lease_release() {
    // cas-627f: `park_task_awaiting_merge` (cas-8d5b) parks a merge-gate-
    // rejected close as `AwaitingMerge` and DELIBERATELY releases the
    // worker's lease (so the one-task gate doesn't block their next `task
    // start`). Confirmed P1: `active_lease` used to be resolved solely from
    // `list_agent_leases`, which returns only `status='active'` rows — so a
    // parked task's lease vanished from that view and `active_lease` came
    // back `None`, making the flagship close-rejected `WorkerIdle`
    // notification unreachable. `active_lease` must still resolve here via
    // the task table (assignee + `AwaitingMerge` status) even with zero
    // active lease rows for this agent.
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    let task_store = init_task_store(cas_dir);
    let agent_store = init_agent_store(cas_dir);
    init_event_store(cas_dir);

    let worker = create_test_agent(
        "worker-1",
        "swift-fox",
        AgentRole::Worker,
        AgentStatus::Idle,
    );
    agent_store.register(&worker).expect("Failed to add worker");

    let task = create_test_task(
        "cas-0001",
        "Parked Task",
        TaskStatus::AwaitingMerge,
        TaskType::Task,
        Priority::HIGH,
        Some("swift-fox"),
    );
    task_store.add(&task).expect("Failed to add task");

    // No lease is ever created for this agent/task — mirrors the released
    // state after park_task_awaiting_merge runs.
    let leases = agent_store
        .list_agent_leases("worker-1")
        .expect("list_agent_leases should not error");
    assert!(
        leases.is_empty(),
        "test setup: no active lease should exist for the parked task"
    );

    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");

    let agent_summary = data
        .agents
        .iter()
        .find(|a| a.name == "swift-fox")
        .expect("swift-fox should be present in agents");
    let lease = agent_summary.active_lease.as_ref().expect(
        "active_lease should resolve for a parked AwaitingMerge task even with no active lease row",
    );
    assert_eq!(lease.task_id, "cas-0001");
    assert_eq!(lease.task_status, TaskStatus::AwaitingMerge);
}

#[test]
fn test_director_data_excludes_epics_from_regular_tasks() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    let task_store = init_task_store(cas_dir);
    init_agent_store(cas_dir);
    init_event_store(cas_dir);

    // Add an epic task
    let epic = create_test_task(
        "cas-epic",
        "Epic Task",
        TaskStatus::InProgress,
        TaskType::Epic,
        Priority::CRITICAL,
        None,
    );

    // Add a regular task
    let task = create_test_task(
        "cas-0001",
        "Regular Task",
        TaskStatus::InProgress,
        TaskType::Task,
        Priority::HIGH,
        None,
    );

    task_store.add(&epic).expect("Failed to add epic");
    task_store.add(&task).expect("Failed to add task");

    // Load DirectorData
    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");

    // Epic should be in epic_tasks, not in in_progress_tasks
    assert_eq!(data.in_progress_tasks.len(), 1);
    assert_eq!(data.in_progress_tasks[0].id, "cas-0001");

    assert_eq!(data.epic_tasks.len(), 1);
    assert_eq!(data.epic_tasks[0].id, "cas-epic");
}

#[test]
fn test_director_data_loads_agents() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    init_task_store(cas_dir);
    let agent_store = init_agent_store(cas_dir);
    init_event_store(cas_dir);

    // Add supervisor and worker agents
    let supervisor = create_test_agent(
        "supervisor-1",
        "quiet-condor",
        AgentRole::Supervisor,
        AgentStatus::Active,
    );
    let worker = create_test_agent(
        "worker-1",
        "swift-fox",
        AgentRole::Worker,
        AgentStatus::Idle,
    );

    agent_store
        .register(&supervisor)
        .expect("Failed to add supervisor");
    agent_store.register(&worker).expect("Failed to add worker");

    // Load DirectorData
    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");

    assert_eq!(data.agents.len(), 2, "Should have 2 agents");
    assert!(data.agents.iter().any(|a| a.name == "quiet-condor"));
    assert!(data.agents.iter().any(|a| a.name == "swift-fox"));
}

#[test]
fn test_director_data_filters_inactive_agents() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    init_task_store(cas_dir);
    let agent_store = init_agent_store(cas_dir);
    init_event_store(cas_dir);

    // Add an inactive agent (should be filtered out)
    let now = chrono::Utc::now();
    let inactive_agent = Agent {
        id: "agent-1".to_string(),
        name: "inactive-agent".to_string(),
        agent_type: AgentType::Primary,
        role: AgentRole::Worker,
        status: AgentStatus::Shutdown, // Inactive
        pid: None,
        ppid: None,
        cc_session_id: None,
        factory_session: None,
        parent_id: None,
        machine_id: None,
        registered_at: now,
        last_heartbeat: now,
        active_tasks: 0,
        pid_starttime: None,
        metadata: std::collections::HashMap::new(),
    };

    agent_store
        .register(&inactive_agent)
        .expect("Failed to add agent");

    // Load DirectorData
    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");

    // Inactive agents should be filtered out
    assert!(
        data.agents.is_empty(),
        "Inactive agents should be filtered out"
    );
}

#[test]
fn test_director_data_builds_agent_id_to_name_map() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    init_task_store(cas_dir);
    let agent_store = init_agent_store(cas_dir);
    init_event_store(cas_dir);

    let agent = create_test_agent(
        "agent-123",
        "cool-panda",
        AgentRole::Worker,
        AgentStatus::Active,
    );

    agent_store.register(&agent).expect("Failed to add agent");

    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");

    // Check agent_id_to_name map
    assert!(data.agent_id_to_name.contains_key("agent-123"));
    assert_eq!(
        data.agent_id_to_name.get("agent-123"),
        Some(&"cool-panda".to_string())
    );
}

// =============================================================================
// Epic Grouping Tests
// =============================================================================

#[test]
fn test_tasks_by_epic_empty() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    init_task_store(cas_dir);
    init_agent_store(cas_dir);
    init_event_store(cas_dir);

    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");

    let (groups, standalone) = data.tasks_by_epic();

    assert!(groups.is_empty());
    assert!(standalone.is_empty());
}

#[test]
fn test_tasks_by_epic_standalone_tasks() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    let task_store = init_task_store(cas_dir);
    init_agent_store(cas_dir);
    init_event_store(cas_dir);

    // Add tasks without epic association
    let task1 = create_test_task(
        "cas-0001",
        "Standalone Task 1",
        TaskStatus::Open,
        TaskType::Task,
        Priority::HIGH,
        None,
    );
    let task2 = create_test_task(
        "cas-0002",
        "Standalone Task 2",
        TaskStatus::InProgress,
        TaskType::Task,
        Priority::MEDIUM,
        None,
    );

    task_store.add(&task1).expect("Failed to add task1");
    task_store.add(&task2).expect("Failed to add task2");

    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");
    let (groups, standalone) = data.tasks_by_epic();

    // No epic groups (no epics in the test data)
    assert!(groups.is_empty());

    // Both tasks should be standalone
    assert_eq!(standalone.len(), 2);
}

#[test]
fn test_tasks_by_epic_with_parent_child_dependency() {
    let temp_dir = setup_test_cas_dir();
    let cas_dir = temp_dir.path();

    let task_store = init_task_store(cas_dir);
    init_agent_store(cas_dir);
    init_event_store(cas_dir);

    // Create an epic
    let epic = create_test_task(
        "cas-epic",
        "Test Epic",
        TaskStatus::InProgress,
        TaskType::Epic,
        Priority::CRITICAL,
        None,
    );

    // Create subtasks
    let subtask1 = create_test_task(
        "cas-0001",
        "Subtask 1",
        TaskStatus::Open,
        TaskType::Task,
        Priority::HIGH,
        None,
    );
    let subtask2 = create_test_task(
        "cas-0002",
        "Subtask 2",
        TaskStatus::InProgress,
        TaskType::Task,
        Priority::HIGH,
        None,
    );

    task_store.add(&epic).expect("Failed to add epic");
    task_store.add(&subtask1).expect("Failed to add subtask1");
    task_store.add(&subtask2).expect("Failed to add subtask2");

    // Create parent-child dependencies
    let dep1 = Dependency {
        from_id: "cas-0001".to_string(),
        to_id: "cas-epic".to_string(),
        dep_type: DependencyType::ParentChild,
        created_at: chrono::Utc::now(),
        created_by: None,
    };
    let dep2 = Dependency {
        from_id: "cas-0002".to_string(),
        to_id: "cas-epic".to_string(),
        dep_type: DependencyType::ParentChild,
        created_at: chrono::Utc::now(),
        created_by: None,
    };

    task_store
        .add_dependency(&dep1)
        .expect("Failed to add dep1");
    task_store
        .add_dependency(&dep2)
        .expect("Failed to add dep2");

    let data = DirectorData::load_fast(cas_dir).expect("Failed to load DirectorData");
    let (groups, standalone) = data.tasks_by_epic();

    // Should have one epic group with 2 subtasks
    assert_eq!(groups.len(), 1, "Should have one epic group");
    assert_eq!(groups[0].epic.id, "cas-epic");
    assert_eq!(groups[0].subtasks.len(), 2, "Epic should have 2 subtasks");
    assert!(groups[0].has_active, "Epic should have active subtasks");

    // No standalone tasks
    assert!(standalone.is_empty(), "All tasks belong to epic");
}

// =============================================================================
// Recency ordering tests (cas-2fb6)
// =============================================================================

/// Build a `TaskSummary` with an explicit `updated_at` for deterministic
/// recency-ordering assertions.
fn summary_at(
    id: &str,
    status: TaskStatus,
    task_type: TaskType,
    priority: Priority,
    epic: Option<&str>,
    updated_at: chrono::DateTime<chrono::Utc>,
) -> TaskSummary {
    TaskSummary {
        id: id.to_string(),
        title: format!("title {id}"),
        status,
        priority,
        assignee: None,
        task_type,
        epic: epic.map(|s| s.to_string()),
        branch: if task_type == TaskType::Epic {
            Some(format!("epic/{id}"))
        } else {
            None
        },
        updated_at: Some(updated_at),
        epic_verification_owner: None,
    }
    }

/// Assemble a minimal `DirectorData` from explicit task summaries. Tasks are
/// partitioned into ready/in_progress/epic buckets the same way `load_with_stores`
/// does, so `tasks_by_epic` sees the same shape it would in production.
fn data_from_tasks(tasks: Vec<TaskSummary>) -> DirectorData {
    let mut ready_tasks = Vec::new();
    let mut in_progress_tasks = Vec::new();
    let mut epic_tasks = Vec::new();
    for t in tasks {
        match (t.task_type, t.status) {
            (TaskType::Epic, _) => epic_tasks.push(t),
            (_, TaskStatus::InProgress) => {
                in_progress_tasks.push(t)
            }
            _ => ready_tasks.push(t),
        }
    }
    DirectorData {
        ready_tasks,
        in_progress_tasks,
        epic_tasks,
        agents: Vec::new(),
        activity: Vec::new(),
        agent_id_to_name: std::collections::HashMap::new(),
        changes: Vec::new(),
        git_loaded: false,
        reminders: Vec::new(),
        epic_closed_counts: std::collections::HashMap::new(),
    }
}

/// A stale epic with MANY low-priority subtasks must NOT pin to the top of the
/// TASKS panel over a smaller, recently-active epic. The panel orders epic
/// groups by recency of activity, not subtask count or static priority.
#[test]
fn test_tasks_by_epic_recent_epic_outranks_stale_high_count_epic() {
    let old = chrono::Utc::now() - chrono::Duration::days(5);
    let recent = chrono::Utc::now();

    let mut tasks = Vec::new();

    // Stale follow-up epic: 14 untouched P4 subtasks (the reported symptom).
    tasks.push(summary_at(
        "cas-stale",
        TaskStatus::Open,
        TaskType::Epic,
        Priority::HIGH,
        None,
        old,
    ));
    for i in 0..14 {
        tasks.push(summary_at(
            &format!("cas-stale-{i}"),
            TaskStatus::Open,
            TaskType::Task,
            Priority::BACKLOG,
            Some("cas-stale"),
            old,
        ));
    }

    // The supervisor's CURRENT focus: a small, recently-touched epic with a
    // single subtask, at a numerically lower (= less urgent) static priority.
    tasks.push(summary_at(
        "cas-current",
        TaskStatus::Open,
        TaskType::Epic,
        Priority::MEDIUM,
        None,
        recent,
    ));
    tasks.push(summary_at(
        "cas-current-0",
        TaskStatus::Open,
        TaskType::Task,
        Priority::MEDIUM,
        Some("cas-current"),
        recent,
    ));

    let data = data_from_tasks(tasks);
    let (groups, _standalone) = data.tasks_by_epic();

    assert_eq!(groups.len(), 2, "both epics should be present");
    assert_eq!(
        groups[0].epic.id, "cas-current",
        "the recently-active epic must sort first, not the stale 14-subtask epic"
    );
    assert_eq!(groups[1].epic.id, "cas-stale");
}

/// Switching focus (touching a different epic's subtask) makes the panel follow
/// within a refresh: the newly-touched epic moves to the top.
#[test]
fn test_tasks_by_epic_follows_focus_switch() {
    let t0 = chrono::Utc::now() - chrono::Duration::hours(2);
    let t1 = chrono::Utc::now() - chrono::Duration::hours(1);
    let t2 = chrono::Utc::now();

    // Epic A touched at t1, epic B touched more recently at t2 -> B first.
    let tasks = vec![
        summary_at(
            "cas-a",
            TaskStatus::Open,
            TaskType::Epic,
            Priority::HIGH,
            None,
            t0,
        ),
        summary_at(
            "cas-a-0",
            TaskStatus::Open,
            TaskType::Task,
            Priority::HIGH,
            Some("cas-a"),
            t1,
        ),
        summary_at(
            "cas-b",
            TaskStatus::Open,
            TaskType::Epic,
            Priority::HIGH,
            None,
            t0,
        ),
        summary_at(
            "cas-b-0",
            TaskStatus::Open,
            TaskType::Task,
            Priority::HIGH,
            Some("cas-b"),
            t2,
        ),
    ];

    let (groups, _) = data_from_tasks(tasks).tasks_by_epic();
    assert_eq!(
        groups[0].epic.id, "cas-b",
        "most-recently-touched epic leads"
    );
    assert_eq!(groups[1].epic.id, "cas-a");
}

/// No regression for a single-epic session: ordering is trivially stable and the
/// epic + its subtasks are returned intact.
#[test]
fn test_tasks_by_epic_single_epic_unchanged() {
    let now = chrono::Utc::now();
    let tasks = vec![
        summary_at(
            "cas-only",
            TaskStatus::InProgress,
            TaskType::Epic,
            Priority::HIGH,
            None,
            now,
        ),
        summary_at(
            "cas-only-0",
            TaskStatus::InProgress,
            TaskType::Task,
            Priority::HIGH,
            Some("cas-only"),
            now,
        ),
        summary_at(
            "cas-only-1",
            TaskStatus::Open,
            TaskType::Task,
            Priority::MEDIUM,
            Some("cas-only"),
            now,
        ),
    ];

    let (groups, standalone) = data_from_tasks(tasks).tasks_by_epic();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].epic.id, "cas-only");
    assert_eq!(groups[0].subtasks.len(), 2);
    assert!(groups[0].has_active);
    assert!(standalone.is_empty());
}

// =============================================================================
// TaskSummary Tests
// =============================================================================

#[test]
fn test_task_summary_fields() {
    let summary = TaskSummary {
        id: "cas-1234".to_string(),
        title: "Test Task".to_string(),
        status: TaskStatus::InProgress,
        priority: Priority::HIGH,
        assignee: Some("agent-1".to_string()),
        task_type: TaskType::Feature,
        epic: Some("cas-epic".to_string()),
        branch: Some("feature/test".to_string()),
        updated_at: None,
            epic_verification_owner: None,
        };

    assert_eq!(summary.id, "cas-1234");
    assert_eq!(summary.title, "Test Task");
    assert_eq!(summary.status, TaskStatus::InProgress);
    assert_eq!(summary.priority, Priority::HIGH);
    assert_eq!(summary.assignee, Some("agent-1".to_string()));
    assert_eq!(summary.task_type, TaskType::Feature);
    assert_eq!(summary.epic, Some("cas-epic".to_string()));
    assert_eq!(summary.branch, Some("feature/test".to_string()));
}

// =============================================================================
// AgentSummary Tests
// =============================================================================

#[test]
fn test_agent_summary_fields() {
    let now = chrono::Utc::now();
    let summary = AgentSummary {
        id: "agent-123".to_string(),
        name: "swift-fox".to_string(),
        status: AgentStatus::Active,
        registered_at: now,
        current_task: Some("cas-1234".to_string()),
        latest_activity: Some(("Edited file".to_string(), now)),
        last_heartbeat: Some(now),
        pending_messages: 0,
        pending_supervisor_messages: 0,
        latest_supervisor_message_at: None,
        active_lease: None,
        effort: None,
    };

    assert_eq!(summary.id, "agent-123");
    assert_eq!(summary.name, "swift-fox");
    assert_eq!(summary.status, AgentStatus::Active);
    assert_eq!(summary.current_task, Some("cas-1234".to_string()));
    assert!(summary.latest_activity.is_some());
    assert!(summary.last_heartbeat.is_some());
}

// =============================================================================
// EpicGroup Tests
// =============================================================================

#[test]
fn test_epic_group_fields() {
    let epic = TaskSummary {
        id: "cas-epic".to_string(),
        title: "Test Epic".to_string(),
        status: TaskStatus::InProgress,
        priority: Priority::CRITICAL,
        assignee: None,
        task_type: TaskType::Epic,
        epic: None,
        branch: Some("epic/test".to_string()),
        updated_at: None,
            epic_verification_owner: None,
        };

    let subtask = TaskSummary {
        id: "cas-0001".to_string(),
        title: "Subtask 1".to_string(),
        status: TaskStatus::InProgress,
        priority: Priority::HIGH,
        assignee: Some("agent-1".to_string()),
        task_type: TaskType::Task,
        epic: Some("cas-epic".to_string()),
        branch: None,
        updated_at: None,
            epic_verification_owner: None,
        };

    let group = EpicGroup {
        epic,
        subtasks: vec![subtask],
        has_active: true,
    };

    assert_eq!(group.epic.id, "cas-epic");
    assert_eq!(group.subtasks.len(), 1);
    assert!(group.has_active);
}
