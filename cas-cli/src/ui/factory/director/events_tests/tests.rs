use crate::ui::factory::director::data::{ActiveLeaseSummary, AgentSummary, DirectorStores};
use crate::ui::factory::director::events::*;
use cas_store::{AgentStore, EventStore, PendingReason, PromptQueueStore, TaskStore};
use cas_types::{Agent, AgentRole, AgentStatus, TaskType};

fn make_task(id: &str, title: &str, status: TaskStatus, assignee: Option<&str>) -> TaskSummary {
    TaskSummary {
        id: id.to_string(),
        title: title.to_string(),
        status,
        priority: cas_types::Priority::MEDIUM,
        assignee: assignee.map(String::from),
        task_type: TaskType::Task,
        epic: None,
        branch: None,
        updated_at: None,
        epic_verification_owner: None,
    }
}

fn make_epic(id: &str, title: &str, status: TaskStatus) -> TaskSummary {
    TaskSummary {
        id: id.to_string(),
        title: title.to_string(),
        status,
        priority: cas_types::Priority::HIGH,
        assignee: None,
        task_type: TaskType::Epic,
        epic: None,
        branch: None,
        updated_at: None,
        epic_verification_owner: None,
    }
}

fn make_epic_with_branch(id: &str, title: &str, status: TaskStatus, branch: &str) -> TaskSummary {
    TaskSummary {
        id: id.to_string(),
        title: title.to_string(),
        status,
        priority: cas_types::Priority::HIGH,
        assignee: None,
        task_type: TaskType::Epic,
        epic: None,
        branch: Some(branch.to_string()),
        updated_at: None,
        epic_verification_owner: None,
    }
}

fn make_agent(id: &str, name: &str, current_task: Option<&str>) -> AgentSummary {
    AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        status: AgentStatus::Active,
        registered_at: chrono::Utc::now(),
        current_task: current_task.map(String::from),
        latest_activity: None,
        last_heartbeat: Some(chrono::Utc::now()),
        pending_messages: 0,
        pending_supervisor_messages: 0,
        latest_supervisor_message_at: None,
        active_lease: None,
        effort: None,
    }
}

#[test]
fn test_detect_task_assigned() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: task unassigned
    let data1 = DirectorData {
        ready_tasks: vec![make_task("task-1", "Test Task", TaskStatus::Open, None)],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // New state: task assigned to swift-fox
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![make_task(
            "task-1",
            "Test Task",
            TaskStatus::InProgress,
            Some("agent-1"),
        )],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", Some("task-1"))],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    assert!(events.iter().any(|e| matches!(
        e,
        DirectorEvent::TaskAssigned { task_id, worker, .. }
            if task_id == "task-1" && worker == "swift-fox"
    )));
}

#[test]
fn test_detect_task_completed() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: task in progress
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![make_task(
            "task-1",
            "Test Task",
            TaskStatus::InProgress,
            Some("agent-1"),
        )],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", Some("task-1"))],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // New state: task completed (no longer in in_progress_tasks)
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    assert!(events.iter().any(|e| matches!(
        e,
        DirectorEvent::TaskCompleted { task_id, worker, .. }
            if task_id == "task-1" && worker == "swift-fox"
    )));
}

/// Helper: build an idle-agent snapshot with a single factory worker.
fn idle_data_for(agent_id: &str, agent_name: &str) -> DirectorData {
    DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent(agent_id, agent_name, None)],
        activity: vec![],
        agent_id_to_name: [(agent_id.to_string(), agent_name.to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    }
}

/// Helper: build a working-agent snapshot with one task assigned to a single factory worker.
fn working_data_for(
    agent_id: &str,
    agent_name: &str,
    task_id: &str,
    task_title: &str,
) -> DirectorData {
    DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![make_task(
            task_id,
            task_title,
            TaskStatus::InProgress,
            Some(agent_id),
        )],
        epic_tasks: vec![],
        agents: vec![make_agent(agent_id, agent_name, Some(task_id))],
        activity: vec![],
        agent_id_to_name: [(agent_id.to_string(), agent_name.to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    }
}

#[test]
fn test_detect_worker_idle() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: worker has task.
    detector.initialize(&working_data_for(
        "agent-1",
        "swift-fox",
        "task-1",
        "Test Task",
    ));

    // Tick 1 of sustained idle — must NOT emit (debounce threshold is 2 ticks).
    let idle = idle_data_for("agent-1", "swift-fox");
    let events_tick1 = detector.detect_changes(&idle, None);
    assert!(
        !events_tick1
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. })),
        "WorkerIdle must not fire after a single idle tick — debouncing prevents spurious \
         idle prompts during close-X → start-Y transitions"
    );

    // Tick 2 of sustained idle — now emit.
    let events_tick2 = detector.detect_changes(&idle, None);
    assert!(
        events_tick2.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerIdle { worker, .. } if worker == "swift-fox"
        )),
        "WorkerIdle must fire once the agent has been idle for the consecutive-tick threshold"
    );

    // Tick 3 of sustained idle — already emitted, do not re-emit every tick.
    let events_tick3 = detector.detect_changes(&idle, None);
    assert!(
        !events_tick3
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. })),
        "WorkerIdle must not re-fire on every tick of a sustained idle streak"
    );
}

#[test]
fn test_just_registered_worker_is_suppressed_during_spawn_assignment_window() {
    let registered_at = chrono::Utc::now();
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());
    let mut data = idle_data_for("agent-1", "swift-fox");
    data.agents[0].registered_at = registered_at;
    data.agents[0].last_heartbeat = None;
    detector.initialize(&data);

    let mut saw_idle = false;
    for offset_secs in [2, 4, 6, 8] {
        saw_idle |= detector
            .detect_changes_at(
                &data,
                None,
                registered_at + chrono::Duration::seconds(offset_secs),
            )
            .iter()
            .any(|event| matches!(event, DirectorEvent::WorkerIdle { .. }));
    }

    assert!(
        !saw_idle,
        "a newly registered worker is expected to be taskless during spawn -> assignment"
    );
}

#[test]
fn test_finished_worker_still_emits_once_after_spawn_grace() {
    let registered_at = chrono::Utc::now() - chrono::Duration::minutes(5);
    let now = chrono::Utc::now();
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());
    let mut working = working_data_for("agent-1", "swift-fox", "task-1", "Finished task");
    working.agents[0].registered_at = registered_at;
    detector.initialize(&working);

    let mut idle = idle_data_for("agent-1", "swift-fox");
    idle.agents[0].registered_at = registered_at;
    idle.agents[0].last_heartbeat = None;
    let first = detector.detect_changes_at(&idle, None, now);
    let second = detector.detect_changes_at(&idle, None, now + chrono::Duration::seconds(2));
    let third = detector.detect_changes_at(&idle, None, now + chrono::Duration::seconds(4));

    assert!(!first.iter().any(|event| matches!(event, DirectorEvent::WorkerIdle { .. })));
    assert_eq!(
        second
            .iter()
            .filter(|event| matches!(event, DirectorEvent::WorkerIdle { .. }))
            .count(),
        1,
        "a worker that finished real work must remain actionable after the spawn grace"
    );
    assert!(
        !third.iter().any(|event| matches!(event, DirectorEvent::WorkerIdle { .. })),
        "the actionable idle signal must still emit exactly once per idle streak"
    );
}

#[test]
fn test_worker_idle_payload_includes_close_rejected_task_state() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    let mut agent = make_agent("agent-1", "swift-fox", None);
    agent.last_heartbeat = None;
    agent.active_lease = Some(ActiveLeaseSummary {
        task_id: "cas-1234".to_string(),
        task_title: "Fix close gate".to_string(),
        task_status: TaskStatus::InProgress,
        close_rejected_reason: Some("MERGE REQUIRED".to_string()),
    });

    let data = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![agent],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data);

    let _ = detector.detect_changes(&data, None);
    let events = detector.detect_changes(&data, None);
    let idle = events
        .iter()
        .find(|event| matches!(event, DirectorEvent::WorkerIdle { .. }))
        .expect("sustained idle should emit WorkerIdle");
    let payload = idle.to_json();

    assert_eq!(payload["worker"], "swift-fox");
    assert_eq!(payload["task_id"], "cas-1234");
    assert_eq!(payload["task_state"], "in_progress");
    assert_eq!(payload["close_rejected"], true);
    assert_eq!(payload["close_rejected_reason"], "MERGE REQUIRED");
    assert!(idle.description().contains("close rejected"));
    assert!(!idle.description().to_lowercase().contains("finished"));
}

