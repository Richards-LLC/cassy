use super::data::{AgentSummary, DirectorData, TaskSummary};
use super::events::{
    MergedCloseBlockedTask, SupervisorActionableState, SupervisorStallTracker,
    supervisor_actionable_state, supervisor_actionable_state_with_merge_classifier,
};
use cas_types::{AgentStatus, Priority, TaskStatus, TaskType};
use chrono::{Duration, TimeZone, Utc};
use std::collections::{HashMap, HashSet};

fn task(
    id: &str,
    status: TaskStatus,
    assignee: Option<&str>,
    epic: Option<&str>,
) -> TaskSummary {
    TaskSummary {
        id: id.into(),
        title: format!("title {id}"),
        status,
        priority: Priority::HIGH,
        assignee: assignee.map(str::to_string),
        task_type: TaskType::Task,
        epic: epic.map(str::to_string),
        branch: None,
        updated_at: None,
        epic_verification_owner: None,
    }
}

fn epic(id: &str) -> TaskSummary {
    TaskSummary {
        id: id.into(),
        title: format!("epic {id}"),
        status: TaskStatus::InProgress,
        priority: Priority::HIGH,
        assignee: None,
        task_type: TaskType::Epic,
        epic: None,
        branch: Some(format!("epic/{id}")),
        updated_at: None,
        epic_verification_owner: None,
    }
}

fn worker(name: &str, registered_at: chrono::DateTime<Utc>) -> AgentSummary {
    AgentSummary {
        id: format!("id-{name}"),
        name: name.into(),
        status: AgentStatus::Active,
        registered_at,
        current_task: None,
        latest_activity: None,
        last_heartbeat: Some(registered_at),
        pending_messages: 0,
        pending_supervisor_messages: 0,
        latest_supervisor_message_at: None,
        active_lease: None,
        effort: None,
    }
}

fn data() -> DirectorData {
    DirectorData {
        ready_tasks: Vec::new(),
        in_progress_tasks: Vec::new(),
        epic_tasks: vec![epic("cas-epic")],
        agents: Vec::new(),
        activity: Vec::new(),
        agent_id_to_name: HashMap::new(),
        changes: Vec::new(),
        git_loaded: false,
        reminders: Vec::new(),
        epic_closed_counts: HashMap::new(),
        start_gated_task_ids: Default::default(),
    }
}

#[test]
fn awaiting_merge_names_task_branch_and_live_tip() {
    let now = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap();
    let mut snapshot = data();
    snapshot.in_progress_tasks.push(task(
        "cas-merge",
        TaskStatus::AwaitingMerge,
        Some("gold-fox"),
        Some("cas-epic"),
    ));

    let state = supervisor_actionable_state(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |branch| (branch == "factory/gold-fox").then(|| "abc123".to_string()),
    );

    assert_eq!(
        state,
        Some(SupervisorActionableState::MergeBranches {
            branches: vec![(
                "cas-merge".into(),
                "factory/gold-fox".into(),
                "abc123".into(),
            )],
        })
    );
}

#[test]
fn merged_delivery_is_classified_as_close_blocked_with_rejection_and_reclose() {
    let now = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap();
    let mut snapshot = data();
    snapshot.in_progress_tasks.push(task(
        "cas-merged",
        TaskStatus::AwaitingMerge,
        Some("gold-fox"),
        Some("cas-epic"),
    ));

    let state = supervisor_actionable_state_with_merge_classifier(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |branch| (branch == "factory/gold-fox").then(|| "factory-tip".to_string()),
        |task, factory_branch, target_branch, factory_tip| {
            assert_eq!(task.id, "cas-merged");
            assert_eq!(factory_branch, "factory/gold-fox");
            assert_eq!(target_branch, "epic/cas-epic");
            assert_eq!(factory_tip, Some("factory-tip"));
            Some(MergedCloseBlockedTask {
                task_id: task.id.clone(),
                factory_branch: factory_branch.to_string(),
                anchor: "anchor-sha".to_string(),
                target_branch: target_branch.to_string(),
                target_tip: "target-tip".to_string(),
                close_rejection: "ZERO-COMMIT after merged delivery".to_string(),
            })
        },
    );

    let Some(SupervisorActionableState::MergeCloseBlocked { tasks }) = state else {
        panic!("merged delivery must be classified separately: {state:?}");
    };
    assert_eq!(tasks.len(), 1);
    let rendered = SupervisorActionableState::MergeCloseBlocked { tasks }.next_step_text();
    assert!(rendered.contains("already merged"));
    assert!(rendered.contains("ZERO-COMMIT after merged delivery"));
    assert!(rendered.contains("task action=close id=cas-merged"));
    assert!(
        !rendered.contains("mcp__"),
        "event facts cannot assume a recipient harness"
    );
    assert!(!rendered.contains("merge the ready delivery branch(es)"));
}

