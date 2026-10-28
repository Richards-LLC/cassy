use super::data::{AgentSummary, DirectorData, TaskSummary};
use super::events::{
    DeliveryHold, MergedCloseBlockedTask, SupervisorActionableState, SupervisorStallTracker,
    blocker_note_after_park, hold_decision_after_park, supervisor_actionable_state,
    supervisor_actionable_state_with_classifiers,
    supervisor_actionable_state_with_merge_classifier, task_delivery_ref,
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

/// The legacy `factory/<worker>` delivery with a `<branch>-tip` commit.
fn worker_branch_tip(_: &TaskSummary, worker: &str) -> (String, Option<String>) {
    let branch = format!("factory/{worker}");
    let tip = format!("{branch}-tip");
    (branch, Some(tip))
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
        qa_reviewer_between_rounds: false,
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

/// cas-7456 (GH #1302 C): a supervisor set tasks to Blocked (waiting on a
/// human) and the workers holding them vanished from the idle list, so the
/// ready-work page never fired while they idled for 10 h.
#[test]
fn a_blocked_task_does_not_hide_its_idle_worker_from_ready_work_gh1302() {
    let now = Utc.with_ymd_and_hms(2026, 10, 20, 11, 0, 0).unwrap();
    let mut snapshot = data();
    snapshot
        .ready_tasks
        .push(task("cas-waits-on-ben", TaskStatus::Blocked, Some("gold-fox"), Some("cas-epic")));
    snapshot
        .ready_tasks
        .push(task("cas-ready", TaskStatus::Open, None, Some("cas-epic")));
    snapshot.agents.push(worker("gold-fox", now - Duration::hours(10)));

    assert_eq!(
        supervisor_actionable_state(&snapshot, Some("cas-epic"), "supervisor", &HashSet::new(), now, 600, |_| None),
        Some(SupervisorActionableState::AssignReadyWork {
            task_ids: vec!["cas-ready".into()],
            idle_workers: vec!["gold-fox".into()],
        })
    );
}

/// cas-c73a: a pinned epic cannot fall through to the project backlog.
/// With no focus, retain the existing project-wide ready-work policy.
#[test]
fn idle_workers_beside_any_ready_work_are_paged_gh1302() {
    let now = Utc.with_ymd_and_hms(2026, 10, 20, 11, 0, 0).unwrap();
    let mut snapshot = data();
    // The focused epic still has work in flight, so it is not assembly time.
    snapshot
        .in_progress_tasks
        .push(task("cas-running", TaskStatus::InProgress, Some("busy-owl"), Some("cas-epic")));
    let backlog = (0..12).map(|n| format!("cas-r{n:02}")).collect::<Vec<_>>();
    for id in &backlog {
        snapshot.ready_tasks.push(task(id, TaskStatus::Open, None, None));
    }
    snapshot.agents.push(worker("gold-fox", now - Duration::hours(10)));
    let expected = Some(SupervisorActionableState::AssignReadyWork {
        task_ids: backlog.clone(),
        idle_workers: vec!["gold-fox".into()],
    });

    let state = supervisor_actionable_state(&snapshot, Some("cas-epic"), "supervisor", &HashSet::new(), now, 600, |_| None);
    assert_eq!(state, None, "a pinned epic has no ready children");
    assert!(!wake_for(state, now), "unrelated backlog must produce no supervisor_stalled wake");
    assert_eq!(supervisor_actionable_state(&snapshot, None, "supervisor", &HashSet::new(), now, 600, |_| None), expected);
    let text = expected.unwrap().next_step_text();
    assert!(text.contains("cas-r09 and 2 more to idle worker(s) gold-fox"), "{text}");
    snapshot.ready_tasks.push(task("cas-focused", TaskStatus::Open, None, Some("cas-epic")));
    snapshot.ready_tasks.push(task("cas-other-epic", TaskStatus::Open, None, Some("cas-other")));
    let focused = supervisor_actionable_state(&snapshot, Some("cas-epic"), "supervisor", &HashSet::new(), now, 600, |_| None);
    assert_eq!(focused, Some(SupervisorActionableState::AssignReadyWork {
        task_ids: vec!["cas-focused".into()], idle_workers: vec!["gold-fox".into()],
    }));
    let mut all = backlog;
    all.extend(["cas-focused".into(), "cas-other-epic".into()]);
    all.sort();
    assert_eq!(supervisor_actionable_state(&snapshot, None, "supervisor", &HashSet::new(), now, 600, |_| None),
        Some(SupervisorActionableState::AssignReadyWork { task_ids: all, idle_workers: vec!["gold-fox".into()] }));
}

#[test]
fn pending_reviewers_are_not_idle_assignment_candidates_cas_c73a() {
    use cas_factory::{ActiveLeaseSummary, PendingQaSummary};
    let now = Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap();
    for (identity, claimed) in [("id-reviewer", false), ("reviewer", false), ("id-reviewer", true), ("reviewer", true)] {
        let mut snapshot = data();
        snapshot.ready_tasks.push(task("cas-focused", TaskStatus::Open, None, Some("cas-epic")));
        let mut implementer = worker("implementer", now - Duration::hours(1));
        implementer.current_task = Some("cas-review".into());
        implementer.active_lease = Some(ActiveLeaseSummary {
            task_id: "cas-review".into(), task_title: "Under review".into(), task_status: TaskStatus::AwaitingMerge,
            close_rejected_reason: None,
            pending_qa: Some(PendingQaSummary { pass_id: "qa-current".into(), round: 1, qa_task_id: None,
                bound_head: "review-head".into(), reviewer_agent_id: Some(identity.into()), claimed,
                deadline_at: now + Duration::minutes(30) }),
        });
        snapshot.agents = vec![implementer, worker("reviewer", now - Duration::hours(1))];
        let action = |snapshot: &DirectorData| supervisor_actionable_state(snapshot, Some("cas-epic"), "supervisor", &HashSet::new(), now, 600, |_| None);
        assert_eq!(action(&snapshot), None, "pending reviewer {identity} is reserved");
        snapshot.agents[0].active_lease.as_mut().unwrap().pending_qa.as_mut().unwrap().deadline_at = now;
        assert!(matches!(action(&snapshot), Some(SupervisorActionableState::AssignReadyWork { .. })), "an expired round does not reserve the reviewer");
        snapshot.agents[0].active_lease.as_mut().unwrap().pending_qa = None;
        assert!(matches!(action(&snapshot), Some(SupervisorActionableState::AssignReadyWork { .. })), "unreserved worker remains available");
        snapshot.agents[1].qa_reviewer_between_rounds = true;
        assert_eq!(action(&snapshot), None, "reviewer between rounds remains reserved");
        snapshot.agents[1].qa_reviewer_between_rounds = false;
        snapshot.ready_tasks.push(task("cas-open-qa-role", TaskStatus::Open, Some("reviewer"), None));
        assert_eq!(action(&snapshot), None, "an assigned open QA role is not idle for new work");
    }
}

#[test]
fn identical_assignment_nudge_has_a_session_cooldown_cas_c73a() {
    let start = Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap();
    let action = Some(SupervisorActionableState::AssignReadyWork {
        task_ids: vec!["cas-ready".into()], idle_workers: vec!["gold-fox".into()],
    });
    let mut tracker = SupervisorStallTracker::default();
    assert!(tracker.observe(action.clone(), Some(start - Duration::minutes(10)), false, start, 600).wake.is_some());
    // The session receipt survives a daemon serialization round trip and a
    // supervisor tool call; neither may restart an identical assignment nag.
    let mut tracker: SupervisorStallTracker = serde_json::from_str(&serde_json::to_string(&tracker).unwrap()).unwrap();
    assert!(tracker.observe(action.clone(), Some(start - Duration::minutes(10)), false, start + Duration::minutes(10), 600).wake.is_none());
    let changed = Some(SupervisorActionableState::AssignReadyWork {
        task_ids: vec!["cas-new-ready".into()], idle_workers: vec!["gold-fox".into()],
    });
    assert!(tracker.observe(changed, Some(start - Duration::minutes(10)), false, start + Duration::minutes(10), 600).wake.is_some());
    let active = Some(start + Duration::minutes(11));
    assert!(tracker.observe(action.clone(), active, false, start + Duration::minutes(11), 600).wake.is_none());
    assert!(tracker.observe(action.clone(), active, false, start + Duration::minutes(21), 600).wake.is_none());
    assert!(tracker.observe(action, active, false, start + Duration::minutes(30), 600).wake.is_some());
}

/// cas-7456 (GH #1302 D): two supervisors left their idle page unanswered for
/// 11–12 h overnight. From two hours of silence with work waiting, every
/// wake asks to page the operator for that silence episode. cas-b080: the
/// tracker keeps asking, so a page whose event was dropped before delivery
/// is asked again on the next wake; the durable operator turn is deduped on
/// the episode. Fresh supervisor activity names a new episode.
#[test]
fn an_unanswered_idle_page_keeps_asking_for_the_operator_until_delivered_gh1302() {
    let start = Utc.with_ymd_and_hms(2026, 10, 20, 1, 5, 0).unwrap();
    let actionable = Some(SupervisorActionableState::AssignReadyWork {
        task_ids: vec!["cas-ready".into()],
        idle_workers: vec!["gold-fox".into()],
    });
    let mut tracker = SupervisorStallTracker::default();
    let pages = |tracker: &mut SupervisorStallTracker, last_call: i64, minutes: std::ops::RangeInclusive<i64>| {
        minutes
            .step_by(5)
            .filter_map(|minute| {
                tracker
                    .observe(
                        actionable.clone(),
                        Some(start + Duration::minutes(last_call)),
                        false,
                        start + Duration::minutes(minute),
                        600,
                    )
                    .operator_page_episode
                    .map(|episode| (minute, episode))
            })
            .collect::<Vec<_>>()
    };
    // Assignment cooldown changes wake cadence, not the rule that every
    // actual wake retries a possibly dropped operator page.
    let first = pages(&mut tracker, 0, 10..=160);
    assert_eq!(
        first,
        vec![(130, start), (160, start)],
        "every wake of the silence asks, so a dropped first page is asked again"
    );
    let second = pages(&mut tracker, 400, 410..=530);
    assert_eq!(second, vec![(530, start + Duration::minutes(400))], "a new silence is a new episode");
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
    let merged = |anchor: &str| SupervisorActionableState::MergeCloseBlocked {
        tasks: vec![MergedCloseBlockedTask {
            task_id: "cas-merged".into(),
            factory_branch: "factory/gold-fox".into(),
            anchor: anchor.into(),
            target_branch: "epic/cas-epic".into(),
            target_tip: "target-tip".into(),
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
    // An unavailable pin must not expand into the session's unrelated queue.
    assert_eq!(
        nudge(Some("pulse-card"), "cas-foreign-epic"),
        None,
        "a foreign epic's children are never suggested"
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

/// GH #896 (cas-e4f8): a parked delivery the supervisor deliberately holds —
/// a held worker, a blocker note after the park, or an open independent QA
/// pass — is not "merge now", so it never produces a supervisor_stalled wake.
fn held_delivery_snapshot() -> DirectorData {
    let mut snapshot = data();
    snapshot.in_progress_tasks.push(task(
        "cas-held",
        TaskStatus::AwaitingMerge,
        Some("gold-fox"),
        Some("cas-epic"),
    ));
    snapshot
}

fn wake_for(state: Option<SupervisorActionableState>, now: chrono::DateTime<Utc>) -> bool {
    let mut tracker = SupervisorStallTracker::default();
    // Silent well past the threshold, twice ten minutes apart.
    let first = tracker.observe(state.clone(), Some(now - Duration::seconds(3600)), false, now, 600);
    let again = tracker.observe(
        state,
        Some(now - Duration::seconds(3600)),
        false,
        now + Duration::seconds(600),
        600,
    );
    first.wake.is_some() || again.wake.is_some()
}

#[test]
fn held_worker_delivery_produces_no_stall_wake_gh896() {
    let now = Utc.with_ymd_and_hms(2026, 9, 24, 20, 0, 0).unwrap();
    let snapshot = held_delivery_snapshot();
    let held = HashSet::from(["gold-fox".to_string()]);

    let state = supervisor_actionable_state(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &held,
        now,
        600,
        |_| Some("tip".to_string()),
    );
    assert_eq!(state, None, "a held worker's delivery is not merge-now");
    assert!(!wake_for(state, now));

    // The same delivery without the hold is merge-now and does wake.
    let unheld = supervisor_actionable_state(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |_| Some("tip".to_string()),
    );
    assert!(matches!(unheld, Some(SupervisorActionableState::MergeBranches { .. })));
    assert!(wake_for(unheld, now));
}

#[test]
fn blocker_note_and_open_qa_pass_hold_a_delivery_but_not_its_siblings_gh896() {
    let now = Utc.with_ymd_and_hms(2026, 9, 24, 20, 0, 0).unwrap();
    let mut snapshot = held_delivery_snapshot();
    snapshot.in_progress_tasks.push(task(
        "cas-qa",
        TaskStatus::AwaitingMerge,
        Some("blue-owl"),
        Some("cas-epic"),
    ));
    snapshot.in_progress_tasks.push(task(
        "cas-ready",
        TaskStatus::AwaitingMerge,
        Some("red-kite"),
        Some("cas-epic"),
    ));

    let state = supervisor_actionable_state_with_classifiers(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        worker_branch_tip,
        |_, _, _, _| None,
        |task, worker| match (task.id.as_str(), worker) {
            ("cas-held", "gold-fox") => Some(DeliveryHold::BlockerNote),
            ("cas-qa", "blue-owl") => Some(DeliveryHold::QaPassOpen),
            _ => None,
        },
    );
    assert_eq!(
        state,
        Some(SupervisorActionableState::MergeBranches {
            branches: vec![(
                "cas-ready".into(),
                "factory/red-kite".into(),
                "factory/red-kite-tip".into(),
            )],
        }),
        "only the unheld delivery is named"
    );

    // With every delivery held, nothing is merge-now and no wake fires.
    let all_held = supervisor_actionable_state_with_classifiers(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        worker_branch_tip,
        |_, _, _, _| None,
        |_, _| Some(DeliveryHold::QaPassOpen),
    );
    assert_eq!(all_held, None);
    assert!(!wake_for(all_held, now));
}

#[test]
fn held_delivery_is_not_reclassified_as_merged_close_blocked_gh896() {
    // The hold wins before the merged/close-blocked classifier runs, so a
    // held delivery cannot resurface as a close demand either.
    let now = Utc.with_ymd_and_hms(2026, 9, 24, 20, 0, 0).unwrap();
    let snapshot = held_delivery_snapshot();
    let state = supervisor_actionable_state_with_classifiers(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |_, worker| (format!("factory/{worker}"), Some("tip".to_string())),
        |_, _, _, _| panic!("a held delivery must not be classified"),
        |_, _| Some(DeliveryHold::BlockerNote),
    );
    assert_eq!(state, None);
}

#[test]
fn blocker_note_counts_only_after_the_latest_park_gh896() {
    let park = "[2026-09-24 19:00] Close rejected: MERGE REQUIRED. Task parked as awaiting_merge; worker lease released until supervisor merge completes.";
    let blocker = "[2026-09-24 19:05] 🚫 BLOCKER waiting on the operator to pick the rollout window";
    let progress = "[2026-09-24 19:06] 📝 PROGRESS noted";
    assert!(!blocker_note_after_park(""));
    assert!(!blocker_note_after_park(park));
    assert!(blocker_note_after_park(&format!("{park}\n{blocker}")));
    assert!(blocker_note_after_park(&format!("{park}\n{blocker}\n{progress}")));
    // Re-parked after the blocker: the delivery came back for merge.
    assert!(!blocker_note_after_park(&format!("{park}\n{blocker}\n{park}")));
    // A blocker with no park line at all is the latest word on the task.
    assert!(blocker_note_after_park(blocker));
}

#[test]
fn legacy_merge_classifier_entry_point_still_names_unheld_deliveries() {
    let now = Utc.with_ymd_and_hms(2026, 9, 24, 20, 0, 0).unwrap();
    let snapshot = held_delivery_snapshot();
    let state = supervisor_actionable_state_with_merge_classifier(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |_| Some("tip".to_string()),
        |_, _, _, _| None,
    );
    assert!(matches!(state, Some(SupervisorActionableState::MergeBranches { .. })));
}


/// cas-58d3: the notes cas-247a carried when the relay kept nagging. The
/// supervisor's decision after the park holds the merge until after the cut.
const CAS_247A_PARK: &str = "[2026-10-02 15:02] Close rejected: MERGE REQUIRED. Task parked as awaiting_merge; worker lease released until supervisor merge completes.";
const CAS_247A_HOLD: &str = "[2026-10-02 15:02] ✅ DECISION Supervisor decision 2026-10-02 at about 15:03Z: the merge of 942a65e1 is held until after the release cut. The delivery touches scripts/release-gate.sh.";

#[test]
fn supervisor_hold_decision_counts_only_after_the_latest_park_cas_58d3() {
    let progress = "[2026-10-02 15:01] 📝 PROGRESS PASS clean pushed, ready to park";
    let other_decision = "[2026-10-02 15:04] ✅ DECISION merge order: cas-a before cas-b";
    let lift = "[2026-10-02 19:00] ✅ DECISION hold lifted: the cut is published, merge now";
    let multi_line = "[2026-10-02 15:02] ✅ DECISION Supervisor decision:\nthe merge stays on hold until the cut.";

    assert!(!hold_decision_after_park(""));
    assert!(!hold_decision_after_park(&format!("{progress}\n{CAS_247A_PARK}")));
    assert!(hold_decision_after_park(&format!(
        "{progress}\n\n{CAS_247A_PARK}\n\n{CAS_247A_HOLD}"
    )));
    // A later unrelated decision does not lift the hold.
    assert!(hold_decision_after_park(&format!(
        "{CAS_247A_PARK}\n{CAS_247A_HOLD}\n{other_decision}"
    )));
    // A decision with no hold language is not a hold.
    assert!(!hold_decision_after_park(&format!("{CAS_247A_PARK}\n{other_decision}")));
    // An explicit lift clears it, and so does a re-park.
    assert!(!hold_decision_after_park(&format!("{CAS_247A_PARK}\n{CAS_247A_HOLD}\n{lift}")));
    assert!(!hold_decision_after_park(&format!(
        "{CAS_247A_PARK}\n{CAS_247A_HOLD}\n{CAS_247A_PARK}"
    )));
    // The hold wording may sit on a continuation line of the entry.
    assert!(hold_decision_after_park(&format!("{CAS_247A_PARK}\n{multi_line}")));
    // Hold wording in a progress note is not a supervisor decision.
    assert!(!hold_decision_after_park(&format!(
        "{CAS_247A_PARK}\n[2026-10-02 15:05] 📝 PROGRESS holding for review"
    )));
}

#[test]
fn supervisor_decision_hold_silences_merge_now_cas_58d3() {
    let now = Utc.with_ymd_and_hms(2026, 10, 2, 15, 50, 0).unwrap();
    let mut snapshot = held_delivery_snapshot();
    snapshot.in_progress_tasks.push(task(
        "cas-ready",
        TaskStatus::AwaitingMerge,
        Some("red-kite"),
        Some("cas-epic"),
    ));
    let notes: HashMap<&str, String> = HashMap::from([
        ("cas-held", format!("{CAS_247A_PARK}\n{CAS_247A_HOLD}")),
        ("cas-ready", CAS_247A_PARK.to_string()),
    ]);
    // The production classifier's decision branch, over the task notes.
    let classify = |task: &TaskSummary, _: &str| {
        notes
            .get(task.id.as_str())
            .filter(|notes| hold_decision_after_park(notes))
            .map(|_| DeliveryHold::SupervisorDecision)
    };
    let state = supervisor_actionable_state_with_classifiers(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        worker_branch_tip,
        |_, _, _, _| None,
        classify,
    );
    assert_eq!(
        state,
        Some(SupervisorActionableState::MergeBranches {
            branches: vec![(
                "cas-ready".into(),
                "factory/red-kite".into(),
                "factory/red-kite-tip".into(),
            )],
        }),
        "the held delivery is left out; its sibling is still named"
    );

    snapshot.in_progress_tasks.retain(|task| task.id != "cas-ready");
    let all_held = supervisor_actionable_state_with_classifiers(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        worker_branch_tip,
        |_, _, _, _| None,
        classify,
    );
    assert_eq!(all_held, None);
    assert!(!wake_for(all_held, now));
}

#[test]
fn merge_alert_names_the_per_task_branch_and_anchor_not_the_worker_base_cas_58d3() {
    // cas-247a parked on factory/loyal-owl-66-cas-247a @ 942a65e1 while
    // factory/loyal-owl-66 had moved on to cas-9069's commit 4a74016c.
    let tips: HashMap<&str, &str> = HashMap::from([
        ("factory/loyal-owl-66", "4a74016c"),
        ("factory/loyal-owl-66-cas-247a", "942a65e1"),
    ]);
    let resolve = |branch: &str| tips.get(branch).map(|tip| tip.to_string());
    let contains = |anchor: &str, tip: &str| anchor == tip;

    assert_eq!(
        task_delivery_ref("cas-247a", "loyal-owl-66", None, Some("942a65e1"), resolve, contains),
        ("factory/loyal-owl-66-cas-247a".into(), Some("942a65e1".into())),
    );
    // A stale recorded parked branch does not win over the branch that
    // actually contains the delivery anchor.
    assert_eq!(
        task_delivery_ref(
            "cas-247a",
            "loyal-owl-66",
            Some("factory/loyal-owl-66"),
            Some("942a65e1"),
            resolve,
            contains,
        ),
        ("factory/loyal-owl-66-cas-247a".into(), Some("942a65e1".into())),
    );
    // A recorded parked branch is used when there is no per-task branch.
    let parked_tips: HashMap<&str, &str> = HashMap::from([
        ("factory/loyal-owl-66", "4a74016c"),
        ("factory/handoff-branch", "942a65e1"),
    ]);
    assert_eq!(
        task_delivery_ref(
            "cas-247a",
            "loyal-owl-66",
            Some("factory/handoff-branch"),
            None,
            |branch: &str| parked_tips.get(branch).map(|tip| tip.to_string()),
            contains,
        ),
        ("factory/handoff-branch".into(), Some("942a65e1".into())),
    );
    // Without a per-task branch, record or anchor it is the worker branch.
    assert_eq!(
        task_delivery_ref("cas-9069", "loyal-owl-66", None, None, resolve, contains),
        ("factory/loyal-owl-66".into(), Some("4a74016c".into())),
    );
    // Nothing resolves: the worker branch is named with the anchor, if any.
    assert_eq!(
        task_delivery_ref("cas-x", "gone", None, Some("abc"), |_: &str| None, contains),
        ("factory/gone".into(), Some("abc".into())),
    );

    // End to end: the merge-now alert names the task's delivery, not the
    // worker's base branch.
    let now = Utc.with_ymd_and_hms(2026, 10, 2, 15, 50, 0).unwrap();
    let mut snapshot = held_delivery_snapshot();
    snapshot.in_progress_tasks.clear();
    snapshot.in_progress_tasks.push(task(
        "cas-247a",
        TaskStatus::AwaitingMerge,
        Some("loyal-owl-66"),
        Some("cas-epic"),
    ));
    let state = supervisor_actionable_state_with_classifiers(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        |task, worker| {
            task_delivery_ref(&task.id, worker, None, Some("942a65e1"), resolve, contains)
        },
        |_, _, _, _| None,
        |_, _| None,
    );
    assert_eq!(
        state,
        Some(SupervisorActionableState::MergeBranches {
            branches: vec![(
                "cas-247a".into(),
                "factory/loyal-owl-66-cas-247a".into(),
                "942a65e1".into(),
            )],
        }),
    );
}

/// cas-207e (after cas-6c13): a parked delivery whose lane CI is still running
/// is not a ready merge. Only green-CI deliveries are named, and pending ones
/// alone wake nobody.
#[test]
fn lane_ci_pending_delivery_is_not_a_ready_merge_cas_207e() {
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 14, 52, 0).unwrap();
    let mut snapshot = held_delivery_snapshot();
    snapshot.in_progress_tasks.push(task(
        "cas-green",
        TaskStatus::AwaitingMerge,
        Some("red-kite"),
        Some("cas-epic"),
    ));
    let pending_ci = |task: &TaskSummary, _: &str| {
        (task.id == "cas-held").then_some(DeliveryHold::LaneCiPending)
    };
    let state = supervisor_actionable_state_with_classifiers(
        &snapshot,
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        worker_branch_tip,
        |_, _, _, _| None,
        pending_ci,
    );
    assert_eq!(
        state,
        Some(SupervisorActionableState::MergeBranches {
            branches: vec![(
                "cas-green".into(),
                "factory/red-kite".into(),
                "factory/red-kite-tip".into(),
            )],
        }),
        "only the green-CI delivery is a ready merge"
    );

    let only_pending = supervisor_actionable_state_with_classifiers(
        &held_delivery_snapshot(),
        Some("cas-epic"),
        "supervisor",
        &HashSet::new(),
        now,
        600,
        worker_branch_tip,
        |_, _, _, _| None,
        |_, _| Some(DeliveryHold::LaneCiPending),
    );
    assert_eq!(only_pending, None);
    assert!(!wake_for(only_pending, now), "pending CI alone is no nudge");
}

/// cas-03ff: a pending `ci_completed` or `branch_contained_in` reminder the
/// supervisor set for this delivery.
fn delivery_reminder(event: &str, filter: serde_json::Value) -> cas_store::Reminder {
    serde_json::from_value(serde_json::json!({
        "id": 1,
        "owner_id": "id-supervisor",
        "target_id": "id-supervisor",
        "message": "merge when CI is green",
        "trigger_type": "event",
        "trigger_at": null,
        "trigger_event": event,
        "trigger_filter": filter,
        "status": "pending",
        "ttl_secs": 0,
        "created_at": "2026-10-16T00:00:00Z",
        "fired_at": null,
        "cancelled_at": null,
        "fired_event": null,
        "cross_session": true,
    }))
    .expect("reminder")
}

/// cas-03ff: deliveries with full-SHA tips, as production resolves them.
fn sha_tip(task: &TaskSummary, worker: &str) -> (String, Option<String>) {
    let tip = if task.id == "cas-waiting" {
        "0b04a99e8b56f99da4c0d0dd954599a1f11568f4"
    } else {
        "a4ac1d3b22a1da2b169da4268afc56691d2fb7ae"
    };
    (format!("factory/{worker}"), Some(tip.to_string()))
}

/// cas-03ff AC3: while the supervisor has a pending `ci_completed` or
/// `branch_contained_in` reminder for a parked delivery, that delivery is
/// waiting, not stalled. A reminder for one delivery leaves a sibling's
/// merge alarm alone, and a reminder that names neither the tip nor the
/// branch holds nothing.
#[test]
fn pending_ci_or_merge_reminder_quiets_the_stall_for_its_delivery_cas_03ff() {
    let now = Utc.with_ymd_and_hms(2026, 10, 16, 0, 40, 0).unwrap();
    let mut snapshot = data();
    snapshot.agents.push(AgentSummary {
        id: "id-supervisor".into(),
        ..worker("supervisor", now)
    });
    snapshot.in_progress_tasks.push(task(
        "cas-waiting",
        TaskStatus::AwaitingMerge,
        Some("gold-fox"),
        Some("cas-epic"),
    ));
    let state = |snapshot: &DirectorData| {
        supervisor_actionable_state_with_classifiers(
            snapshot,
            Some("cas-epic"),
            "supervisor",
            &HashSet::new(),
            now,
            600,
            sha_tip,
            |_, _, _, _| None,
            |_, _| None,
        )
    };
    assert!(state(&snapshot).is_some(), "no reminder: the parked delivery is a ready merge");

    for reminder in [
        delivery_reminder(
            "ci_completed",
            serde_json::json!({"sha": "0b04a99e8b56f99da4c0d0dd954599a1f11568f4"}),
        ),
        delivery_reminder(
            "branch_contained_in",
            serde_json::json!({"commit": "0b04a99e8", "target_branch": "epic/cas-epic"}),
        ),
        delivery_reminder(
            "branch_contained_in",
            serde_json::json!({"branch": "origin/factory/gold-fox", "target": "epic/cas-epic"}),
        ),
    ] {
        let mut waiting = snapshot.clone();
        waiting.reminders.push(reminder.clone());
        let held = state(&waiting);
        assert_eq!(held, None, "{:?}", reminder.trigger_filter);
        assert!(!wake_for(held, now), "waiting on CI or a merge is not stalled");

        let mut sibling = waiting.clone();
        sibling.in_progress_tasks.push(task(
            "cas-ready",
            TaskStatus::AwaitingMerge,
            Some("red-kite"),
            Some("cas-epic"),
        ));
        assert_eq!(
            state(&sibling),
            Some(SupervisorActionableState::MergeBranches {
                branches: vec![(
                    "cas-ready".into(),
                    "factory/red-kite".into(),
                    "a4ac1d3b22a1da2b169da4268afc56691d2fb7ae".into(),
                )],
            }),
            "only the watched delivery is held"
        );
    }

    for (label, reminder) in [
        (
            "another commit",
            delivery_reminder(
                "ci_completed",
                serde_json::json!({"sha": "1111111111111111111111111111111111111111"}),
            ),
        ),
        (
            "not an external wait",
            delivery_reminder("task_completed", serde_json::json!({"task_id": "cas-waiting"})),
        ),
        ("fired", {
            let mut fired = delivery_reminder(
                "ci_completed",
                serde_json::json!({"sha": "0b04a99e8b56f99da4c0d0dd954599a1f11568f4"}),
            );
            fired.status = cas_store::ReminderStatus::Fired;
            fired
        }),
        ("someone else's", {
            let mut other = delivery_reminder(
                "ci_completed",
                serde_json::json!({"sha": "0b04a99e8b56f99da4c0d0dd954599a1f11568f4"}),
            );
            other.target_id = "id-red-kite".into();
            other
        }),
    ] {
        let mut unrelated = snapshot.clone();
        unrelated.reminders.push(reminder);
        assert!(state(&unrelated).is_some(), "{label} reminder holds nothing");
    }
}