/// cas-627f/cas-728b: a MERGE REQUIRED close parks the task as
/// `AwaitingMerge` and releases the worker's lease (cas-8d5b). The worker
/// then sits genuinely idle — `current_task` is `None` (parking doesn't set
/// `InProgress`) for as long as the task stays parked. cas-627f made
/// `active_lease` resolve the parked task again (fallback by assignee +
/// status), so `WorkerIdle` carries the close-rejected payload once the
/// idle streak crosses `IDLE_CONSECUTIVE_TICKS` — same as the InProgress
/// case above. What this test adds: proving `idle_already_emitted` already
/// debounces this to ONE notification per sustained idle streak, not one
/// per 2s tick, WITHOUT any additional Blocked/AwaitingMerge-specific
/// suppression. A prior version of cas-728b's Fix 5 unconditionally
/// suppressed this notification for parked states — that re-hid the exact
/// notification cas-627f fixed; this test locks in the debounce-without-
/// silencing behavior that replaced it.
#[test]
fn test_worker_idle_awaiting_merge_close_rejected_debounces_after_first_fire() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    let mut agent = make_agent("agent-1", "swift-fox", None);
    agent.last_heartbeat = None;
    agent.active_lease = Some(ActiveLeaseSummary {
        task_id: "cas-1234".to_string(),
        task_title: "Fix close gate".to_string(),
        task_status: TaskStatus::AwaitingMerge,
        close_rejected_reason: Some("MERGE REQUIRED".to_string()),
    });

    let data = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![agent],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data);

    let tick1 = detector.detect_changes(&data, None);
    assert!(
        !tick1
            .iter()
            .any(|event| matches!(event, DirectorEvent::WorkerIdle { .. })),
        "IDLE_CONSECUTIVE_TICKS == 2, so tick-1 must still be suppressed: {tick1:?}"
    );

    let tick2 = detector.detect_changes(&data, None);
    let idle = tick2
        .iter()
        .find(|event| matches!(event, DirectorEvent::WorkerIdle { .. }))
        .expect("sustained idle streak should emit WorkerIdle once threshold is crossed");
    let payload = idle.to_json();
    assert_eq!(payload["task_id"], "cas-1234");
    assert_eq!(payload["task_state"], "awaiting_merge");
    assert_eq!(payload["close_rejected_reason"], "MERGE REQUIRED");

    // Same sustained idle streak, task still parked, nothing changed: ticks
    // 3-5 must NOT re-emit WorkerIdle. This is the debounce-after-first-fire
    // guarantee — `idle_already_emitted` stays set until `current_task`
    // becomes `Some` again (worker picks up new work), not merely a
    // few-tick cooldown.
    for _ in 0..3 {
        let repeat = detector.detect_changes(&data, None);
        assert!(
            !repeat
                .iter()
                .any(|event| matches!(event, DirectorEvent::WorkerIdle { .. })),
            "repeated ticks within the same idle streak must not re-fire WorkerIdle \
             for a still-parked task: {repeat:?}"
        );
    }
}

/// Regression test for cas-f9e8.
///
/// Reproduces the close-X → start-Y race that caused spurious "Worker X is
/// idle" prompts: a worker finishes one task and immediately claims another,
/// but a single director refresh lands inside the sub-second window where
/// `agent_tasks[worker] = None`. Before the fix, the old transition-based
/// detector emitted `WorkerIdle` as soon as it observed that None, delivering
/// a stale "idle" prompt to the supervisor after the worker had already
/// resumed work. The consecutive-tick debounce must suppress this.
#[test]
fn test_no_worker_idle_on_transient_close_then_start() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initialize: worker is working on task-1.
    detector.initialize(&working_data_for(
        "agent-1",
        "swift-fox",
        "task-1",
        "First task",
    ));

    // Transient idle: one refresh tick catches the close-X → start-Y gap.
    let events_transient = detector.detect_changes(&idle_data_for("agent-1", "swift-fox"), None);
    assert!(
        !events_transient
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. })),
        "WorkerIdle must not fire from a single transient idle tick — this is exactly the \
         close-X → start-Y race described in cas-f9e8"
    );

    // Next refresh: worker has claimed task-2. Idle streak should be reset.
    let events_claimed = detector.detect_changes(
        &working_data_for("agent-1", "swift-fox", "task-2", "Second task"),
        None,
    );
    assert!(
        !events_claimed
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. })),
        "WorkerIdle must not fire once the worker has resumed work on a new task"
    );

    // Sanity: a subsequent sustained idle still emits after the threshold,
    // so the reset didn't break normal idle detection.
    let idle = idle_data_for("agent-1", "swift-fox");
    let _ = detector.detect_changes(&idle, None); // tick 1
    let events_tick2 = detector.detect_changes(&idle, None); // tick 2
    assert!(
        events_tick2.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerIdle { worker, .. } if worker == "swift-fox"
        )),
        "Sustained idle after a reset must still emit WorkerIdle once the threshold is met"
    );
}

#[test]
fn test_ignore_non_factory_agents() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state
    let data1 = DirectorData {
        ready_tasks: vec![make_task("task-1", "Test Task", TaskStatus::Open, None)],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // New state: task assigned to agent not in factory
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![make_task(
            "task-1",
            "Test Task",
            TaskStatus::InProgress,
            Some("other-agent"),
        )],
        epic_tasks: vec![],
        agents: vec![make_agent("other-agent", "other-agent", Some("task-1"))],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    // Should not detect assignment since "other-agent" is not in factory
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DirectorEvent::TaskAssigned { .. }))
    );
}

#[test]
fn test_debouncing() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: task unassigned
    let data1 = DirectorData {
        ready_tasks: vec![make_task("task-1", "Test Task", TaskStatus::Open, None)],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // First assignment - should emit event
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![make_task(
            "task-1",
            "Test Task",
            TaskStatus::InProgress,
            Some("agent-1"),
        )],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", Some("task-1"))],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events1 = detector.detect_changes(&data2, None);
    assert_eq!(events1.len(), 1);
    assert!(matches!(
        &events1[0],
        DirectorEvent::TaskAssigned { task_id, .. } if task_id == "task-1"
    ));

    // Re-initialize and try again immediately - should be debounced
    detector.last_state = DirectorState::from_data(&data1);
    let events2 = detector.detect_changes(&data2, None);
    assert!(
        events2.is_empty(),
        "Expected debounced event, but got {events2:?}"
    );
}

#[test]
fn test_debounce_key_uniqueness() {
    // Different event types should have different keys
    let assigned = DirectorEvent::TaskAssigned {
        task_id: "task-1".to_string(),
        task_title: "Title".to_string(),
        worker: "worker-1".to_string(),
    };
    let completed = DirectorEvent::TaskCompleted {
        task_id: "task-1".to_string(),
        task_title: "Title".to_string(),
        worker: "worker-1".to_string(),
    };

    assert_ne!(assigned.debounce_key(), completed.debounce_key());

    // Same event type with different tasks should have different keys
    let assigned2 = DirectorEvent::TaskAssigned {
        task_id: "task-2".to_string(),
        task_title: "Title".to_string(),
        worker: "worker-1".to_string(),
    };

    assert_ne!(assigned.debounce_key(), assigned2.debounce_key());

    // Same event type with same task but different worker should have different keys
    let assigned3 = DirectorEvent::TaskAssigned {
        task_id: "task-1".to_string(),
        task_title: "Title".to_string(),
        worker: "worker-2".to_string(),
    };

    assert_ne!(assigned.debounce_key(), assigned3.debounce_key());
}

#[test]
fn test_detect_epic_started() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: epic is open
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![make_epic("epic-1", "Test Epic", TaskStatus::Open)],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // New state: epic started (in progress)
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![make_epic("epic-1", "Test Epic", TaskStatus::InProgress)],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    assert!(events.iter().any(|e| matches!(
        e,
        DirectorEvent::EpicStarted { epic_id, epic_title }
            if epic_id == "epic-1" && epic_title == "Test Epic"
    )));
}

#[test]
fn test_detect_epic_completed() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: epic is in progress
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![make_epic("epic-1", "Test Epic", TaskStatus::InProgress)],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // New state: epic completed
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![make_epic("epic-1", "Test Epic", TaskStatus::Closed)],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    assert!(events.iter().any(|e| matches!(
        e,
        DirectorEvent::EpicCompleted { epic_id } if epic_id == "epic-1"
    )));
}

#[test]
fn test_no_epic_event_when_unchanged() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: epic is in progress
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![make_epic("epic-1", "Test Epic", TaskStatus::InProgress)],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // Same state: epic still in progress
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![make_epic("epic-1", "Test Epic", TaskStatus::InProgress)],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    // No epic events should be emitted
    assert!(!events.iter().any(|e| matches!(
        e,
        DirectorEvent::EpicStarted { .. } | DirectorEvent::EpicCompleted { .. }
    )));
}

#[test]
fn test_idle_events_suppressed_for_removed_workers() {
    let mut detector = DirectorEventDetector::new(
        vec!["swift-fox".to_string(), "calm-owl".to_string()],
        "supervisor".to_string(),
    );

    // Initial state: both workers have tasks
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![
            make_task("task-1", "Task 1", TaskStatus::InProgress, Some("agent-1")),
            make_task("task-2", "Task 2", TaskStatus::InProgress, Some("agent-2")),
        ],
        epic_tasks: vec![],
        agents: vec![
            make_agent("agent-1", "swift-fox", Some("task-1")),
            make_agent("agent-2", "calm-owl", Some("task-2")),
        ],
        activity: vec![],
        agent_id_to_name: [
            ("agent-1".to_string(), "swift-fox".to_string()),
            ("agent-2".to_string(), "calm-owl".to_string()),
        ]
        .into_iter()
        .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // Shut down swift-fox
    detector.remove_worker("swift-fox");

    // New state: both workers idle (swift-fox's agent might still linger in data)
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![
            make_agent("agent-1", "swift-fox", None),
            make_agent("agent-2", "calm-owl", None),
        ],
        activity: vec![],
        agent_id_to_name: [
            ("agent-1".to_string(), "swift-fox".to_string()),
            ("agent-2".to_string(), "calm-owl".to_string()),
        ]
        .into_iter()
        .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    // Two idle ticks are required to cross the consecutive-tick debounce.
    let _ = detector.detect_changes(&data2, None);
    let events = detector.detect_changes(&data2, None);

    // calm-owl idle event should be emitted
    assert!(
        events.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerIdle { worker, .. } if worker == "calm-owl"
        )),
        "Expected idle event for calm-owl"
    );

    // swift-fox idle event should be suppressed (removed worker)
    assert!(
        !events.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerIdle { worker, .. } if worker == "swift-fox"
        )),
        "Expected no idle event for removed worker swift-fox"
    );
}