#[test]
fn ready_work_and_idle_worker_become_actionable_only_after_threshold() {
    let now = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap();
    let mut snapshot = data();
    snapshot
        .ready_tasks
        .push(task("cas-ready", TaskStatus::Open, None, Some("cas-epic")));
    snapshot
        .agents
        .push(worker("gold-fox", now - Duration::seconds(599)));

    let early = supervisor_actionable_state(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |_| None,
    );
    assert_eq!(early, None);

    snapshot.agents[0].registered_at = now - Duration::seconds(600);
    let ready = supervisor_actionable_state(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |_| None,
    );
    assert_eq!(
        ready,
        Some(SupervisorActionableState::AssignReadyWork {
            task_ids: vec!["cas-ready".into()],
            idle_workers: vec!["gold-fox".into()],
        })
    );
}

#[test]
fn all_terminal_children_choose_assembly_exit_step() {
    let now = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap();
    let mut snapshot = data();
    snapshot.epic_closed_counts.insert("cas-epic".into(), 3);

    assert_eq!(
        supervisor_actionable_state(
            &snapshot,
            Some("cas-epic"),
            "supervisor",
            &HashSet::new(),
            now,
            600,
            |_| None,
        ),
        Some(SupervisorActionableState::AssembleGatePipeline {
            epic_id: "cas-epic".into(),
        })
    );
}

#[test]
fn stall_gate_suppresses_recent_supervisor_action_and_covering_reminder() {
    let now = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap();
    let actionable = SupervisorActionableState::AssembleGatePipeline {
        epic_id: "cas-epic".into(),
    };
    let mut tracker = SupervisorStallTracker::default();

    assert!(
        tracker
            .observe(
                Some(actionable.clone()),
                Some(now - Duration::seconds(599)),
                false,
                now,
                600,
            )
            .wake
            .is_none()
    );
    assert!(
        tracker
            .observe(
                Some(actionable),
                Some(now - Duration::seconds(600)),
                true,
                now,
                600,
            )
            .wake
            .is_none()
    );
}

#[test]
fn stall_gate_fires_once_per_ten_minutes_and_accumulates_actionable_idle_time() {
    let start = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap();
    let actionable = SupervisorActionableState::AssembleGatePipeline {
        epic_id: "cas-epic".into(),
    };
    let mut tracker = SupervisorStallTracker::default();

    let first = tracker.observe(
        Some(actionable.clone()),
        Some(start - Duration::seconds(600)),
        false,
        start,
        600,
    );
    assert_eq!(first.wake, Some(actionable.clone()));

    let early = tracker.observe(
        Some(actionable.clone()),
        Some(start - Duration::seconds(600)),
        false,
        start + Duration::seconds(599),
        600,
    );
    assert!(early.wake.is_none());
    assert_eq!(early.actionable_idle_secs, 599);

    let refire = tracker.observe(
        Some(actionable),
        Some(start - Duration::seconds(600)),
        false,
        start + Duration::seconds(600),
        600,
    );
    assert!(refire.wake.is_some());
    assert_eq!(refire.actionable_idle_secs, 600);

    let cleared = tracker.observe(
        None,
        Some(start - Duration::seconds(600)),
        false,
        start + Duration::seconds(720),
        600,
    );
    assert_eq!(cleared.actionable_idle_secs, 720);
    assert!(cleared.wake.is_none());
}

