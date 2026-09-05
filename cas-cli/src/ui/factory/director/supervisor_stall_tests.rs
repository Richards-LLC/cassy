use super::data::{AgentSummary, DirectorData, TaskSummary};
use super::events::{
    SupervisorActionableState, SupervisorStallTracker, supervisor_actionable_state,
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
        Some(start - Duration::seconds(1_199)),
        false,
        start + Duration::seconds(599),
        600,
    );
    assert!(early.wake.is_none());
    assert_eq!(early.actionable_idle_secs, 599);

    let refire = tracker.observe(
        Some(actionable),
        Some(start - Duration::seconds(1_200)),
        false,
        start + Duration::seconds(600),
        600,
    );
    assert!(refire.wake.is_some());
    assert_eq!(refire.actionable_idle_secs, 600);

    let cleared = tracker.observe(
        None,
        Some(start + Duration::seconds(601)),
        false,
        start + Duration::seconds(720),
        600,
    );
    assert_eq!(cleared.actionable_idle_secs, 720);
}