#[test]
fn test_idle_rate_limit_longer_than_general_debounce() {
    use std::time::Duration;

    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: worker has task
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![make_task(
            "task-1",
            "Test Task",
            TaskStatus::InProgress,
            Some("agent-1"),
        )],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", Some("task-1"))],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // Worker goes idle - first event should emit
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    // Two idle ticks are required to cross the consecutive-tick debounce.
    let _ = detector.detect_changes(&data2, None);
    let events = detector.detect_changes(&data2, None);
    assert!(
        events.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerIdle { worker, .. } if worker == "swift-fox"
        )),
        "First idle event should emit"
    );

    // Simulate: worker gets task and goes idle again after 60 seconds
    // (past the 30s general debounce but within the 5-minute idle rate limit).
    // We drive this through detect_changes rather than poking last_state directly
    // so the consecutive-idle counters reset the way they would in production.
    let _ = detector.detect_changes(&data1, None);

    // Manually advance the idle debounce time to 60s ago (past 30s general debounce)
    let key = "idle:swift-fox".to_string();
    if let Some(time) = detector.last_prompt_times.get_mut(&key) {
        *time = std::time::Instant::now() - Duration::from_secs(60);
    }

    let events2 = detector.detect_changes(&data2, None);
    assert!(
        !events2.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerIdle { worker, .. } if worker == "swift-fox"
        )),
        "Idle event should be rate-limited (within 5-minute window)"
    );
}

#[test]
fn test_detect_epic_started_open_with_branch() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: no epics
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // New state: an Open epic with a branch appears (auto-created by supervisor)
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![make_epic_with_branch(
            "epic-1",
            "New Epic",
            TaskStatus::Open,
            "epic/new-epic",
        )],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    assert!(
        events.iter().any(|e| matches!(
            e,
            DirectorEvent::EpicStarted { epic_id, epic_title }
                if epic_id == "epic-1" && epic_title == "New Epic"
        )),
        "Open-with-branch epic should fire EpicStarted"
    );
}

#[test]
fn test_no_duplicate_epic_started_for_existing_open_with_branch() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: already has an Open epic with branch
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![make_epic_with_branch(
            "epic-1",
            "Existing Epic",
            TaskStatus::Open,
            "epic/existing",
        )],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // Same state: epic still Open with branch
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![make_epic_with_branch(
            "epic-1",
            "Existing Epic",
            TaskStatus::Open,
            "epic/existing",
        )],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DirectorEvent::EpicStarted { .. })),
        "Should not fire EpicStarted for already-tracked Open-with-branch epic"
    );
}

#[test]
fn test_in_progress_epic_takes_priority_over_open_with_branch() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial state: no epics
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // Both an Open-with-branch and an InProgress epic appear
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![
            make_epic_with_branch("epic-open", "Open Epic", TaskStatus::Open, "epic/open"),
            make_epic("epic-active", "Active Epic", TaskStatus::InProgress),
        ],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: HashMap::new(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    // InProgress should win
    assert!(
        events.iter().any(|e| matches!(
            e,
            DirectorEvent::EpicStarted { epic_id, .. } if epic_id == "epic-active"
        )),
        "InProgress epic should take priority over Open-with-branch"
    );
}

// ---------------------------------------------------------------------------
// cas-177f: terminal-status guard on TaskAssigned dispatch
// ---------------------------------------------------------------------------

/// A recently-closed task must not produce a fresh TaskAssigned event, even
/// when an idle worker happens to appear on the same refresh tick. This is
/// the exact shape of the cas-177f repro — solid-jay-17 closed cas-953d and
/// kept getting re-dispatched.
#[test]
fn test_closed_task_not_redispatched_to_idle_worker() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial: task in progress for swift-fox
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![make_task(
            "task-1",
            "Closed Task",
            TaskStatus::InProgress,
            Some("agent-1"),
        )],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", Some("task-1"))],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // Worker closes task → disappears from active sets, worker goes idle
    let data2 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    // TaskCompleted is fine and expected; TaskAssigned for task-1 must NOT fire
    assert!(
        !events.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskAssigned { task_id, .. } if task_id == "task-1"
        )),
        "Closed task must not produce a TaskAssigned event: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskCompleted { task_id, .. } if task_id == "task-1"
        )),
        "Expected TaskCompleted when in-progress task disappears"
    );
}

/// Defensive guard: if a Closed task somehow leaks into `ready_tasks` (e.g.
/// future refactor of the data-loading filter in
/// `crates/cas-factory/src/director.rs`), `detect_changes` must still refuse
/// to fire TaskAssigned for it. This is the "belt-and-suspenders" scenario
/// the cas-177f acceptance criteria asks for.
#[test]
fn test_closed_task_leaked_into_ready_tasks_not_dispatched() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial: empty
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // Synthetic: closed task with an assignee pushed into ready_tasks. In the
    // current code path this can't happen because the data loader filters by
    // status, but the events module must not rely on that invariant.
    let data2 = DirectorData {
        ready_tasks: vec![make_task(
            "task-1",
            "Leaked Closed Task",
            TaskStatus::Closed,
            Some("agent-1"),
        )],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    assert!(
        !events.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskAssigned { task_id, .. } if task_id == "task-1"
        )),
        "Closed task in ready_tasks must not produce TaskAssigned: {events:?}"
    );
}

/// A Blocked task (which currently shares the `ready_tasks` bucket with Open
/// per `crates/cas-factory/src/director.rs:228`) must not be dispatched to a
/// worker. This extends the older `bugfix_director_dispatches_blocked_tasks`
/// memory by pinning the behavior in the events module.
#[test]
fn test_blocked_task_not_dispatched() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // Supervisor sets a Blocked task's assignee to swift-fox. The data loader
    // puts blocked tasks into `ready_tasks`, which is how they reach the
    // detector today.
    let data2 = DirectorData {
        ready_tasks: vec![make_task(
            "task-1",
            "Blocked Task",
            TaskStatus::Blocked,
            Some("agent-1"),
        )],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    // TaskBlocked is expected (separate dispatch concern, routed to
    // supervisor not worker); TaskAssigned must NOT fire.
    assert!(
        !events.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskAssigned { task_id, .. } if task_id == "task-1"
        )),
        "Blocked task must not produce TaskAssigned: {events:?}"
    );
}

/// Regression test for cas-afb7: spawn race where `WorkerIdle` fires before
/// the prompt queue is drained on first poll.
///
/// A freshly spawned worker appears task-less before it has polled its first
/// assignment from the prompt queue. The idle detector must not emit
/// `WorkerIdle` as long as `pending_messages > 0`. Once the queue is drained
/// (pending_messages == 0), normal debounce-threshold idle detection resumes.
#[test]
fn test_no_worker_idle_while_pending_messages_in_queue() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Fresh worker: no task, one pending message (task assignment queued).
    let data_with_pending = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![AgentSummary {
            id: "agent-1".to_string(),
            name: "swift-fox".to_string(),
            status: AgentStatus::Active,
            registered_at: chrono::Utc::now(),
            current_task: None,
            latest_activity: None,
            last_heartbeat: Some(chrono::Utc::now()),
            pending_messages: 1,
            pending_supervisor_messages: 0,
            latest_supervisor_message_at: None,
            active_lease: None,
            effort: None,
        }],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data_with_pending);

    // Two consecutive ticks with pending_messages == 1: must NOT emit WorkerIdle.
    let events_tick1 = detector.detect_changes(&data_with_pending, None);
    let events_tick2 = detector.detect_changes(&data_with_pending, None);
    assert!(
        !events_tick1
            .iter()
            .chain(events_tick2.iter())
            .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. })),
        "WorkerIdle must not fire while the worker has unread prompt-queue messages (spawn race)"
    );

    // Once the queue drains (pending_messages == 0), idle detection resumes.
    // IDLE_CONSECUTIVE_TICKS == 2, so tick-1 must still be suppressed.
    let idle = idle_data_for("agent-1", "swift-fox");
    let events_after_drain_tick1 = detector.detect_changes(&idle, None);
    assert!(
        !events_after_drain_tick1
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. })),
        "First idle tick after queue drained must not fire (debounce threshold not yet reached)"
    );
    // Tick-2 crosses the threshold: WorkerIdle must now fire.
    let events_after_drain_tick2 = detector.detect_changes(&idle, None);
    assert!(
        events_after_drain_tick2.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerIdle { worker, .. } if worker == "swift-fox"
        )),
        "WorkerIdle must fire once pending messages are gone and idle threshold is met"
    );
}