#[test]
fn supervisor_tool_activity_resets_actionable_idle_clock() {
    let start = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap();
    let actionable = SupervisorActionableState::AssembleGatePipeline {
        epic_id: "cas-epic".into(),
    };
    let mut tracker = SupervisorStallTracker::default();

    assert!(
        tracker
            .observe(
                Some(actionable.clone()),
                Some(start - Duration::seconds(600)),
                false,
                start,
                600,
            )
            .wake
            .is_some()
    );

    // A later supervisor tool call is forward motion even when the same
    // actionable item remains in the snapshot. It must begin a fresh idle
    // span instead of carrying the completed span into the next nag.
    let after_activity = tracker.observe(
        Some(actionable.clone()),
        Some(start + Duration::seconds(1)),
        false,
        start + Duration::seconds(1),
        600,
    );
    assert_eq!(after_activity.actionable_idle_secs, 0);
    assert!(after_activity.wake.is_none());

    let after_silence = tracker.observe(
        Some(actionable),
        Some(start + Duration::seconds(1)),
        false,
        start + Duration::seconds(601),
        600,
    );
    assert!(after_silence.wake.is_some());
}

#[test]
fn merged_close_blocked_stall_wakes_for_changed_delivery_without_refiring_same_state() {
    let start = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap();
    let merged = |target_tip: &str| SupervisorActionableState::MergeCloseBlocked {
        tasks: vec![MergedCloseBlockedTask {
            task_id: "cas-merged".into(),
            factory_branch: "factory/gold-fox".into(),
            anchor: "anchor-sha".into(),
            target_branch: "epic/cas-epic".into(),
            target_tip: target_tip.into(),
            close_rejection: "already merged".into(),
        }],
    };
    let mut tracker = SupervisorStallTracker::default();

    assert!(
        tracker
            .observe(
                Some(merged("target-a")),
                Some(start - Duration::seconds(600)),
                false,
                start,
                600,
            )
            .wake
            .is_some()
    );
    assert!(
        tracker
            .observe(
                Some(merged("target-a")),
                Some(start - Duration::seconds(600)),
                false,
                start + Duration::seconds(600),
                600,
            )
            .wake
            .is_none()
    );
    assert!(
        tracker
            .observe(
                Some(merged("target-b")),
                Some(start - Duration::seconds(600)),
                false,
                start + Duration::seconds(601),
                600,
            )
            .wake
            .is_some()
    );
}

/// cas-6577: a gate task whose `requires_start` prerequisite is still in
/// progress is refused by `task action=start`, so the stall detector must not
/// tell the supervisor to assign it to an idle worker.
#[test]
fn start_gated_task_is_never_named_for_assignment() {
    let now = Utc.with_ymd_and_hms(2026, 9, 22, 22, 18, 0).unwrap();
    let mut snapshot = data();
    snapshot.in_progress_tasks.push(task(
        "cas-7294",
        TaskStatus::InProgress,
        Some("id-watchful-swan-31"),
        Some("cas-epic"),
    ));
    snapshot
        .ready_tasks
        .push(task("cas-1cd0", TaskStatus::Open, None, Some("cas-epic")));
    snapshot.start_gated_task_ids.insert("cas-1cd0".into());
    let mut busy = worker("watchful-swan-31", now - Duration::seconds(3600));
    busy.current_task = Some("cas-7294".into());
    snapshot.agents.push(busy);
    snapshot
        .agents
        .push(worker("lively-hawk-36", now - Duration::seconds(3600)));

    let state = supervisor_actionable_state(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |_| None,
    );
    assert_eq!(state, None, "only start-gated work is open: no nudge");

    // An ungated sibling is still named; the gate never is.
    snapshot
        .ready_tasks
        .push(task("cas-free", TaskStatus::Open, None, Some("cas-epic")));
    let state = supervisor_actionable_state(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |_| None,
    );
    assert_eq!(
        state,
        Some(SupervisorActionableState::AssignReadyWork {
            task_ids: vec!["cas-free".into()],
            idle_workers: vec!["lively-hawk-36".into()],
        })
    );

    // Once the prerequisite is terminal the snapshot no longer gates it.
    snapshot.start_gated_task_ids.clear();
    let state = supervisor_actionable_state(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |_| None,
    );
    assert_eq!(
        state,
        Some(SupervisorActionableState::AssignReadyWork {
            task_ids: vec!["cas-1cd0".into(), "cas-free".into()],
            idle_workers: vec!["lively-hawk-36".into()],
        })
    );
}