#[test]
fn long_backoff_supervisor_message_does_not_permanently_suppress_worker_idle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let stores = DirectorStores::open(temp_dir.path()).unwrap();
    stores.task_store.init().unwrap();
    stores.agent_store.init().unwrap();
    stores.event_store.init().unwrap();
    let prompt_queue = stores.prompt_queue_store.as_ref().unwrap();
    prompt_queue.init().unwrap();

    let supervisor = Agent::new_with_role(
        "supervisor-session".to_string(),
        "supervisor".to_string(),
        AgentRole::Supervisor,
    );
    stores.agent_store.register(&supervisor).unwrap();
    let worker = Agent::new_with_role(
        "worker-session".to_string(),
        "backed-off-worker".to_string(),
        AgentRole::Worker,
    );
    stores.agent_store.register(&worker).unwrap();

    let prompt_id = prompt_queue
        .enqueue("supervisor", "backed-off-worker", "assigned work")
        .unwrap();
    let mut retry_at = chrono::Utc::now();
    for _ in 0..6 {
        let disposition = prompt_queue
            .record_retry(
                prompt_id,
                PendingReason::TargetUnavailable,
                Some("pane unavailable"),
            )
            .unwrap();
        let cas_store::PromptRetryDisposition::Scheduled {
            retry_at: scheduled_at,
            ..
        } = disposition
        else {
            panic!("six failed deliveries must remain retryable");
        };
        retry_at = scheduled_at;
    }
    assert!(
        retry_at > chrono::Utc::now() + chrono::Duration::seconds(4),
        "fixture must reach the 5s retry ceiling"
    );

    let data = DirectorData::load_with_stores(temp_dir.path(), None, false, Some(&stores)).unwrap();
    let worker_summary = data
        .agents
        .iter()
        .find(|agent| agent.id == "worker-session")
        .expect("worker is present in director data");
    assert_eq!(
        worker_summary.pending_messages, 0,
        "a row beyond the idle horizon must not look immediately deliverable"
    );
    assert_eq!(worker_summary.pending_supervisor_messages, 0);

    let mut detector = DirectorEventDetector::new(
        vec!["backed-off-worker".to_string()],
        "supervisor".to_string(),
    );
    detector.initialize(&data);
    let clock = Instant::now();
    let utc = chrono::Utc::now() + chrono::Duration::seconds(1);

    let tick_1 = detector.detect_changes_at(&data, None, clock, utc);
    assert!(
        !tick_1
            .iter()
            .any(|event| matches!(event, DirectorEvent::WorkerIdle { .. }))
    );
    let tick_2 = detector.detect_changes_at(&data, None, clock + Duration::from_secs(2), utc);
    assert!(tick_2.iter().any(|event| matches!(
        event,
        DirectorEvent::WorkerIdle { worker, .. } if worker == "backed-off-worker"
    )));
}

#[test]
fn delivered_supervisor_message_keeps_current_idle_streak_suppressed() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());
    detector.initialize(&working_data_for(
        "agent-1",
        "swift-fox",
        "task-1",
        "Work Item",
    ));

    let clock = Instant::now();
    let utc = chrono::Utc::now();
    let idle = idle_data_for("agent-1", "swift-fox");
    detector.detect_changes_at(&idle, None, clock, utc);

    let mut contacted = idle.clone();
    contacted.agents[0].pending_messages = 1;
    contacted.agents[0].pending_supervisor_messages = 1;
    contacted.agents[0].latest_supervisor_message_at =
        Some(utc + chrono::Duration::seconds(1));
    detector.detect_changes_at(
        &contacted,
        None,
        clock + Duration::from_secs(1),
        utc + chrono::Duration::seconds(1),
    );

    contacted.agents[0].pending_messages = 0;
    contacted.agents[0].pending_supervisor_messages = 0;
    for seconds in 2..8 {
        let events = detector.detect_changes_at(
            &contacted,
            None,
            clock + Duration::from_secs(seconds),
            utc + chrono::Duration::seconds(seconds as i64),
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, DirectorEvent::WorkerIdle { .. })),
            "delivered supervisor contact must handle the current idle streak: {events:?}"
        );
    }

    detector.detect_changes_at(
        &working_data_for("agent-1", "swift-fox", "task-2", "Next Work"),
        None,
        clock + Duration::from_secs(10),
        utc + chrono::Duration::seconds(10),
    );
    let later_idle_tick_1 = detector.detect_changes_at(
        &contacted,
        None,
        clock + Duration::from_secs(20),
        utc + chrono::Duration::seconds(20),
    );
    let later_idle_tick_2 = detector.detect_changes_at(
        &contacted,
        None,
        clock + Duration::from_secs(21),
        utc + chrono::Duration::seconds(21),
    );
    assert!(!later_idle_tick_1
        .iter()
        .any(|event| matches!(event, DirectorEvent::WorkerIdle { .. })));
    assert!(later_idle_tick_2.iter().any(|event| matches!(
        event,
        DirectorEvent::WorkerIdle { worker, .. } if worker == "swift-fox"
    )));
}

/// Covariance with cas-3bd4: a Closed task can still carry a stale
/// `assignee` field (supervisor-close path historically didn't clear it).
/// If that task ever reaches the event detector — via a future refactor of
/// the ready_tasks filter, a stale cache, or a cross-session data race —
/// the terminal-status guard must fire BEFORE the lingering assignee is
/// reinterpreted as an active assignment. Explicitly pin that behavior.
#[test]
fn test_closed_task_with_stale_assignee_not_redispatched() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial: empty. Detector's last_state has no knowledge of task-1.
    let data1 = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data1);

    // A Closed task is synthesized into ready_tasks with a stale assignee
    // (matches the cas-3bd4 close-path condition). The data loader should
    // never produce this, but the detector must not depend on that —
    // specifically because supervisor-close once left assignee populated
    // on Closed rows. The guard must fire regardless of what last_state
    // looked like before the task appeared.
    let data2 = DirectorData {
        ready_tasks: vec![make_task(
            "task-1",
            "Closed With Stale Assignee",
            TaskStatus::Closed,
            Some("agent-1"),
        )],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };

    let events = detector.detect_changes(&data2, None);

    assert!(
        !events.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskAssigned { task_id, .. } if task_id == "task-1"
        )),
        "Closed task with stale assignee must not produce TaskAssigned: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// cas-55dc: TaskCompleted edge-trigger + state-guard
// ---------------------------------------------------------------------------

/// Regression for cas-55dc facet A: TaskCompleted must fire at most ONCE per
/// genuine completion, never re-fire on active-set churn/oscillation.
///
/// Scenario:
///   1. Task is InProgress → disappears → TaskCompleted emitted.
///   2. Task reappears (lease oscillation / active-set churn).
///   3. Task disappears again → TaskCompleted must NOT re-fire.
#[test]
fn test_task_completed_no_refire_on_oscillation() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial: task is in-progress.
    detector.initialize(&working_data_for(
        "agent-1",
        "swift-fox",
        "task-1",
        "Work Item",
    ));

    // Tick: task disappears (closed / completed). Should emit TaskCompleted once.
    let events1 = detector.detect_changes(&idle_data_for("agent-1", "swift-fox"), None);
    assert!(
        events1.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskCompleted { task_id, .. } if task_id == "task-1"
        )),
        "TaskCompleted must fire when InProgress task leaves active sets: {events1:?}"
    );

    // Tick: task reappears (oscillation — lease re-acquired, task back in InProgress).
    detector.detect_changes(
        &working_data_for("agent-1", "swift-fox", "task-1", "Work Item"),
        None,
    );

    // Tick: task disappears again. Must NOT re-emit TaskCompleted.
    let events3 = detector.detect_changes(&idle_data_for("agent-1", "swift-fox"), None);
    assert!(
        !events3.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskCompleted { task_id, .. } if task_id == "task-1"
        )),
        "TaskCompleted must not re-fire on active-set churn/oscillation: {events3:?}"
    );
}

/// Regression for cas-55dc facet B: the state-guard must block re-emission even
/// after the 30s debounce window expires. Uses an injectable clock so the test
/// does NOT rely on the debounce timer — it isolates the HashSet guard.
///
/// Calls `detect_changes_at` with a synthetic `now` that is 31 s ahead of the
/// first emission, bypassing the debounce window. Without the state-guard the
/// second emission would sneak through; with the guard it must not.
#[test]
fn test_task_completed_state_guard_independent_of_debounce() {
    use std::time::{Duration, Instant};

    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // t=0: task InProgress.
    detector.initialize(&working_data_for(
        "agent-1",
        "swift-fox",
        "task-1",
        "Work Item",
    ));

    let t0 = Instant::now();
    let t0_utc = chrono::Utc::now();

    // t=0: task disappears → first TaskCompleted.
    let events1 =
        detector.detect_changes_at(&idle_data_for("agent-1", "swift-fox"), None, t0, t0_utc);
    assert!(
        events1.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskCompleted { task_id, .. } if task_id == "task-1"
        )),
        "TaskCompleted must fire on first disappearance: {events1:?}"
    );

    // Oscillation: task reappears.
    detector.detect_changes_at(
        &working_data_for("agent-1", "swift-fox", "task-1", "Work Item"),
        None,
        t0 + Duration::from_secs(1),
        t0_utc + chrono::Duration::seconds(1),
    );

    // t+31s: task disappears again. Debounce would allow re-emission (31s > 30s
    // DEBOUNCE_DURATION) but the state-guard must block it.
    let events3 = detector.detect_changes_at(
        &idle_data_for("agent-1", "swift-fox"),
        None,
        t0 + Duration::from_secs(31),
        t0_utc + chrono::Duration::seconds(31),
    );
    assert!(
        !events3.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskCompleted { task_id, .. } if task_id == "task-1"
        )),
        "State-guard must block TaskCompleted re-emission even past debounce window: {events3:?}"
    );
}

/// Regression for cas-55dc: TaskAssigned must also carry an oscillation guard —
/// once announced for (task_id, worker), do not re-announce when the same
/// assignment reappears after transient active-set churn.
#[test]
fn test_task_assigned_no_refire_on_oscillation() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initial: task unassigned.
    let unassigned = DirectorData {
        ready_tasks: vec![make_task("task-1", "Work Item", TaskStatus::Open, None)],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![make_agent("agent-1", "swift-fox", None)],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "swift-fox".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&unassigned);

    // Tick: task assigned to swift-fox → must emit TaskAssigned once.
    let assigned = working_data_for("agent-1", "swift-fox", "task-1", "Work Item");
    let events1 = detector.detect_changes(&assigned, None);
    assert!(
        events1.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskAssigned { task_id, worker, .. }
                if task_id == "task-1" && worker == "swift-fox"
        )),
        "TaskAssigned must fire on first assignment: {events1:?}"
    );

    // Tick: task temporarily disappears from active sets (lease oscillation).
    detector.detect_changes(&idle_data_for("agent-1", "swift-fox"), None);

    // Tick: task reappears with the SAME assignee.
    let events3 = detector.detect_changes(&assigned, None);
    assert!(
        !events3.iter().any(|e| matches!(
            e,
            DirectorEvent::TaskAssigned { task_id, worker, .. }
                if task_id == "task-1" && worker == "swift-fox"
        )),
        "TaskAssigned must not re-fire when same assignment reappears after oscillation: {events3:?}"
    );
}

// ---------------------------------------------------------------------------
// cas-4038: WorkerIdle must not fire for a live, recently-active worker
// ---------------------------------------------------------------------------

/// Build an agent snapshot with `latest_activity` set to `activity_ago_secs` seconds
/// before `base_utc`, and heartbeat `heartbeat_ago_secs` seconds before `base_utc`.
fn make_agent_active(
    id: &str,
    name: &str,
    heartbeat_ago_secs: i64,
    activity_ago_secs: i64,
    base_utc: chrono::DateTime<chrono::Utc>,
) -> AgentSummary {
    use chrono::Duration as CDuration;
    AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        status: AgentStatus::Active,
        registered_at: base_utc,
        current_task: None, // task-less between turns
        latest_activity: Some((
            "tool_call".to_string(),
            base_utc - CDuration::seconds(activity_ago_secs),
        )),
        last_heartbeat: Some(base_utc - CDuration::seconds(heartbeat_ago_secs)),
        pending_messages: 0,
        pending_supervisor_messages: 0,
        latest_supervisor_message_at: None,
        active_lease: None,
        effort: None,
    }
}

fn active_agent_data(agent: AgentSummary) -> DirectorData {
    let id = agent.id.clone();
    let name = agent.name.clone();
    DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![agent],
        activity: vec![],
        agent_id_to_name: [(id, name)].into_iter().collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    }
}

/// A worker with current_task=None but a fresh heartbeat and recent activity
/// must NOT trigger WorkerIdle, even after the consecutive-tick threshold.
/// This is the "between turns / mid-work" scenario from the cas-4038 description
/// (e.g. agile-cobra-92 flagged idle while it had uncommitted work).
#[test]
fn test_no_worker_idle_for_recently_active_worker() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Worker has fresh heartbeat (5s old) and recent activity (30s old). No current_task.
    let agent = make_agent_active("agent-1", "swift-fox", 5, 30, base_utc);
    let data = active_agent_data(agent);

    detector.initialize(&data);

    // Run enough ticks to exceed IDLE_CONSECUTIVE_TICKS with a frozen now_utc
    // (heartbeat and activity still appear fresh from the detector's perspective).
    for _ in 0..5 {
        let events = detector.detect_changes_at(&data, None, t0, base_utc);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. })),
            "WorkerIdle must NOT fire for a worker with fresh heartbeat + recent activity: {events:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// cas-b67d: never nudge supervisor/lead as an idle worker
// ---------------------------------------------------------------------------

/// The supervisor must NEVER appear as the `worker` field in a WorkerIdle
/// event. In cas-fb94 the director sent "Worker fierce-puma-23 is idle…"
/// to the primary agent because is_factory_agent includes the supervisor.
/// After the fix, supervisor agents are silently skipped in idle detection.
#[test]
fn test_supervisor_never_emits_worker_idle() {
    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Both a worker and the supervisor appear in data.agents with no task.
    let data_both_idle = DirectorData {
        ready_tasks: vec![],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![
            make_agent("agent-w", "swift-fox", None),
            make_agent("agent-s", "supervisor", None),
        ],
        activity: vec![],
        agent_id_to_name: [
            ("agent-w".to_string(), "swift-fox".to_string()),
            ("agent-s".to_string(), "supervisor".to_string()),
        ]
        .into_iter()
        .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&data_both_idle);

    // Run enough ticks to cross IDLE_CONSECUTIVE_TICKS for both agents.
    let _ = detector.detect_changes(&data_both_idle, None);
    let events = detector.detect_changes(&data_both_idle, None);

    // Worker may emit WorkerIdle — supervisor must NOT.
    assert!(
        !events.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerIdle { worker, .. } if worker == "supervisor"
        )),
        "Supervisor must never appear as an idle WORKER in WorkerIdle events: {events:?}"
    );
}

/// Once the worker's heartbeat goes stale (no heartbeat for > threshold), the
/// fresh-heartbeat gate is inactive and normal idle detection fires after the
/// consecutive-tick threshold.
#[test]
fn test_worker_idle_fires_when_heartbeat_stale() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Worker initially had a task (to establish the streak-reset baseline).
    detector.initialize(&working_data_for("agent-1", "swift-fox", "task-1", "Work"));

    // Worker now has stale heartbeat (120s old) — beyond the fresh-heartbeat window.
    let agent = make_agent_active("agent-1", "swift-fox", 120, 300, base_utc);
    let data = active_agent_data(agent);

    // now_utc == base_utc, so heartbeat appears 120s old.
    // Tick 1: not yet at threshold.
    let ev1 = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        !ev1.iter()
            .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. })),
        "Tick 1 with stale heartbeat must not fire (still below consecutive threshold): {ev1:?}"
    );

    // Tick 2: crosses IDLE_CONSECUTIVE_TICKS — WorkerIdle must fire.
    let ev2 = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        ev2.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerIdle { worker, .. } if worker == "swift-fox"
        )),
        "WorkerIdle must fire for genuinely-idle worker (stale heartbeat) after threshold: {ev2:?}"
    );
}

// ── cas-c790 regression tests ─────────────────────────────────────────────────

/// cas-c790: WorkerIdle must NEVER fire for the supervisor, even when the
/// supervisor's name has been added to worker_names via `add_worker`.
///
/// The original recurrence (cas-c790 / cas-b67d) was traced to a path where
/// the supervisor name could enter worker_names during resume/reconnect, causing
/// `is_worker_agent_name` to return true for the lead. Two guards now close the
/// gap: `add_worker` rejects the supervisor's own name, and `is_worker_agent_name`
/// explicitly excludes the supervisor even if the list is corrupted.
#[test]
fn test_c790_worker_idle_never_fires_for_supervisor() {
    let supervisor = "team-lead-zen";
    let t0 = std::time::Instant::now();
    let base_utc = chrono::Utc::now();

    // Detector with no workers — single-session run.
    let mut detector = DirectorEventDetector::new(vec![], supervisor.to_string());

    // Attempt to add the supervisor to worker_names via the mutator.
    // add_worker must silently reject this (the guard closes the race window).
    detector.add_worker(supervisor.to_string());

    // Initialize with the supervisor as the sole agent, no task assigned.
    let data = idle_data_for("sup-session-id", supervisor);
    detector.initialize(&data);

    // Run 2 ticks (> IDLE_CONSECUTIVE_TICKS=2 threshold) to give it every
    // opportunity to fire.
    let ev1 = detector.detect_changes_at(&data, None, t0, base_utc);
    let ev2 = detector.detect_changes_at(&data, None, t0, base_utc);

    let fired = ev1
        .iter()
        .chain(ev2.iter())
        .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. }));

    assert!(
        !fired,
        "cas-c790: WorkerIdle must never fire for the supervisor, even after add_worker \
         attempted to add them to the worker list. Events: {ev1:?}, {ev2:?}"
    );
}

/// cas-c790: Legitimate workers must still produce WorkerIdle events — the
/// supervisor guard must not accidentally suppress real workers.
#[test]
fn test_c790_worker_idle_still_fires_for_real_workers() {
    let t0 = std::time::Instant::now();
    let base_utc = chrono::Utc::now();

    let mut detector =
        DirectorEventDetector::new(vec!["swift-fox".to_string()], "supervisor".to_string());

    // Initialize with the worker active and then immediately idle.
    detector.initialize(&working_data_for("agent-1", "swift-fox", "task-1", "Work"));
    let data = idle_data_for("agent-1", "swift-fox");

    // 2 ticks to cross the threshold.
    let ev1 = detector.detect_changes_at(&data, None, t0, base_utc);
    let ev2 = detector.detect_changes_at(&data, None, t0, base_utc);

    let fired = ev1
        .iter()
        .chain(ev2.iter())
        .any(|e| matches!(e, DirectorEvent::WorkerIdle { worker, .. } if worker == "swift-fox"));

    assert!(
        fired,
        "cas-c790: WorkerIdle must still fire for a legitimate worker after the supervisor \
         guard is added. Events: {ev1:?}, {ev2:?}"
    );
}