/// cas-0c98 (GH #995): the pulse-card supervisor was nudged to assign
/// gabber-studio tasks to its idle workers. Seed a real store with a
/// foreign-origin open task linked under the session's focused epic plus a
/// foreign epic with its own ready child; the stall nudge built from the
/// session's project-scoped load names only the local task.
#[test]
fn stall_nudge_never_suggests_foreign_origin_tasks_cas_0c98() {
    use cas_store::{AgentStore, EventStore, TaskStore};
    use cas_types::{Dependency, DependencyType, Task};

    let temp = tempfile::tempdir().unwrap();
    let stores = cas_factory::DirectorStores::open(temp.path()).unwrap();
    stores.task_store.init().unwrap();
    stores.agent_store.init().unwrap();
    stores.event_store.init().unwrap();
    let add = |id: &str, task_type: TaskType, origin: &str, epic: Option<&str>| {
        let mut task = Task::new(id.to_string(), format!("{id} title"));
        task.task_type = task_type;
        task.origin_project = Some(origin.to_string());
        if task_type == TaskType::Epic {
            task.status = TaskStatus::InProgress;
            task.branch = Some(format!("epic/{id}"));
        }
        stores.task_store.add(&task).unwrap();
        if let Some(epic) = epic {
            stores
                .task_store
                .add_dependency(&Dependency::new(
                    id.to_string(),
                    epic.to_string(),
                    DependencyType::ParentChild,
                ))
                .unwrap();
        }
    };
    add("cas-epic", TaskType::Epic, "pulse-card", None);
    add("cas-local-ready", TaskType::Task, "pulse-card", Some("cas-epic"));
    add("cas-1aec", TaskType::Task, "gabber-studio", Some("cas-epic"));
    add("cas-foreign-epic", TaskType::Epic, "gabber-studio", None);
    add("cas-72d3", TaskType::Task, "gabber-studio", Some("cas-foreign-epic"));

    let now = Utc::now();
    let nudge = |project: Option<&str>, focus: &str| {
        let mut snapshot =
            DirectorData::load_for_project(temp.path(), None, false, project).unwrap();
        snapshot
            .agents
            .push(worker("gold-fox", now - Duration::seconds(3_600)));
        supervisor_actionable_state(
            &snapshot,
            Some(focus),
            "supervisor",
            &HashSet::new(),
            now,
            600,
            |_| None,
        )
    };

    assert_eq!(
        nudge(Some("pulse-card"), "cas-epic"),
        Some(SupervisorActionableState::AssignReadyWork {
            task_ids: vec!["cas-local-ready".into()],
            idle_workers: vec!["gold-fox".into()],
        }),
        "the stall nudge must name only this project's task"
    );
    assert_eq!(
        nudge(Some("pulse-card"), "cas-foreign-epic"),
        None,
        "a foreign epic is not in the session's snapshot, so its children are never suggested"
    );
    // Precondition: without a project scope the foreign rows reach the nudge,
    // which is the GH #995 symptom.
    let Some(SupervisorActionableState::AssignReadyWork { task_ids, .. }) =
        nudge(None, "cas-epic")
    else {
        panic!("unscoped load should still offer ready work");
    };
    assert!(task_ids.contains(&"cas-1aec".to_string()), "{task_ids:?}");
}