// ---------------------------------------------------------------------------
// cas-9829: activity-based WorkerStalled detection
// ---------------------------------------------------------------------------

/// Build an agent with an in-progress task, a configurable heartbeat age,
/// and an optional `latest_activity` age (`None` = no activity ever
/// recorded).
fn make_agent_working_stalled(
    id: &str,
    name: &str,
    task_id: &str,
    heartbeat_ago_secs: i64,
    activity_ago_secs: Option<i64>,
    base_utc: chrono::DateTime<chrono::Utc>,
) -> AgentSummary {
    use chrono::Duration as CDuration;
    AgentSummary {
        id: id.to_string(),
        name: name.to_string(),
        status: AgentStatus::Active,
        registered_at: base_utc,
        current_task: Some(task_id.to_string()),
        latest_activity: activity_ago_secs
            .map(|secs| ("tool_call".to_string(), base_utc - CDuration::seconds(secs))),
        last_heartbeat: Some(base_utc - CDuration::seconds(heartbeat_ago_secs)),
        pending_messages: 0,
        pending_supervisor_messages: 0,
        latest_supervisor_message_at: None,
        active_lease: None,
        effort: None,
    }
}

/// Set `effort` on an already-built agent summary (cas-09d0). Kept as a
/// small wrapper rather than adding a parameter to `make_agent_working_stalled`
/// so the 9 existing call sites of that helper don't need touching.
fn with_effort(mut agent: AgentSummary, effort: cas_mux::Effort) -> AgentSummary {
    agent.effort = Some(effort);
    agent
}

fn stalled_data_for(agent: AgentSummary) -> DirectorData {
    let id = agent.id.clone();
    let name = agent.name.clone();
    let task_id = agent.current_task.clone();
    let in_progress_tasks = match &task_id {
        Some(tid) => vec![make_task(
            tid,
            "Stalled task",
            TaskStatus::InProgress,
            Some(&id),
        )],
        None => vec![],
    };
    DirectorData {
        ready_tasks: vec![],
        in_progress_tasks,
        epic_tasks: vec![],
        agents: vec![agent],
        activity: vec![],
        agent_id_to_name: [(id, name)].into_iter().collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    }
}

fn assigned_unstarted_data(
    base_utc: chrono::DateTime<chrono::Utc>,
    assigned_ago_secs: i64,
) -> DirectorData {
    let mut agent =
        make_agent_working_stalled("agent-1", "lively-crow", "cas-unstarted", 5, None, base_utc);
    agent.current_task = None;
    let mut task = make_task(
        "cas-unstarted",
        "Assigned but unstarted",
        TaskStatus::Open,
        Some("lively-crow"),
    );
    task.updated_at = Some(base_utc - chrono::Duration::seconds(assigned_ago_secs));
    DirectorData {
        ready_tasks: vec![task],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![agent],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "lively-crow".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    }
}

#[test]
fn test_78bf_assigned_open_past_threshold_escalates_to_supervisor() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();
    let data = assigned_unstarted_data(base_utc, 310);
    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);

    assert!(
        events.iter().any(|event| matches!(
            event,
            DirectorEvent::WorkerStalled {
                worker,
                task_id,
                elapsed_secs: 310,
                escalate: true,
            } if worker == "lively-crow" && task_id == "cas-unstarted"
        )),
        "a live worker holding assigned Open work past the threshold must escalate: {events:?}"
    );
}

#[test]
fn test_78bf_assigned_open_within_threshold_keeps_dbbb_grace_window() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();
    let data = assigned_unstarted_data(base_utc, 299);
    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, DirectorEvent::WorkerStalled { .. })),
        "the normal just-assigned window must remain suppressed by cas-dbbb: {events:?}"
    );
}

#[test]
fn test_c14e4_stalled_in_progress_wins_over_assigned_open_escalation() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();
    let mut data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-active",
        5,
        Some(310),
        base_utc,
    ));
    let mut assigned_open = make_task(
        "cas-unstarted",
        "Second assigned task",
        TaskStatus::Open,
        Some("lively-crow"),
    );
    assigned_open.updated_at = Some(base_utc - chrono::Duration::seconds(310));
    data.ready_tasks.push(assigned_open);

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);

    assert!(
        events.iter().any(|event| matches!(
            event,
            DirectorEvent::WorkerStalled {
                task_id,
                escalate: false,
                ..
            } if task_id == "cas-active"
        )),
        "the genuine InProgress stall must retain its first-stage nudge: {events:?}"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            DirectorEvent::WorkerStalled { task_id, .. } if task_id == "cas-unstarted"
        )),
        "the lower-priority assigned-Open signal must not shadow the active-task stall: {events:?}"
    );
}

/// A worker with a fresh heartbeat, an in-progress task, and activity older
/// than the stall threshold must fire a non-escalating `WorkerStalled`
/// (auto-nudge) on first detection, per the cas-9829 bug report: heartbeat
/// alone said "healthy" while the worker had produced nothing for 10+
/// minutes.
#[test]
fn test_9829_worker_stalled_fires_auto_nudge_on_first_detection() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,         // fresh heartbeat
        Some(310), // activity 310s ago, past the 300s threshold
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);

    let nudge = events.iter().find(|e| {
        matches!(
            e,
            DirectorEvent::WorkerStalled { worker, task_id, escalate: false, .. }
                if worker == "lively-crow" && task_id == "cas-0b7d"
        )
    });
    assert!(
        nudge.is_some(),
        "expected a non-escalating WorkerStalled auto-nudge on first stall detection: {events:?}"
    );
}

/// A worker still stalled after the auto-nudge must escalate to the
/// supervisor on the next detection — the nudge fires once, not forever.
#[test]
fn test_9829_worker_stalled_escalates_after_nudge() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,
        Some(310),
        base_utc,
    ));
    detector.initialize(&data);

    let ev1 = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        ev1.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled {
                escalate: false,
                ..
            }
        )),
        "first tick must be the non-escalating nudge: {ev1:?}"
    );

    // Still stalled on the next tick (nothing changed) — must escalate, and
    // must NOT re-emit the nudge.
    let ev2 = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        ev2.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled { worker, escalate: true, .. } if worker == "lively-crow"
        )),
        "second tick while still stalled must escalate to the supervisor: {ev2:?}"
    );
    assert!(
        !ev2.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled {
                escalate: false,
                ..
            }
        )),
        "the nudge must not re-fire once already sent: {ev2:?}"
    );
}

/// Once activity resumes (elapsed drops back under the threshold), the
/// stall streak must clear so a future stall re-nudges from scratch instead
/// of staying silently suppressed forever.
#[test]
fn test_9829_worker_stalled_streak_resets_when_activity_resumes() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);

    let stalled = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,
        Some(310),
        base_utc,
    ));
    detector.initialize(&stalled);
    let ev1 = detector.detect_changes_at(&stalled, None, t0, base_utc);
    assert!(
        ev1.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled {
                escalate: false,
                ..
            }
        )),
        "expected the initial nudge: {ev1:?}"
    );

    // Worker resumes activity (e.g. responded to the nudge).
    let active = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,
        Some(10),
        base_utc,
    ));
    let ev2 = detector.detect_changes_at(&active, None, t0, base_utc);
    assert!(
        !ev2.iter()
            .any(|e| matches!(e, DirectorEvent::WorkerStalled { .. })),
        "resumed activity must suppress WorkerStalled: {ev2:?}"
    );

    // Goes stale again — must nudge again (streak was cleared), not jump
    // straight to escalate. Advance `Instant` past `DEBOUNCE_DURATION` (30s)
    // so the generic per-key debounce in `debounce_events` isn't the thing
    // suppressing re-emission — this test is isolating the streak-reset
    // logic specifically, not the debounce window.
    let t1 = t0 + std::time::Duration::from_secs(31);
    let ev3 = detector.detect_changes_at(&stalled, None, t1, base_utc);
    assert!(
        ev3.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled {
                escalate: false,
                ..
            }
        )),
        "a fresh stall after activity resumed must re-nudge, not escalate: {ev3:?}"
    );
}

/// cas-728b: reproduces the exact 2026-07-07 false-positive class — two
/// workers flagged stalled ~5m after a task started while still in a
/// read-only investigation turn (zero checkpoint-class events by design).
/// A genuine Open→InProgress transition must grant a grace period keyed to
/// task-start, and that grace period must naturally elapse once the task
/// has truly run past the threshold with zero activity since start.
#[test]
fn test_728b_worker_stalled_suppressed_within_grace_window_then_fires_once_elapsed() {
    let t0 = std::time::Instant::now();
    let base_utc = chrono::Utc::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);

    // Tick 1: task is still Open — nobody has started it yet. Establishes
    // `last_state` so the next tick's Open→InProgress edge is detectable.
    let open_data = DirectorData {
        ready_tasks: vec![make_task(
            "cas-6945",
            "Dense investigation task",
            TaskStatus::Open,
            None,
        )],
        in_progress_tasks: vec![],
        epic_tasks: vec![],
        agents: vec![],
        activity: vec![],
        agent_id_to_name: [("agent-1".to_string(), "lively-crow".to_string())]
            .into_iter()
            .collect(),
        changes: vec![],
        git_loaded: true,
        reminders: vec![],
        epic_closed_counts: HashMap::new(),
    };
    detector.initialize(&open_data);

    // Tick 2 (at base_utc): task just transitioned to InProgress. Worker is
    // in its first read/plan turn — fresh heartbeat, but zero
    // checkpoint-class activity ever recorded (`latest_activity: None`),
    // exactly the repro's "explicitly instructed evidence-first, so zero
    // file/git activity is the CORRECT behavior" shape.
    let just_started = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-6945",
        5,    // fresh heartbeat
        None, // no checkpoint activity ever recorded
        base_utc,
    ));
    let ev_at_start = detector.detect_changes_at(&just_started, None, t0, base_utc);
    assert!(
        !ev_at_start
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerStalled { .. })),
        "must not fire immediately at task-start with zero checkpoint activity: {ev_at_start:?}"
    );

    // Tick 3: same task, still InProgress (no further status transition —
    // just_transitioned_to_in_progress is now false), 310s later, STILL
    // zero checkpoint activity. The grace period from tick 2 has now
    // genuinely elapsed past the 300s threshold — must fire.
    let later_utc = base_utc + chrono::Duration::seconds(310);
    let still_quiet = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-6945",
        5,
        None,
        later_utc,
    ));
    let ev_later = detector.detect_changes_at(&still_quiet, None, t0, later_utc);
    assert!(
        ev_later.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled {
                escalate: false,
                ..
            }
        )),
        "grace period must naturally elapse once genuinely past the threshold: {ev_later:?}"
    );
}

/// cas-728b / cas-de95: `transcript_confirms_stall_for_path` — the pure
/// decision behind the transcript-mtime confirmation gate. Missing telemetry
/// must **not** confirm a stall (false auto-nudge root cause).
#[test]
fn test_728b_transcript_confirms_stall_for_path_freshness_decision() {
    use cas_mux::SupervisorCli;
    let tmp = tempfile::tempdir().unwrap();

    // cas-de95: unresolved path is missing evidence — do not confirm stall.
    assert!(
        !transcript_confirms_stall_for_path(None, SupervisorCli::Claude),
        "missing transcript path must NOT confirm stall (telemetry absence ≠ starvation)"
    );

    // Path claimed but file missing → mtime unreadable → same: no confirm.
    let missing = tmp.path().join("does-not-exist.jsonl");
    assert!(
        !transcript_confirms_stall_for_path(Some(&missing), SupervisorCli::Claude),
        "unreadable transcript mtime must NOT confirm stall"
    );

    // Just-written file: fresh mtime — suppress stall.
    let fresh = tmp.path().join("fresh.jsonl");
    std::fs::write(&fresh, b"{}").unwrap();
    assert!(
        !transcript_confirms_stall_for_path(Some(&fresh), SupervisorCli::Claude),
        "a transcript written moments ago must suppress the stall verdict"
    );

    // Resolved cold transcript — positive evidence of inactivity; confirm.
    // cas-7e85: TRANSCRIPT_FRESH_WINDOW widened 60s -> 3min, so 200s (past
    // the widened window, still within Codex's 5m — covered by the cas-ab80
    // harness-window tests below) replaces the old 120s fixture.
    let stale = tmp.path().join("stale.jsonl");
    std::fs::write(&stale, b"{}").unwrap();
    let old_mtime = filetime::FileTime::from_system_time(
        std::time::SystemTime::now() - std::time::Duration::from_secs(200),
    );
    filetime::set_file_mtime(&stale, old_mtime).unwrap();
    assert!(
        transcript_confirms_stall_for_path(Some(&stale), SupervisorCli::Claude),
        "a transcript stale past Claude TRANSCRIPT_FRESH_WINDOW must confirm the stall verdict"
    );
}

/// A worker with a stale heartbeat (not just stale activity) is not "alive"
/// by the fresh-heartbeat gate — that's the existing `[stale]`/`[DEAD]`
/// liveness signal's territory, not a stall. WorkerStalled must not fire.
#[test]
fn test_9829_worker_stalled_requires_fresh_heartbeat() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        200, // heartbeat stale (> FRESH_HEARTBEAT_SECS = 60)
        Some(400),
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerStalled { .. })),
        "a stale heartbeat must not produce WorkerStalled — that's the dead/stale liveness \
         signal's job: {events:?}"
    );
}

/// The stall threshold is configurable — a worker stalled past a
/// tightened threshold fires even though it's well under the 300s default.
#[test]
fn test_9829_worker_stalled_threshold_is_configurable() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(60);

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,
        Some(65), // past the tightened 60s threshold, well under the 300s default
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        events.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled {
                escalate: false,
                ..
            }
        )),
        "a lowered stall_threshold_secs must be honored: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// cas-09d0: transcript-mtime activity, effort-aware thresholds, holds
// ---------------------------------------------------------------------------

/// AC (a): a fresh transcript (< TRANSCRIPT_FRESH_WINDOW) must suppress the
/// stall alert even though the checkpoint-class `latest_activity` signal is
/// long past the threshold — this is the exact false-positive class from the
/// live repro (worker actively producing tokens, zero file-edit/commit/
/// subagent events). Uses `set_transcript_age_override` so the suppression
/// path is exercised end-to-end through `detect_changes_at` without a real
/// SqliteAgentStore + filesystem round-trip.
#[test]
fn test_09d0_fresh_transcript_suppresses_stall_alert() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);
    detector.set_transcript_age_override(HashMap::from([(
        "lively-crow".to_string(),
        Some(std::time::Duration::from_secs(3)), // transcript written 3s ago
    )]));

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,         // fresh heartbeat
        Some(310), // checkpoint-class activity 310s ago — past the threshold
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerStalled { .. })),
        "a transcript written 3s ago must suppress the stall alert regardless \
         of stale checkpoint-class activity: {events:?}"
    );
}

/// Inverse of the above: a stale transcript (>= TRANSCRIPT_FRESH_WINDOW) does
/// NOT suppress — the checkpoint-age verdict stands.
#[test]
fn test_09d0_stale_transcript_does_not_suppress_stall_alert() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);
    detector.set_transcript_age_override(HashMap::from([(
        "lively-crow".to_string(),
        Some(std::time::Duration::from_secs(600)), // transcript stale too
    )]));

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,
        Some(310),
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        events.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled {
                escalate: false,
                ..
            }
        )),
        "a stale transcript must NOT suppress a genuine stall: {events:?}"
    );
}

/// cas-7e85 AC1 end-to-end: an in-flight tool call suppresses the
/// WorkerStalled alert even when BOTH the checkpoint-class activity AND the
/// (overridden) transcript age are long past every threshold — this is the
/// exact shape of all four reported false positives (a worker sleeping on a
/// backgrounded `cargo test --no-fail-fast` gate, transcript cold for
/// minutes because there is nothing to write while the shell blocks on
/// `sleep`).
#[test]
fn test_7e85_in_flight_tool_call_suppresses_stall_alert() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);
    detector.set_transcript_age_override(HashMap::from([(
        "lively-crow".to_string(),
        Some(std::time::Duration::from_secs(280)), // cold the whole `sleep 280` duration
    )]));
    detector.set_transcript_in_flight_override(HashMap::from([(
        "lively-crow".to_string(),
        true,
    )]));

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,         // fresh heartbeat
        Some(400), // checkpoint-class activity long past the threshold too
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerStalled { .. })),
        "an in-flight tool call must suppress the stall alert regardless of \
         cold transcript AND stale checkpoint activity: {events:?}"
    );
}

/// cas-7e85 AC2 (safety property): the in-flight-tool-call check is an
/// ADDITIONAL suppression path, not a replacement for the existing one — a
/// worker with NO in-flight call and a genuinely cold transcript must still
/// be flagged stalled. Same shape as
/// `test_09d0_stale_transcript_does_not_suppress_stall_alert` but explicit
/// about the in-flight override being `false`, so this test fails loudly if
/// a future change makes "no override present" accidentally default to
/// "suppress".
#[test]
fn test_7e85_no_in_flight_call_does_not_suppress_genuine_stall() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);
    detector.set_transcript_age_override(HashMap::from([(
        "lively-crow".to_string(),
        Some(std::time::Duration::from_secs(600)), // cold, no outstanding call
    )]));
    detector.set_transcript_in_flight_override(HashMap::from([(
        "lively-crow".to_string(),
        false,
    )]));

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,
        Some(610),
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        events.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled {
                escalate: false,
                ..
            }
        )),
        "a genuinely wedged worker (no in-flight call, cold transcript) must \
         still be flagged — the safety property must survive cas-7e85: {events:?}"
    );
}

/// Pure-function coverage for the transcript age → confirm decision,
/// independent of the DirectorEventDetector plumbing above.
#[test]
fn test_09d0_transcript_confirms_stall_for_age_pure_cases() {
    use crate::cli::factory::wedged::TRANSCRIPT_FRESH_WINDOW;
    assert!(
        !transcript_confirms_stall_for_age(
            Some(std::time::Duration::from_secs(5)),
            TRANSCRIPT_FRESH_WINDOW,
            false,
        ),
        "fresh age must NOT confirm the stall (i.e. suppress it)"
    );
    // cas-7e85: TRANSCRIPT_FRESH_WINDOW widened 60s -> 3min, so the "stale"
    // fixture moved from 120s to 200s to stay past the window.
    assert!(
        transcript_confirms_stall_for_age(
            Some(std::time::Duration::from_secs(200)),
            TRANSCRIPT_FRESH_WINDOW,
            false,
        ),
        "stale age must confirm (proceed with) the stall verdict"
    );
    // cas-de95: unknown age is missing evidence, not a stall confirmation.
    assert!(
        !transcript_confirms_stall_for_age(None, TRANSCRIPT_FRESH_WINDOW, false),
        "unknown age (no telemetry) must NOT confirm stall — absence ≠ starvation"
    );
    // cas-7e85 AC1: an in-flight tool call suppresses unconditionally, even
    // with an age that would otherwise confirm the stall.
    assert!(
        !transcript_confirms_stall_for_age(
            Some(std::time::Duration::from_secs(10 * 60)),
            TRANSCRIPT_FRESH_WINDOW,
            true,
        ),
        "an in-flight tool call must suppress the stall verdict regardless of age"
    );
}

/// cas-ab80: director must use the same harness-specific fresh window as
/// `is-wedged` (`activity_fresh_window`). A Codex transcript at 120s is still
/// "fresh" under the 5m Codex window and must not confirm stall; under the
/// Claude 60s window the same age confirms.
#[test]
fn test_ab80_harness_specific_transcript_window_matches_is_wedged() {
    use crate::cli::factory::wedged::{
        CODEX_TRANSCRIPT_FRESH_WINDOW, GROK_TRANSCRIPT_FRESH_WINDOW, TRANSCRIPT_FRESH_WINDOW,
        activity_fresh_window,
    };
    use cas_mux::SupervisorCli;

    // Pin the shared helper the production path uses.
    assert_eq!(
        activity_fresh_window(SupervisorCli::Codex),
        CODEX_TRANSCRIPT_FRESH_WINDOW
    );
    assert_eq!(
        activity_fresh_window(SupervisorCli::Claude),
        TRANSCRIPT_FRESH_WINDOW
    );
    // cas-7e85: Grok kept at its own (unwidened) constant — deliberately NOT
    // the same value as Claude's TRANSCRIPT_FRESH_WINDOW anymore.
    assert_eq!(
        activity_fresh_window(SupervisorCli::Grok),
        GROK_TRANSCRIPT_FRESH_WINDOW
    );

    let mid_codex_window = std::time::Duration::from_secs(120); // past 60s, under 5m
    assert!(
        !transcript_confirms_stall_for_age(
            Some(mid_codex_window),
            activity_fresh_window(SupervisorCli::Codex),
            false,
        ),
        "Codex transcript active within 5m window must NOT confirm stall"
    );
    // cas-7e85: TRANSCRIPT_FRESH_WINDOW widened 60s -> 3min, so this
    // sub-case needs an age past 3min (not 120s) to still confirm for Claude.
    let past_claude_window = std::time::Duration::from_secs(200);
    assert!(
        transcript_confirms_stall_for_age(
            Some(past_claude_window),
            activity_fresh_window(SupervisorCli::Claude),
            false,
        ),
        "Claude transcript at 200s (past the widened 3min window) must confirm stall"
    );
    assert!(
        transcript_confirms_stall_for_age(
            Some(std::time::Duration::from_secs(6 * 60)),
            activity_fresh_window(SupervisorCli::Codex),
            false,
        ),
        "Codex transcript past 5m window must confirm stall"
    );
}

/// cas-ab80 end-to-end: a Codex-window transcript age of 120s suppresses the
/// director stall nudge even when checkpoint activity is long past threshold.
#[test]
fn test_ab80_codex_mid_window_transcript_suppresses_stall_alert() {
    use crate::cli::factory::wedged::CODEX_TRANSCRIPT_FRESH_WINDOW;

    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);
    detector.set_transcript_age_override(HashMap::from([(
        "lively-crow".to_string(),
        Some(std::time::Duration::from_secs(120)), // past Claude 60s, under Codex 5m
    )]));
    detector.set_transcript_window_override(HashMap::from([(
        "lively-crow".to_string(),
        CODEX_TRANSCRIPT_FRESH_WINDOW,
    )]));

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,
        Some(310),
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerStalled { .. })),
        "Codex-window transcript at 120s must suppress stall nudge (director/is-wedged agreement): {events:?}"
    );
}

/// cas-de95: unresolved transcript must not auto-nudge a worker that only
/// looks stalled on checkpoint-age (live Codex/Claude false-positive class).
#[test]
fn test_de95_missing_transcript_does_not_auto_nudge() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["codex-worker".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);
    // Explicit None age = unresolved telemetry (same as no path).
    detector.set_transcript_age_override(HashMap::from([("codex-worker".to_string(), None)]));

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "codex-worker",
        "cas-de95",
        5,
        Some(310),
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerStalled { .. })),
        "missing transcript must not fire WorkerStalled auto-nudge: {events:?}"
    );

    // Unchanged evidence on the next tick must still not nudge (dedup/suppression).
    let events2 = detector.detect_changes_at(
        &data,
        None,
        t0 + std::time::Duration::from_secs(30),
        base_utc,
    );
    assert!(
        !events2
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerStalled { .. })),
        "repeat tick with same missing-transcript evidence must not nudge: {events2:?}"
    );
}

/// AC (b): effort scales the stall threshold. A base threshold of 300s with
/// an XHigh-effort worker (3x) must NOT fire at 310s elapsed — that's well
/// under the effective 900s threshold — while the same elapsed time on a
/// worker with no configured effort (1x, the pre-cas-09d0 default) DOES fire.
#[test]
fn test_09d0_xhigh_effort_worker_gets_longer_grace_before_stalling() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);

    let agent = with_effort(
        make_agent_working_stalled("agent-1", "lively-crow", "cas-0b7d", 5, Some(310), base_utc),
        cas_mux::Effort::XHigh,
    );
    let data = stalled_data_for(agent);
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerStalled { .. })),
        "an XHigh-effort worker at 310s elapsed (under the scaled 900s \
         threshold) must not be flagged stalled: {events:?}"
    );
}

/// Same elapsed time (310s) with no configured effort must still fire at the
/// unscaled base threshold — proves the scaling is additive, not a blanket
/// suppression, and that `None` effort preserves pre-cas-09d0 behavior.
#[test]
fn test_09d0_no_effort_worker_uses_unscaled_threshold() {
    let base_utc = chrono::Utc::now();
    let t0 = std::time::Instant::now();

    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.set_stall_threshold_secs(300);

    let data = stalled_data_for(make_agent_working_stalled(
        "agent-1",
        "lively-crow",
        "cas-0b7d",
        5,
        Some(310),
        base_utc,
    ));
    detector.initialize(&data);

    let events = detector.detect_changes_at(&data, None, t0, base_utc);
    assert!(
        events.iter().any(|e| matches!(
            e,
            DirectorEvent::WorkerStalled {
                escalate: false,
                ..
            }
        )),
        "no configured effort must use the unscaled base threshold: {events:?}"
    );
}

/// Pure-function coverage for the effort → multiplier table.
#[test]
fn test_09d0_effective_stall_threshold_secs_scaling_table() {
    assert_eq!(effective_stall_threshold_secs(300, None), 300);
    assert_eq!(
        effective_stall_threshold_secs(300, Some(cas_mux::Effort::Minimal)),
        300
    );
    assert_eq!(
        effective_stall_threshold_secs(300, Some(cas_mux::Effort::Low)),
        300
    );
    assert_eq!(
        effective_stall_threshold_secs(300, Some(cas_mux::Effort::Medium)),
        450
    );
    assert_eq!(
        effective_stall_threshold_secs(300, Some(cas_mux::Effort::High)),
        600
    );
    assert_eq!(
        effective_stall_threshold_secs(300, Some(cas_mux::Effort::XHigh)),
        900
    );
}

/// AC (c): a worker explicitly put on hold must never be flagged
/// `WorkerIdle`, no matter how many ticks it sits idle — the hold is a
/// deliberate supervisor pause, not idle-needing-work.
#[test]
fn test_09d0_held_worker_never_flags_idle() {
    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.mark_worker_hold("lively-crow");
    assert!(detector.is_worker_held("lively-crow"));

    let data = idle_data_for("agent-1", "lively-crow");
    detector.initialize(&data);

    // Drive several ticks past IDLE_CONSECUTIVE_TICKS — a non-held worker
    // would have fired WorkerIdle by now (see the idle-detection tests
    // above using the same `idle_data_for` fixture).
    let mut saw_idle = false;
    for _ in 0..5 {
        let events = detector.detect_changes(&data, None);
        if events
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. }))
        {
            saw_idle = true;
        }
    }
    assert!(!saw_idle, "a held worker must never emit WorkerIdle");
}

/// Clearing a hold resumes normal idle detection on subsequent ticks.
#[test]
fn test_09d0_clearing_hold_resumes_idle_detection() {
    let mut detector =
        DirectorEventDetector::new(vec!["lively-crow".to_string()], "supervisor".to_string());
    detector.mark_worker_hold("lively-crow");

    let data = idle_data_for("agent-1", "lively-crow");
    detector.initialize(&data);
    for _ in 0..3 {
        detector.detect_changes(&data, None);
    }

    detector.clear_worker_hold("lively-crow");
    assert!(!detector.is_worker_held("lively-crow"));

    // IDLE_CONSECUTIVE_TICKS is 2 — two ticks after release must emit.
    detector.detect_changes(&data, None);
    let events = detector.detect_changes(&data, None);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DirectorEvent::WorkerIdle { .. })),
        "idle detection must resume once the hold is cleared: {events:?}"
    );
}
