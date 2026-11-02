//! cas-ee9ab: a slow store never extends a daemon loop pass.
use super::super::super::imports::*;
use super::{DirectorRefresh, ReadGate};
use std::sync::Arc;

/// A test daemon on real job lanes (the provisioning fixture runs queue jobs
/// inline, which would put every job back on the loop).
fn daemon_on_lanes(cas_dir: &std::path::Path) -> FactoryDaemon {
    let mut daemon = super::super::provisioning_tests::daemon(cas_dir);
    daemon.queue_jobs = super::super::prompt_queue_jobs::QueueJobs::start(
        cas_dir.to_path_buf(),
        daemon.app.project_scope(),
    );
    daemon
}

/// The pass latency #1165 asks for.
const PASS_LATENCY_LIMIT: Duration = Duration::from_millis(100);
/// The injected store delay.
const SLOW_READ: Duration = Duration::from_millis(500);

#[tokio::test]
async fn r1_approved_verdict_leaves_close_retry_with_worker() {
    r1_verification_wait_wakes("verdict").await;
}

#[tokio::test]
async fn r1_expired_dispatch_wakes_supervisor_once() {
    for release in ["deadline", "close-timeout", "sweep-before-close-timeout"] {
        r1_verification_wait_wakes(release).await;
    }
}

async fn r1_verification_wait_wakes(release: &str) {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::{
        LifecycleTransition, emit_verification_dispatch_handoff,
    };
    use cas_types::{Agent, AgentRole, TaskStatus};
    let mut fixture = crate::ui::factory::app::merge_alert_tests::Fixture::new();
    let root = fixture.app.cas_dir().to_path_buf();
    let lead = Agent::new_with_role(
        "lead-id".into(),
        fixture.app.supervisor_name().into(),
        AgentRole::Supervisor,
    );
    crate::store::open_agent_store(&root)
        .unwrap()
        .register(&lead)
        .unwrap();
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    let mut task = tasks.get("cas-9efb-test").unwrap();
    // This wait exercises a repository-independent proof, not the merge fixture's selector.
    task.deliverables.work_target = None;
    tasks.update(&task).unwrap();
    let deadline = chrono::Utc::now() + chrono::Duration::minutes(10);
    let dispatch = cas_store::create_verification_dispatch(
        &root,
        "cas-9efb-test",
        "merge-w",
        &lead.id,
        deadline,
    )
    .unwrap();
    task.status = TaskStatus::Blocked;
    task.pending_verification = true;
    task.notes = format!(
        "[{}] 🚫 BLOCKER {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        crate::mcp::tools::core::task::block_hold::verification_wait_note(&dispatch.id)
    );
    tasks.update(&task).unwrap();
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    emit_verification_dispatch_handoff(
        &root,
        queue.as_ref(),
        &dispatch.id,
        &task.id,
        &lead.id,
        deadline,
        "merge-w",
        None,
        None,
        None,
    )
    .unwrap();
    let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    core.push_task_lifecycle(
        &task.id,
        &task.title,
        TaskStatus::InProgress,
        TaskStatus::Blocked,
        "merge-w",
        None,
        LifecycleTransition::Blocked,
        &chrono::Utc::now().to_rfc3339(),
    )
    .unwrap();
    for _ in 0..3 {
        let idle = crate::ui::factory::director::DirectorEvent::WorkerIdle {
            worker: "merge-w".into(),
            active_task: None,
        };
        let mut inputs = fixture
            .app
            .delivery_request(std::slice::from_ref(&idle))
            .read(None);
        inputs.set_idle_liveness(
            [(
                "merge-w".into(),
                Some(crate::mcp::tools::service::worker_liveness::Observation {
                    state: crate::mcp::tools::service::worker_liveness::Liveness::WaitingForInput,
                    evidence: "recorded task_complete".into(),
                }),
            )]
            .into(),
        );
        let (_, prompts, _) = fixture
            .app
            .revalidate_and_prompt_for_delivery_from(&[idle], inputs);
        assert!(prompts.is_empty());
        core.push_task_lifecycle(
            &task.id,
            &task.title,
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            "merge-w",
            None,
            LifecycleTransition::Blocked,
            &chrono::Utc::now().to_rfc3339(),
        )
        .unwrap();
    }
    let rows = queue.peek_all(20).unwrap();
    // pin: dispatch handoff is the only waking source during this owned episode.
    assert_eq!(
        rows.iter()
            .filter(|row| !crate::supervisor_message_wakes::quiet(&row.prompt))
            .count(),
        1,
        "the dispatch handoff is the sole wake while verification owns the next step"
    );
    assert_eq!(rows.len(), 2);
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::reconcile_blocked_task_alert;
    reconcile_blocked_task_alert(&root, &task.id, deadline - chrono::Duration::seconds(1)).unwrap();
    assert_eq!(queue.peek_all(20).unwrap().len(), 2);
    if release == "verdict" {
        fixture._env.set("CAS_AGENT_ROLE", "supervisor");
        core.set_agent_id_for_testing(lead.id.clone());
        let result = core
            .cas_verification_add(rmcp::handler::server::wrapper::Parameters(
                serde_json::from_value(serde_json::json!({
                    "task_id": task.id,
                    "dispatch_id": dispatch.id,
                    "status": "approved",
                    "summary": "Review passed; worker can retry close."
                }))
                .unwrap(),
            ))
            .await
            .unwrap();
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let rows = queue.peek_all(20).unwrap();
        assert_eq!(
            rows.iter().filter(|row| row.target == "merge-w").count(),
            1,
            "the real verdict path must queue the worker's close retry: {rows:?}"
        );
    }
    let released_at = if release != "verdict" {
        deadline + chrono::Duration::seconds(1)
    } else {
        chrono::Utc::now()
    };
    if release == "sweep-before-close-timeout" {
        crate::ui::factory::daemon::runtime::lifecycle::escalate_expired_verification_dispatches(
            &root, released_at,
        );
    }
    if release.contains("close-timeout") {
        // The close retry persists this transition before the daemon sees it.
        let timed_out = cas_store::timeout_verification_dispatch(&root, &task.id, released_at)
            .unwrap().unwrap();
        assert_eq!(timed_out.state, cas_types::VerificationDispatchState::TimedOut);
    }
    for _ in 0..3 {
        reconcile_blocked_task_alert(&root, &task.id, released_at).unwrap();
        crate::ui::factory::daemon::runtime::lifecycle::escalate_expired_verification_dispatches(
            &root,
            released_at,
        );
    }
    let rows = queue.peek_all(20).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.target != "merge-w"
                && !crate::supervisor_message_wakes::quiet(&row.prompt))
            .count(),
        if release != "verdict" { 2 } else { 1 },
        "after the dispatch handoff, expiry needs one wake and approval needs zero: {rows:?}"
    );
    if release == "verdict" {
        let resolved_at = cas_store::get_latest_verification_dispatch(&root, &task.id)
            .unwrap().unwrap().resolved_at.unwrap();
        let close_deadline = resolved_at + chrono::Duration::minutes(5);
        reconcile_blocked_task_alert(&root, &task.id, close_deadline - chrono::Duration::seconds(1)).unwrap();
        assert_eq!(queue.peek_all(20).unwrap().len(), rows.len(),
            "approval gives the worker a bounded chance to retry close");
        for _ in 0..3 {
            reconcile_blocked_task_alert(&root, &task.id, close_deadline).unwrap();
            crate::ui::factory::daemon::runtime::lifecycle::escalate_expired_verification_dispatches(
                &root, close_deadline,
            );
        }
        let rows = queue.peek_all(20).unwrap();
        assert_eq!(rows.iter().filter(|row| row.target != "merge-w"
            && !crate::supervisor_message_wakes::quiet(&row.prompt)).count(), 2,
            "an unretried approval must escalate exactly once after its grace period: {rows:?}");
    }
}

#[tokio::test]
async fn b8916_owned_verification_block_is_a_non_waking_receipt() {
    use cas_types::{Agent, AgentRole, TaskStatus};
    for cause in ["owned", "foreign", "expired", "independent", "successor"] {
        let mut fixture = crate::ui::factory::app::merge_alert_tests::Fixture::new();
        let cas_dir = fixture.app.cas_dir().to_path_buf();
        let supervisor = Agent::new_with_role(
            "lead-id".into(),
            fixture.app.supervisor_name().into(),
            AgentRole::Supervisor,
        );
        crate::store::open_agent_store(&cas_dir)
            .unwrap()
            .register(&supervisor)
            .unwrap();
        if cause == "successor" {
            let mut successor = supervisor.clone();
            successor.id = "successor-id".into();
            successor.registered_at += chrono::Duration::seconds(1);
            crate::store::open_agent_store(&cas_dir)
                .unwrap()
                .register(&successor)
                .unwrap();
        }
        let dispatch = cas_store::create_verification_dispatch(
            &cas_dir,
            "cas-9efb-test",
            "merge-w",
            if cause == "foreign" {
                "other-lead"
            } else {
                "lead-id"
            },
            chrono::Utc::now()
                + chrono::Duration::minutes(if cause == "expired" { -1 } else { 10 }),
        )
        .unwrap();
        let store = crate::store::open_task_store_local(&cas_dir).unwrap();
        let mut task = store.get("cas-9efb-test").unwrap();
        task.status = TaskStatus::Blocked;
        task.notes = format!(
            "[{}] 🚫 BLOCKER Verification required: exact dispatch {} must record a verdict before close.",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            dispatch.id
        );
        store.update(&task).unwrap();
        if cause == "independent" {
            store.patch_execution_state(&task.id, &serde_json::json!({"blocked_hold": {
                "reason": "Operator must restore credentials", "who": "merge-w", "when": chrono::Utc::now()
            }})).unwrap();
        }
        let event = crate::ui::factory::director::DirectorEvent::TaskBlocked {
            task_id: task.id,
            task_title: task.title,
            worker: "merge-w".into(),
        };
        let (_, prompts, _) = fixture
            .app
            .revalidate_and_prompt_for_delivery(std::slice::from_ref(&event));
        assert!(prompts.is_empty());
        let core = crate::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
        use crate::mcp::tools::core::task::lifecycle::supervisor_push::{
            LifecycleTransition, is_lifecycle_wake_source,
        };
        core.push_task_lifecycle(
            "cas-9efb-test",
            "verification wait",
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            "merge-w",
            None,
            LifecycleTransition::Blocked,
            &chrono::Utc::now().to_rfc3339(),
        )
        .unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        let owned = matches!(cause, "owned" | "expired");
        // pin: owned dispatches use their handoff/expiry relay instead of a generic blocker wake.
        assert_eq!(
            is_lifecycle_wake_source(&rows[0].source),
            !owned,
            "{cause}: lifecycle routing must use the same sole-blocker classification"
        );
        // pin: an owned dispatch keeps its generic receipt quiet even after expiry.
        assert_eq!(
            crate::supervisor_message_wakes::quiet(&rows[0].prompt),
            owned
        );
        use crate::mcp::tools::core::task::lifecycle::supervisor_push::reconcile_blocked_task_alert;
        for _ in 0..3 {
            reconcile_blocked_task_alert(&cas_dir, "cas-9efb-test", chrono::Utc::now()).unwrap();
        }
        assert_eq!(queue.peek_all(10).unwrap().len(), 1);
        for _ in 0..3 {
            reconcile_blocked_task_alert(
                &cas_dir,
                "cas-9efb-test",
                chrono::Utc::now() + chrono::Duration::minutes(20),
            )
            .unwrap();
        }
        assert_eq!(queue.peek_all(10).unwrap().len(), if owned { 1 } else { 2 });
    }
}

#[tokio::test]
async fn answered_blocker_stays_quiet_across_verifier_reject_and_reblock_cas_4e8e() {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::{LifecycleTransition, is_lifecycle_wake_source};
    use cas_types::{Agent, AgentRole, TaskStatus, Verification, VerificationStatus};
    use rmcp::handler::server::wrapper::Parameters;
    let mut fixture = crate::ui::factory::app::merge_alert_tests::Fixture::new();
    // Explicit reply validation recognizes the supervisor's registered name
    // through its normal factory environment as well as the logical alias.
    fixture._env.set("CAS_AGENT_NAME", fixture.app.supervisor_name());
    let root = fixture.app.cas_dir().to_path_buf();
    let agents = crate::store::open_agent_store(&root).unwrap();
    let lead = Agent::new_with_role("lead-id".into(), fixture.app.supervisor_name().into(), AgentRole::Supervisor);
    agents.register(&lead).unwrap();
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    let mut task = tasks.get("cas-9efb-test").unwrap();
    let block = |task: &mut cas_types::Task, reason: &str| {
        task.status = TaskStatus::Blocked;
        tasks.update(task).unwrap();
        tasks.patch_execution_state(&task.id, &serde_json::json!({"blocked_hold": {
            "reason": reason, "who": "merge-w", "when": chrono::Utc::now()
        }})).unwrap();
    };
    block(&mut task, "Awaiting operator decision");
    let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    core.set_agent_id_for_testing(lead.id.clone());
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    core.push_task_lifecycle(&task.id, &task.title, TaskStatus::InProgress, TaskStatus::Blocked,
        "merge-w", None, LifecycleTransition::Blocked, &chrono::Utc::now().to_rfc3339()).unwrap();
    assert!(queue.peek_all(20).unwrap().iter().any(|row| is_lifecycle_wake_source(&row.source)),
        "an unanswered blocker must still wake its supervisor");
    let service = crate::mcp::CasService::new(core.clone(), None);
    let worker_core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
    worker_core.set_agent_id_for_testing("merge-w".into());
    let worker_service = crate::mcp::CasService::new(worker_core, None);
    let ask = worker_service.coordination(Parameters(serde_json::from_value(serde_json::json!({
        "action": "message", "target": "supervisor", "task_id": task.id, "blocker": true,
        "summary": "Operator decision needed", "message": "Which option should I implement?"
    })).unwrap())).await.unwrap();
    assert!(!ask.is_error.unwrap_or(false), "{ask:?}");
    let blocker_id = queue.peek_all(30).unwrap().into_iter()
        .find(|row| crate::prompt_revalidation::parse_blocker_envelope(&row.prompt).is_some()).unwrap().id;
    let status = worker_service.coordination(Parameters(serde_json::from_value(serde_json::json!({
        "action": "message", "target": "supervisor", "task_id": task.id,
        "summary": "Progress", "message": "The completed checks are recorded."
    })).unwrap())).await.unwrap();
    assert!(!status.is_error.unwrap_or(false), "{status:?}");
    let status_id = queue.peek_all(30).unwrap().into_iter()
        .filter(|row| row.source == "merge-w").max_by_key(|row| row.id).unwrap().id;
    // Neither an unrelated direct ping nor a reply to ordinary status owns the blocker.
    for reply_to in [None, Some(status_id)] {
        let ping = service.coordination(Parameters(serde_json::from_value(serde_json::json!({
            "action": "message", "target": "merge-w", "task_id": task.id, "in_reply_to": reply_to,
            "summary": "Status query", "message": "Are your checks complete?"
        })).unwrap())).await.unwrap();
        assert!(!ping.is_error.unwrap_or(false), "{ping:?}");
        let event = crate::ui::factory::director::DirectorEvent::TaskBlocked {
            task_id: task.id.clone(), task_title: task.title.clone(), worker: "merge-w".into(),
        };
        assert_eq!(fixture.app.revalidate_and_prompt_for_delivery(&[event]).0.len(), 1,
            "an unrelated supervisor message must leave the blocker armed (reply_to={reply_to:?})");
    }
    let answered = service.coordination(Parameters(serde_json::from_value(serde_json::json!({
        "action": "message", "target": "merge-w",
        "in_reply_to": blocker_id, "summary": "Decision relayed", "message": "I relayed your question to the operator; wait for their decision."
    })).unwrap())).await.unwrap();
    assert!(!answered.is_error.unwrap_or(false), "{answered:?}");
    let before_answer = queue.peek_all(20).unwrap().into_iter()
        .find(|row| is_lifecycle_wake_source(&row.source)).unwrap();
    assert!(super::super::queue_and_events::read_row_facts(
        &root, queue.as_ref(), Some(tasks.as_ref()), &agents.list(None).unwrap(), &before_answer,
    ).answered_blocker, "already-queued blocker wake must also be withdrawn after the answer");
    let idle_id = queue.enqueue_urgent_with_outcome(
        "lifecycle-wake:worker-attention:42", fixture.app.supervisor_name(),
        "<worker-attention kind=\"worker_idle\" worker=\"merge-w\" notification_id=\"42\">Idle episode restarted</worker-attention>",
        None, None, None, false, Some(&cas_store::QueueOrigin::Daemon),
    ).unwrap().id();
    let idle_row = queue.peek_all(30).unwrap().into_iter().find(|row| row.id == idle_id).unwrap();
    assert!(super::super::queue_and_events::read_row_facts(
        &root, queue.as_ref(), Some(tasks.as_ref()), &agents.list(None).unwrap(), &idle_row,
    ).answered_blocker);
    for row in queue.peek_all(30).unwrap() {
        if row.id != before_answer.id && row.id != idle_id { queue.mark_processed(row.id).unwrap(); }
    }
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    std::mem::swap(&mut daemon.app, &mut fixture.app);
    daemon.process_prompt_queue().await.unwrap();
    assert!(daemon.finish_shutdown_writes(super::super::store_worker::global(), Duration::from_secs(10)));
    for id in [before_answer.id, idle_id] {
        let report = queue.message_delivery_report(id).unwrap().unwrap();
        assert!(report.delivered_at.is_none());
        if id == before_answer.id {
            // pin: an outstanding blocker wake is parked after its explicit answer.
            assert_eq!(report.pending_detail.as_deref(), Some("answered blocker; readable without another supervisor wake"));
        }
    }
    // 29da493b3 / 5092b8d5f: routine worker_idle rows bypass daemon delivery
    // entirely. Prove the digest and its full-read escape instead of expecting
    // a parking stamp on a row QueueSelect intentionally never selects.
    let digest = queue.surface_for_recipient_within_budget(
        fixture.app.supervisor_name(), None, cas_store::HookDrain::NoticeDigest, 30, 0,
    ).unwrap().rows;
    assert_eq!(digest.iter().map(|row| row.id).collect::<Vec<_>>(), vec![idle_id]);
    let explicit = queue.poll_named_for_recipient(fixture.app.supervisor_name(), None, &[idle_id]).unwrap();
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].prompt, idle_row.prompt);
    assert_eq!(queue.peek_all(30).unwrap().into_iter().find(|row| row.id == before_answer.id).unwrap().prompt,
        before_answer.prompt);
    // Release the held lifecycle alert after the worker resumes: it must be
    // retired as stale, never transported as a fresh blocked notification.
    task.status = TaskStatus::InProgress;
    tasks.update(&task).unwrap();
    queue.park_for_busy_recipient(before_answer.id, None,
        chrono::Utc::now() - chrono::Duration::seconds(1)).unwrap();
    daemon.process_prompt_queue().await.unwrap();
    assert!(daemon.finish_shutdown_writes(super::super::store_worker::global(), Duration::from_secs(10)));
    let retired = queue.message_delivery_report(before_answer.id).unwrap().unwrap();
    assert!(retired.delivered_at.is_none(), "a stale held blocker must never be transported");
    assert!(!queue.peek_all(30).unwrap().iter().any(|row| row.id == before_answer.id),
        "a held blocker must leave the pending queue once the task leaves Blocked: {retired:?}");
    block(&mut task, "Awaiting operator decision");
    std::mem::swap(&mut daemon.app, &mut fixture.app);

    for rejected in [false, true] {
        if rejected {
            let mut verdict = Verification::new(task.id.clone(), "reviewer".into());
            verdict.status = VerificationStatus::Rejected;
            verdict.summary = "Operator decision remains outstanding; re-open to finish after the answer.".into();
            core.return_rejected_merged_delivery(&task.id, &verdict);
            task = tasks.get(&task.id).unwrap();
            assert_eq!(task.status, TaskStatus::Open);
            task.status = TaskStatus::InProgress;
            tasks.update(&task).unwrap();
            block(&mut task, "Awaiting operator decision");
        }
        let events = [
            crate::ui::factory::director::DirectorEvent::WorkerIdle { worker: "merge-w".into(), active_task: None },
            crate::ui::factory::director::DirectorEvent::TaskBlocked {
                task_id: task.id.clone(), task_title: task.title.clone(), worker: "merge-w".into(),
            },
        ];
        let (current, prompts, _) = fixture.app.revalidate_and_prompt_for_delivery(&events);
        assert!(current.is_empty() && prompts.is_empty(),
            "answered blocker must suppress idle and director-blocked wakes (rejected={rejected}): {prompts:?}");
        for row in queue.peek_all(30).unwrap() { queue.mark_processed(row.id).unwrap(); }
        core.push_task_lifecycle(&task.id, &task.title, TaskStatus::InProgress, TaskStatus::Blocked,
            "merge-w", None, LifecycleTransition::Blocked, &chrono::Utc::now().to_rfc3339()).unwrap();
        let rows = queue.peek_all(30).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!is_lifecycle_wake_source(&rows[0].source));
        assert!(crate::supervisor_message_wakes::quiet(&rows[0].prompt));
    }
    let asked = worker_service.coordination(Parameters(serde_json::from_value(serde_json::json!({
        "action": "message", "target": "supervisor", "task_id": task.id, "blocker": true,
        "summary": "New operator question", "message": "The operator must now decide a different question."
    })).unwrap())).await.unwrap();
    assert!(!asked.is_error.unwrap_or(false), "{asked:?}");
    let event = crate::ui::factory::director::DirectorEvent::TaskBlocked {
        task_id: task.id.clone(), task_title: task.title.clone(), worker: "merge-w".into(),
    };
    let (events, prompts, _) = fixture.app.revalidate_and_prompt_for_delivery(&[event]);
    assert_eq!(events.len(), 1);
    assert!(prompts.is_empty());
    let answered_again = service.coordination(Parameters(serde_json::from_value(serde_json::json!({
        "action": "message", "target": "merge-w", "task_id": task.id,
        "kind": "answer", "summary": "New question relayed", "message": "I also relayed your new question; wait for that answer."
    })).unwrap())).await.unwrap();
    assert!(!answered_again.is_error.unwrap_or(false), "{answered_again:?}");
    let reply_id = queue.peek_all(30).unwrap().into_iter()
        .filter(|row| row.source == "supervisor" && row.target == "merge-w")
        .max_by_key(|row| row.id).unwrap().id;
    let repeated = worker_service.coordination(Parameters(serde_json::from_value(serde_json::json!({
        "action": "message", "target": "supervisor", "task_id": task.id, "blocker": true,
        "in_reply_to": reply_id, "summary": "Same pending question",
        "message": "The operator must now decide a different question."
    })).unwrap())).await.unwrap();
    assert!(!repeated.is_error.unwrap_or(false), "{repeated:?}");
    let event = crate::ui::factory::director::DirectorEvent::TaskBlocked {
        task_id: task.id.clone(), task_title: task.title.clone(), worker: "merge-w".into(),
    };
    assert!(fixture.app.revalidate_and_prompt_for_delivery(&[event]).1.is_empty());
    // A different hold is a new question even on the same task and assignee.
    block(&mut task, "Credentials are now missing");
    let event = crate::ui::factory::director::DirectorEvent::TaskBlocked {
        task_id: task.id.clone(), task_title: task.title.clone(), worker: "merge-w".into(),
    };
    let (events, prompts, _) = fixture.app.revalidate_and_prompt_for_delivery(&[event]);
    assert_eq!(events.len(), 1);
    assert!(prompts.is_empty());
    let opened = service.task(Parameters(serde_json::from_value(serde_json::json!({
        "action": "update", "id": task.id, "status": "open"
    })).unwrap())).await.unwrap();
    assert!(!opened.is_error.unwrap_or(false), "{opened:?}");
    assert!(tasks.get_execution_state(&task.id).unwrap().unwrap().get("answered_blocker").is_none());
}

// a6be40dba / 1ecb22e52 deliberately replace direct green Teams/PTY
// fallbacks with one durable readiness episode. Reproduce the original race
// after the real batch check, then drive that queue through its final live read.
#[tokio::test]
async fn cas_91bab_merge_after_batch_check_never_reaches_teammate_inbox() {
    use super::super::supervisor_single_turn_tests::{attach_supervisor_pty, fixture_child, teams_messages};
    if !fixture_child("ui::factory::daemon::runtime::director_refresh::tests::cas_91bab_merge_after_batch_check_never_reaches_teammate_inbox") {
        return;
    }
    for cli in [cas_mux::SupervisorCli::Claude, cas_mux::SupervisorCli::Codex] {
        for outcome in ["live", "merged", "declined"] {
            let mut fixture = crate::ui::factory::app::merge_alert_tests::Fixture::new();
            let cas_dir = fixture.app.cas_dir().to_path_buf();
            let root = cas_dir.parent().unwrap();
            fixture.app.set_supervisor_cli_for_test(cli);
            let (_, mut prompts, _) = fixture.app.revalidate_and_prompt_for_delivery(
                std::slice::from_ref(&fixture.event),
            );
            assert_eq!(prompts.len(), 1);
            let mut daemon = daemon_on_lanes(&cas_dir);
            std::mem::swap(&mut daemon.app, &mut fixture.app);
            fixture._env.set("CAS_FACTORY_SESSION", &daemon.session_name);
            attach_supervisor_pty(&mut daemon, root, cli, true).await;

            assert!(prompts[0].is_merge_ready());
            let expected_text = prompts[0].text.lines().next().unwrap().to_string();
            assert_eq!(daemon.app.deliverability_request().read(None, &mut prompts), vec![true],
                "the merge must be genuinely outstanding at the batch check");
            let mut refresh = DirectorRefresh::new();
            refresh.start_deliverability_check(
                &daemon.app, &mut daemon.queue_jobs, prompts,
                vec![fixture.event.clone()], Instant::now(),
            ).unwrap();
            let batch = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Some(batch) = refresh.take_deliverability(&mut daemon.queue_jobs).pop() {
                        break batch;
                    }
                    tokio::task::yield_now().await;
                }
            }).await.expect("batch recheck must finish");
            assert!(batch.prompts[0].1);

            // The target/task changes only after the successful async verdict.
            match outcome {
                "merged" => fixture.advance_target(true),
                "declined" => {
                    let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
                    let mut task = tasks.get("cas-9efb-test").unwrap();
                    task.deliverables.decline_factory_branch_anchor();
                    task.status = cas_types::TaskStatus::Open;
                    tasks.update(&task).unwrap();
                }
                _ => {}
            }
            let writer = super::super::store_worker::global();
            daemon.inject_rechecked_batch(batch, writer).await;
            assert!(writer.wait_idle(Duration::from_secs(10)));
            let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
            let rows = queue.peek_all(10).unwrap();
            assert_eq!(rows.len(), 1, "one durable readiness episode: {cli:?}/{outcome}");
            let id = rows[0].id;
            assert!(rows[0].prompt.contains(&expected_text));
            assert!(teams_messages(&daemon).is_empty(), "green readiness uses the queue");

            let completed = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    daemon.process_prompt_queue().await.unwrap();
                    let report = queue.message_delivery_report(id).unwrap().unwrap();
                    let written = std::fs::read_to_string(root.join("input")).unwrap();
                    if outcome == "live" {
                        if report.delivered_at.is_some() && written.contains(&expected_text)
                            && written.ends_with('\r') && !daemon.app.mux.has_pending_injects()
                        {
                            break (report, written);
                        }
                    } else if report.pending_reason == Some(cas_store::PendingReason::SupersededStale) {
                        break (report, written);
                    }
                    tokio::task::yield_now().await;
                }
            }).await.unwrap_or_else(|_| panic!("queue did not settle: {cli:?}/{outcome}: {:?}",
                queue.message_delivery_report(id).unwrap()));
            let (report, written) = completed;
            assert_eq!(report.delivered_at.is_some(), outcome == "live", "{cli:?}/{outcome}: {report:?}");
            assert_eq!(written.matches(&expected_text).count(), usize::from(outcome == "live"),
                "only the live merge demand may reach the actual supervisor consumer: {cli:?}/{outcome}: {written:?}");
            assert!(report.confirmed_at.is_none(), "transport is not an observed model turn");
            assert!(teams_messages(&daemon).is_empty(), "no duplicate teammate body");
            assert!(daemon.finish_shutdown_writes(writer, Duration::from_secs(10)));
        }
    }
}

// Non-green decisions still use native Teams and PTY transport. Keep these
// fallback and retraction contracts alongside the green durable-queue cases.
#[tokio::test]
async fn cas_91bab_non_green_direct_delivery_fallbacks() {
    use super::super::supervisor_single_turn_tests::fixture_child;
    if !fixture_child("ui::factory::daemon::runtime::director_refresh::tests::cas_91bab_non_green_direct_delivery_fallbacks") {
        return;
    }
    let mut shutdown_delivered = false;
    let mut stale_fallback_suppressed = false;
    for scenario in [
        "superseded",
        "missing",
        "shutdown",
        "merged_fallback",
        "superseded_refused",
        "superseded_unreported",
    ] {
        let synchronous_fallback =
            matches!(scenario, "superseded_refused" | "superseded_unreported");
        let mut fixture = crate::ui::factory::app::merge_alert_tests::Fixture::new();
        fixture.ci(Some("failure"));
        let cas_dir = fixture.app.cas_dir().to_path_buf();
        // A dropped incident must count as zero rows, not an uninitialized schema.
        let _queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        fixture
            .app
            .set_supervisor_cli_for_test(if synchronous_fallback {
                cas_mux::SupervisorCli::Codex
            } else {
                cas_mux::SupervisorCli::Claude
            });
        let (_, mut prompts, _) = fixture
            .app
            .revalidate_and_prompt_for_delivery(std::slice::from_ref(&fixture.event));
        assert_eq!(prompts.len(), 1);
        // Red CI keeps the direct transport route after a6be40dba / 1ecb22e52.
        assert!(
            prompts[0]
                .text
                .contains("CI failed")
        );
        assert!(!prompts[0].is_merge_ready());
        assert!(prompts[0].retract_task.is_some());
        let verdicts = fixture
            .app
            .deliverability_request()
            .read(None, &mut prompts);
        assert_eq!(
            verdicts,
            vec![true],
            "the merge is genuinely outstanding at the batch check"
        );

        let mut daemon = daemon_on_lanes(&cas_dir);
        std::mem::swap(&mut daemon.app, &mut fixture.app);
        let teams = super::super::teams::TeamsManager::new(&daemon.session_name);
        teams
            .init_team_config(&[], cas_dir.parent().unwrap(), &Default::default(), "lead")
            .unwrap();
        let inbox = teams.teams_dir().join("inboxes/supervisor.json");
        daemon.teams = Some(teams);

        if scenario == "superseded" || synchronous_fallback {
            let store = crate::store::open_task_store_local(&cas_dir).unwrap();
            let mut task = store.get("cas-9efb-test").unwrap();
            task.deliverables.decline_factory_branch_anchor();
            task.status = cas_types::TaskStatus::Open;
            store.update(&task).unwrap();
        }
        // Exercise the real batch job: relabelled prompts must survive its handoff.
        let mut refresh = DirectorRefresh::new();
        let batch = if synchronous_fallback {
            None
        } else {
            refresh
                .start_deliverability_check(
                    &daemon.app,
                    &mut daemon.queue_jobs,
                    prompts.clone(),
                    vec![fixture.event.clone()],
                    Instant::now(),
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let batch = loop {
                let mut batches = refresh.take_deliverability(&mut daemon.queue_jobs);
                if let Some(batch) = batches.pop() {
                    break batch;
                }
                assert!(Instant::now() < deadline, "batch recheck did not complete");
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            if scenario == "superseded" {
                // pin: this failure identifies the missing superseded-body batch handoff.
                assert!(
                    batch.prompts[0].1,
                    "superseded prompt must survive the batch handoff"
                );
            }
            Some(batch)
        };
        let expected_task = prompts[0].retract_task.clone();
        let expected_text = prompts[0].text.lines().next().unwrap().to_string();
        let capture = cas_dir.parent().unwrap().join("input");
        if matches!(scenario, "missing" | "shutdown" | "merged_fallback") || synchronous_fallback {
            super::super::supervisor_single_turn_tests::attach_supervisor_pty(
                &mut daemon,
                cas_dir.parent().unwrap(),
                if synchronous_fallback { cas_mux::SupervisorCli::Codex } else { cas_mux::SupervisorCli::Claude },
                true,
            ).await;
            let tree = daemon.teams.as_ref().unwrap().teams_dir().to_path_buf();
            std::fs::remove_dir_all(&tree).unwrap();
            // A missing live tree with no saved roster exercises the real ENOENT path.
            daemon.teams = Some(super::super::teams::TeamsManager::rooted_at(
                tree,
                &daemon.session_name,
            ));
        }

        if synchronous_fallback {
            let store_worker = super::super::store_worker::global();
            if scenario == "superseded_refused" {
                daemon.queue_jobs.close_lanes_for_test();
                daemon
                    .deliver_director_events(
                        &mut refresh,
                        vec![fixture.event.clone()],
                        (vec![fixture.event.clone()], prompts, None),
                        Instant::now(),
                        0.0,
                        store_worker,
                        &super::super::loop_watchdog::LoopProgress::new(),
                    )
                    .await;
            } else {
                // The real read job cannot report until shutdown has used its
                // unchecked fallback; no scheduling race or timed sleep.
                let gate = Arc::new(ReadGate::default());
                refresh = DirectorRefresh::new().with_read_gate(Arc::clone(&gate));
                refresh
                    .start_deliverability_check(
                        &daemon.app,
                        &mut daemon.queue_jobs,
                        prompts,
                        vec![fixture.event.clone()],
                        Instant::now(),
                    )
                    .unwrap();
                daemon
                    .drain_director_deliveries_for_shutdown(
                        &mut refresh,
                        store_worker,
                        Duration::ZERO,
                    )
                    .await;
                gate.open();
                let deadline = Instant::now() + Duration::from_secs(10);
                while daemon.queue_jobs.external_completions_ready() == 0 {
                    assert!(Instant::now() < deadline, "late recheck did not finish");
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                // The original pending batch was already consumed by shutdown.
                assert!(
                    refresh
                        .take_deliverability(&mut daemon.queue_jobs)
                        .is_empty()
                );
            }
            let name = daemon.app.supervisor_name().to_string();
            // A Codex inject retains the submit boundary after writing its body.
            // Wait for ownership to clear before issuing the marker; otherwise
            // inject correctly defers it and there is nothing for cat to drain.
            let deadline = Instant::now() + Duration::from_secs(2);
            while daemon.app.mux.pane_submit_pending(&name) && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(!daemon.app.mux.pane_submit_pending(&name));
            let marker = "sync-fallback-check-finished";
            assert_eq!(
                daemon.app.mux.inject(&name, marker).await.unwrap(),
                cas_mux::InjectOutcome::Delivered
            );
            while !std::fs::read_to_string(&capture)
                .unwrap_or_default()
                .contains(marker)
                && Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let written = std::fs::read_to_string(&capture).unwrap_or_default();
            assert!(
                written.contains(marker),
                "PTY did not drain its complete body"
            );
            // pin: both direct transport fallbacks must preserve the Superseded label and remove merge instructions.
            assert!(written.contains("Superseded delivery"), "{scenario}: {written}");
            assert!(!written.contains("MERGE REQUIRED"), "{scenario}: {written}");
            assert!(daemon.finish_shutdown_writes(store_worker, Duration::from_secs(10)));
            continue;
        }
        daemon
            .inject_rechecked_batch(batch.unwrap(), super::super::store_worker::global())
            .await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while daemon.queue_jobs.external_completions_ready() == 0 {
            assert!(
                Instant::now() < deadline,
                "merge alert job did not complete"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if scenario == "merged_fallback" {
            // The failed native write already finished, but the PTY fallback has not run.
            fixture.advance_target(true);
        }
        if scenario != "shutdown" {
            daemon
                .director_refresh_pass(
                    &mut refresh,
                    false,
                    super::super::store_worker::global(),
                    &super::super::loop_watchdog::LoopProgress::new(),
                )
                .await;
        }
        daemon
            .drain_director_deliveries_for_shutdown(
                &mut refresh,
                super::super::store_worker::global(),
                Duration::from_secs(10),
            )
            .await;
        assert!(daemon.finish_shutdown_writes(
            super::super::store_worker::global(),
            Duration::from_secs(10)
        ));
        let rows: Vec<super::super::teams::InboxMessage> =
            serde_json::from_str(&std::fs::read_to_string(inbox).unwrap_or_else(|_| "[]".into()))
                .unwrap();
        if scenario == "superseded" {
            assert_eq!(
                rows.len(),
                1,
                "a superseded delivery must reach the teammate inbox"
            );
            assert_eq!(rows[0].retract_task, expected_task);
            // pin: request_changes must reach the supervisor as superseded, without a merge instruction.
            assert!(rows[0].text.contains("Superseded delivery"));
            // pin: this heading asks the supervisor to merge a superseded tip.
            assert!(!rows[0].text.contains("MERGE REQUIRED"));
        } else if scenario == "merged_fallback" {
            // A later marker on the same PTY proves all earlier queued payloads
            // have reached cat before absence of the stale body is judged.
            let name = daemon.app.supervisor_name().to_string();
            let deadline = Instant::now() + Duration::from_secs(2);
            while daemon.app.mux.pane_submit_pending(&name) && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let marker = "fallback-check-finished";
            daemon.app.mux.inject(&name, marker).await.unwrap();
            while !std::fs::read_to_string(&capture)
                .unwrap_or_default()
                .contains(marker)
                && Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let written = std::fs::read_to_string(&capture).unwrap_or_default();
            stale_fallback_suppressed =
                written.contains(marker) && !written.contains(&expected_text);

        } else {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !std::fs::read_to_string(&capture)
                .unwrap_or_default()
                .contains(&expected_text)
                && Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let delivered = std::fs::read_to_string(capture)
                .unwrap_or_default()
                .contains(&expected_text);
            if scenario == "shutdown" {
                shutdown_delivered = delivered;
            } else {
                assert!(
                    delivered,
                    "the genuine alert must fall back to the actual supervisor PTY"
                );
            }
            let db = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
            let incidents: i64 = db.query_row("SELECT COUNT(*) FROM prompt_queue WHERE source = 'teams-recovery' AND target = 'operator'", [], |row| row.get(0)).unwrap();
            if scenario == "shutdown" {
                shutdown_delivered &= incidents == 1;
            } else {
                assert_eq!(
                    incidents, 1,
                    "missing team files must publish one durable incident"
                );
            }
        }
    }
    assert!(
        shutdown_delivered,
        "shutdown must consume the completed native failure and deliver its PTY fallback plus incident (stale fallback suppressed: {stale_fallback_suppressed})"
    );
    assert!(
        stale_fallback_suppressed,
        "a merge after the native error must suppress the PTY fallback"
    );
}

#[tokio::test]
async fn cas_9efb_merge_ci_probe_runs_on_external_lane_without_blocking_control() {
    let mut fixture = crate::ui::factory::app::merge_alert_tests::Fixture::new();
    let cas_dir = fixture.app.cas_dir().to_path_buf();
    crate::test_paths::warm_stub(&cas_dir.join("gh-fixture"),
        "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '%s\\n' '{\"nameWithOwner\":\"acme/cas\"}'; else sleep 0.4; cat .cas/ci.json; fi\n");
    let mut daemon = daemon_on_lanes(&cas_dir);
    std::mem::swap(&mut daemon.app, &mut fixture.app);
    let mut refresh = DirectorRefresh::new();
    let started = Instant::now();
    refresh.start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![fixture.event.clone()]).unwrap();
    assert!(started.elapsed() < PASS_LATENCY_LIMIT, "submitting merge evidence blocked control");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut external_seen = false;
    let mut slowest = Duration::ZERO;
    let (events, inputs) = loop {
        assert!(Instant::now() < deadline, "merge evidence did not finish");
        let pass = Instant::now();
        let ready = refresh.take_delivery_read(&mut daemon.queue_jobs);
        slowest = slowest.max(pass.elapsed());
        external_seen |= daemon.queue_jobs.job_lanes().unwrap().lane_stats().into_iter().any(|stats| {
            stats.running.is_some_and(|job| {
                job.lane == super::super::jobs::Lane::External && job.key == super::super::factory_jobs::DELIVERY_READ_KEY
            })
        });
        if let Some(ready) = ready { break ready; }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert!(external_seen, "the actual merge read must use the external lane");
    let pass = Instant::now();
    let (_, prompts, _) = daemon.app.revalidate_and_prompt_for_delivery_from(&events, inputs);
    slowest = slowest.max(pass.elapsed());
    assert_eq!(prompts.len(), 1);
    assert!(slowest < PASS_LATENCY_LIMIT, "control waited for GitHub: {slowest:?}");
}

#[tokio::test]
async fn a_slow_director_read_never_extends_a_loop_pass_cas_ee9ab() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::super::loop_watchdog::LoopProgress::new();
    let store_worker = super::super::store_worker::global();
    let mut refresh = DirectorRefresh::new().with_read_delay(SLOW_READ);

    // Written before the refresh is due: the first snapshot must carry it.
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    store
        .add(&cas_types::Task::new(
            "cas-ee9a".into(),
            "visible after one refresh".into(),
        ))
        .unwrap();

    let started = Instant::now();
    let mut slowest = Duration::ZERO;
    let mut due = true;
    let mut visible_after = None;
    while started.elapsed() < Duration::from_secs(10) {
        let pass = Instant::now();
        daemon
            .director_refresh_pass(&mut refresh, due, store_worker, &progress)
            .await;
        slowest = slowest.max(pass.elapsed());
        due = false;
        if daemon
            .app
            .director_data()
            .ready_tasks
            .iter()
            .any(|task| task.id == "cas-ee9a")
        {
            visible_after = Some(started.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let visible_after = visible_after.expect("the refresh never reached the panel data");
    assert!(
        visible_after >= SLOW_READ,
        "the snapshot appeared before the slow read finished ({visible_after:?})"
    );
    assert!(
        slowest < PASS_LATENCY_LIMIT,
        "a pass waited for the {SLOW_READ:?} store read: slowest pass {slowest:?}"
    );
}

/// spec-4c5b §7 gate test (C2, cas-223f): a 2 s `DirectorSnapshot` job leaves
/// every control pass under 100 ms. The snapshot runs as a job on the jobs
/// runtime's read lane, where the watchdog sees it, and the panels pick it up
/// on a later pass.
#[tokio::test]
async fn flv2_director_snapshot_off_lane() {
    const SNAPSHOT: Duration = Duration::from_secs(2);
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::super::loop_watchdog::LoopProgress::new();
    let store_worker = super::super::store_worker::global();
    let mut refresh = DirectorRefresh::new().with_read_delay(SNAPSHOT);
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    store
        .add(&cas_types::Task::new(
            "cas-f1v2".into(),
            "after one snapshot".into(),
        ))
        .unwrap();

    let started = Instant::now();
    let mut slowest = Duration::ZERO;
    let mut due = true;
    let mut snapshot_seen_running = false;
    let mut visible_after = None;
    while started.elapsed() < Duration::from_secs(15) {
        let pass = Instant::now();
        daemon
            .director_refresh_pass(&mut refresh, due, store_worker, &progress)
            .await;
        slowest = slowest.max(pass.elapsed());
        due = false;
        let lanes = daemon.queue_jobs.job_lanes().expect("real job lanes");
        if lanes.lane_stats().into_iter().any(|stats| {
            stats.running.is_some_and(|job| {
                job.lane == super::super::jobs::Lane::Read
                    && job.kind == super::super::factory_jobs::DIRECTOR_SNAPSHOT
            })
        }) {
            snapshot_seen_running = true;
        }
        if daemon
            .app
            .director_data()
            .ready_tasks
            .iter()
            .any(|task| task.id == "cas-f1v2")
        {
            visible_after = Some(started.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let visible_after = visible_after.expect("the snapshot never reached the panel data");
    assert!(
        snapshot_seen_running,
        "the snapshot did not run as a DirectorSnapshot job on the read lane"
    );
    assert!(
        visible_after >= SNAPSHOT,
        "the snapshot appeared before the slow job finished ({visible_after:?})"
    );
    assert!(
        slowest < PASS_LATENCY_LIMIT,
        "a pass waited for the {SNAPSHOT:?} snapshot: slowest pass {slowest:?}"
    );
}

/// spec-4c5b §7 gate test (C2/C3, cas-223f): the inject loop's retraction
/// decision is a fresh read, run as a `RevalidateFresh` job, never the cached
/// snapshot and never on the loop. The cached snapshot here is stale both
/// ways: it still shows `worker-a` assigned (which would retract a
/// legitimate idle alert) and does not yet show `worker-b` assigned.
#[tokio::test]
async fn flv2_retraction_reads_fresh() {
    use cas_store::wait_budget;
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    let mut task_a = cas_types::Task::new("cas-aaaa".into(), "a's work".into());
    task_a.status = cas_types::TaskStatus::InProgress;
    task_a.assignee = Some("worker-a".into());
    store.add(&task_a).unwrap();
    let mut task_b = cas_types::Task::new("cas-bbbb".into(), "b's work".into());
    store.add(&task_b).unwrap();

    let mut daemon = daemon_on_lanes(&cas_dir);
    // The cached snapshot: worker-a assigned, worker-b not.
    daemon.app.refresh_data().unwrap();
    // Then the store moves on: worker-a finished, worker-b was assigned.
    task_a.status = cas_types::TaskStatus::Closed;
    store.update(&task_a).unwrap();
    task_b.status = cas_types::TaskStatus::InProgress;
    task_b.assignee = Some("worker-b".into());
    store.update(&task_b).unwrap();

    let idle_alert = |worker: &str| crate::ui::factory::director::Prompt {
        merge_ready: false, merge_alert: None,
        target: "supervisor".into(),
        text: format!("{worker} is idle"),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: Some(worker.into()),
        durable_retry: false,
    };
    let mut refresh = DirectorRefresh::new();
    let violations = wait_budget::store_access_violations();
    {
        let _forbid = wait_budget::forbid_store_access();
        refresh
            .start_deliverability_check(
                &daemon.app,
                &mut daemon.queue_jobs,
                vec![idle_alert("worker-a"), idle_alert("worker-b")],
                Vec::new(),
                Instant::now(),
            )
            .expect("the re-check runs as a job");
    }
    assert_eq!(
        wait_budget::store_access_violations(),
        violations,
        "the retraction decision read the store on the loop: {:?}",
        wait_budget::last_store_access_violation()
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let batch = loop {
        if let Some(batch) = refresh.take_deliverability(&mut daemon.queue_jobs).pop() {
            break batch;
        }
        assert!(Instant::now() < deadline, "the re-check never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    let verdicts: Vec<(String, bool)> = batch
        .prompts
        .into_iter()
        .map(|(prompt, deliverable)| (prompt.drop_if_worker_assigned.unwrap(), deliverable))
        .collect();
    assert_eq!(
        verdicts,
        vec![
            ("worker-a".to_string(), true),
            ("worker-b".to_string(), false)
        ],
        "decided from the fresh read, not the cached snapshot"
    );
}

/// Review round 1, M2: a snapshot job that dies without a result frees its
/// slot, so the next pass starts a new one instead of waiting forever.
#[tokio::test]
async fn c2_failed_snapshot_frees_its_slot() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let mut refresh = DirectorRefresh::new().with_panicking_reads();
    refresh.start_director_read(&daemon.app, &mut daemon.queue_jobs);
    assert!(refresh.take_director_read(&mut daemon.queue_jobs).is_none());
    assert!(
        !refresh.director.busy(),
        "a dead snapshot job must not hold the slot"
    );
    // The next pass starts a fresh one, which now succeeds.
    refresh.set_panicking_reads(false);
    refresh.start_director_read(&daemon.app, &mut daemon.queue_jobs);
    assert!(refresh.take_director_read(&mut daemon.queue_jobs).is_some());
}

/// Review round 1, M2: a delivery read that dies keeps its events: they
/// wait for the next read instead of being lost.
#[tokio::test]
async fn c2_failed_delivery_read_requeues_its_events() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let mut refresh = DirectorRefresh::new().with_panicking_reads();
    let event = crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "idle-worker".into(),
        active_task: None,
    };
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();
    assert!(refresh.take_delivery_read(&mut daemon.queue_jobs).is_none());
    assert!(!refresh.delivery.busy());
    assert_eq!(refresh.delivery_backlog.len(), 1, "the event waits for the next read");
    refresh.set_panicking_reads(false);
    refresh
        .start_delivery_backlog(&daemon.app, &mut daemon.queue_jobs)
        .unwrap();
    let (events, _) = refresh
        .take_delivery_read(&mut daemon.queue_jobs)
        .expect("the retried read reports");
    assert_eq!(events.len(), 1);
}

/// Review round 1, M4: shutdown never drops prompts waiting for their
/// re-check. A re-check still running past the drain is handed back for the
/// caller to re-check itself.
#[tokio::test]
async fn c2_shutdown_hands_back_pending_rechecks() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let gate = Arc::new(ReadGate::default());
    let mut refresh = DirectorRefresh::new().with_read_gate(Arc::clone(&gate));
    let alert = crate::ui::factory::director::Prompt {
        merge_ready: false, merge_alert: None,
        target: "supervisor".into(),
        text: "worker-a is idle".into(),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: Some("worker-a".into()),
        durable_retry: false,
    };
    refresh
        .start_deliverability_check(
            &daemon.app,
            &mut daemon.queue_jobs,
            vec![alert.clone()],
            Vec::new(),
            Instant::now(),
        )
        .unwrap();
    let (done, pending) = refresh
        .drain_deliverability(&mut daemon.queue_jobs, Duration::ZERO)
        .await;
    assert!(done.is_empty());
    assert_eq!(pending.len(), 1, "the unfinished re-check is handed back");
    assert_eq!(pending[0].prompts[0].text, alert.text);

    gate.open();

    refresh
        .start_deliverability_check(
            &daemon.app,
            &mut daemon.queue_jobs,
            vec![alert],
            Vec::new(),
            Instant::now(),
        )
        .unwrap();
    let (done, pending) = refresh
        .drain_deliverability(&mut daemon.queue_jobs, Duration::from_secs(10))
        .await;
    assert!(pending.is_empty());
    assert_eq!(done.len(), 1, "a re-check that finishes during the drain is delivered");
}

/// Review round 1, M1: the git half of the stall evidence is its own job on
/// the external lane, never part of a read-lane job.
#[test]
fn c2_ci_status_runs_on_the_external_lane() {
    let spec = super::super::factory_jobs::ci_status_spec(1);
    assert_eq!(spec.lane, super::super::jobs::Lane::External);
    assert_eq!(spec.kind, super::super::factory_jobs::CI_STATUS);
    for read in [
        super::super::factory_jobs::director_snapshot_spec(1),
        super::super::factory_jobs::delivery_read_spec(1),
        super::super::factory_jobs::deliverability_spec(1),
    ] {
        assert_eq!(read.lane, super::super::jobs::Lane::Read, "{}", read.key);
    }
}

/// C3 round-3 review: a lane can drop a queued snapshot and then still run
/// the key's coalesced rerun. C2 frees its slot on Dropped and applies a
/// later Done only when its generation is the one in flight.
#[test]
fn c2_dropped_then_done_applies_only_the_current_generation() {
    use super::super::jobs::JobResult;
    use super::super::prompt_queue_jobs::ExternalOutcome;
    let result = |generation, outcome| JobResult {
        kind: super::super::factory_jobs::DIRECTOR_SNAPSHOT,
        key: super::super::factory_jobs::DIRECTOR_SNAPSHOT_KEY.to_string(),
        generation,
        outcome: Ok(outcome),
        elapsed: Duration::ZERO,
        writer_wait: Duration::ZERO,
        over_budget: false,
    };
    let mut refresh = DirectorRefresh::new();
    refresh.director.start(1);
    refresh.accept(result(1, ExternalOutcome::Dropped));
    assert!(!refresh.director.busy(), "Dropped frees the slot");

    // A later rerun for generation 1 reports Done while generation 2 runs.
    refresh.director.start(2);
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let load = || {
        Box::new(super::super::factory_jobs::FactoryJobOutput::DirectorSnapshot(
            daemon.app.director_refresh_request().read(None),
        )) as Box<dyn std::any::Any + Send>
    };
    refresh.accept(result(1, ExternalOutcome::Done(load())));
    assert!(refresh.director.ready.is_none(), "a stale generation is ignored");
    refresh.accept(result(2, ExternalOutcome::Done(load())));
    assert!(refresh.director.ready.is_some(), "the current generation applies");
}

/// Review round 2, M2: events re-queued after a failed delivery read start
/// their next read on the next pass, even when nothing else happens (no
/// refresh due, no other read finished).
#[tokio::test]
async fn c2_requeued_events_start_on_a_quiet_pass() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let progress = super::super::loop_watchdog::LoopProgress::new();
    let store_worker = super::super::store_worker::global();
    let mut refresh = DirectorRefresh::new().with_panicking_reads();
    let event = crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "quiet-worker".into(),
        active_task: None,
    };
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();
    assert!(refresh.take_delivery_read(&mut daemon.queue_jobs).is_none());
    assert_eq!(refresh.delivery_backlog.len(), 1);
    refresh.set_panicking_reads(false);
    assert!(
        refresh.has_finished(&mut daemon.queue_jobs),
        "waiting events wake the refresh pass"
    );
    // Two quiet passes: the first starts the read, the second delivers it.
    for _ in 0..2 {
        daemon
            .director_refresh_pass(&mut refresh, false, store_worker, &progress)
            .await;
    }
    assert!(refresh.delivery_backlog.is_empty(), "the backlog started");
    assert!(!refresh.delivery.busy(), "and its read was delivered");
}

/// Review round 2, M3: a delivery read without fresh data is retried three
/// times (four reads in all), and only then are its events delivered as
/// detected.
#[test]
fn c2_stale_delivery_read_retries_before_delivering() {
    let mut refresh = DirectorRefresh::new();
    let event = || crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "w".into(),
        active_task: None,
    };
    for attempt in 1..=3 {
        assert!(refresh.retry_stale_delivery(vec![event()]).is_none(), "attempt {attempt}");
    }
    assert_eq!(refresh.delivery_backlog.len(), 3);
    assert_eq!(refresh.retry_stale_delivery(vec![event()]).map(|e| e.len()), Some(1));
    refresh.note_fresh_delivery_read();
    assert!(refresh.retry_stale_delivery(vec![event()]).is_none(), "a fresh read resets the count");
}

/// Review round 2, M3: when the fresh read fails, revalidation never
/// filters an event against the cache; every event is delivered as detected.
#[tokio::test]
async fn c2_failed_fresh_read_never_revalidates_from_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let events = vec![
        crate::ui::factory::director::DirectorEvent::WorkerIdle {
            worker: "unknown-worker".into(),
            active_task: None,
        },
        crate::ui::factory::director::DirectorEvent::WorkerIdle {
            worker: "another-unknown".into(),
            active_task: None,
        },
    ];
    let (delivered, _prompts, sweep) = daemon.app.revalidate_and_prompt_for_delivery_from(
        &events,
        crate::ui::factory::app::DeliveryInputs::failed_for_test(),
    );
    assert_eq!(delivered.len(), events.len(), "nothing is filtered from the cache");
    assert!(sweep.is_none(), "no retraction without a fresh read");
}

/// Review round 2, L3: a delivery read that outlives its timeout gives its
/// events to the next read, and its late result is ignored.
#[tokio::test]
async fn c2_expired_delivery_read_requeues_and_ignores_the_late_result() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let gate = Arc::new(ReadGate::default());
    // cas-3b16 (L-d): the read holds on a gate and the timeout is zero, so
    // the read times out on the first poll, whatever the machine's speed.
    let mut refresh = DirectorRefresh::new()
        .with_read_gate(Arc::clone(&gate))
        .with_timeout(Duration::ZERO);
    let event = crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "slow-worker".into(),
        active_task: None,
    };
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();
    assert!(refresh.take_delivery_read(&mut daemon.queue_jobs).is_none());
    assert!(!refresh.delivery_busy(), "the timed-out read freed its slot");
    assert_eq!(refresh.delivery_backlog.len(), 1, "its event waits for the next read");
    refresh.timeout_override = Some(Duration::from_secs(30));
    gate.open();
    // The read and the liveness job beside it both report late.
    wait_for_late_results(&mut refresh, &mut daemon, 2).await;
    assert!(
        refresh.take_delivery_read(&mut daemon.queue_jobs).is_none(),
        "the late result is ignored"
    );
}

/// Poll until `count` late results were ignored. The deadline is a hang
/// guard, not a timing margin: the outcome never depends on it.
async fn wait_for_late_results(refresh: &mut DirectorRefresh, daemon: &mut FactoryDaemon, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while refresh.late_results_ignored < count {
        refresh.has_finished(&mut daemon.queue_jobs);
        assert!(Instant::now() < deadline, "the late results never arrived");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Review round 2, L3: a prompt re-check that outlives its timeout delivers
/// its prompts (uncertainty delivers), and its late verdicts are ignored.
#[tokio::test]
async fn c2_expired_recheck_delivers_and_ignores_the_late_result() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let gate = Arc::new(ReadGate::default());
    let mut refresh = DirectorRefresh::new()
        .with_read_gate(Arc::clone(&gate))
        .with_timeout(Duration::ZERO);
    let alert = crate::ui::factory::director::Prompt {
        merge_ready: false, merge_alert: None,
        target: "supervisor".into(),
        text: "worker-a is idle".into(),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: Some("worker-a".into()),
        durable_retry: false,
    };
    refresh
        .start_deliverability_check(&daemon.app, &mut daemon.queue_jobs, vec![alert], Vec::new(), Instant::now())
        .unwrap();
    let batches = refresh.take_deliverability(&mut daemon.queue_jobs);
    assert_eq!(batches.len(), 1);
    assert!(batches[0].prompts.iter().all(|(_, deliverable)| *deliverable));
    refresh.timeout_override = Some(Duration::from_secs(30));
    gate.open();
    wait_for_late_results(&mut refresh, &mut daemon, 1).await;
    assert!(
        refresh.take_deliverability(&mut daemon.queue_jobs).is_empty(),
        "the late verdicts are ignored"
    );
}

/// Review round 2, L3: a real lane drop. With the blocking lane busy and
/// full, the oldest queued health probe is dropped and handed back to the
/// detector to ask again, once.
#[tokio::test]
async fn c2_lane_drop_hands_the_probe_back_once() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let (s, r) = (Arc::clone(&started), Arc::clone(&release));
    daemon.queue_jobs.submit_external(
        super::super::jobs::JobSpec {
            kind: "TestBlocker",
            key: "TestBlocker:c2".into(),
            lane: super::super::jobs::Lane::Blocking,
            criticality: super::super::jobs::Criticality::Critical,
            generation: 1,
            budget: Duration::from_secs(60),
        },
        move || {
            s.wait();
            r.wait();
            Box::new(())
        },
    );
    started.wait();
    let mut refresh = DirectorRefresh::new();
    let requests: Vec<_> = (0..=256)
        .map(|n| crate::ui::factory::director::WorkerHealthRequest {
            worker: format!("w{n}"),
            task: "cas-t".into(),
            threshold_secs: 300,
            delay_secs: 5,
            requested_at: chrono::Utc::now(),
        })
        .collect();
    let refused = refresh.submit_health_probes(&daemon.app, &mut daemon.queue_jobs, requests);
    assert_eq!(
        refused,
        vec![("w0".to_string(), "cas-t".to_string())],
        "the oldest queued probe was dropped and handed back"
    );
    release.wait();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (reports, failed) = refresh.take_health_reports(&mut daemon.queue_jobs);
        assert!(failed.is_empty(), "a dropped probe is handed back once: {failed:?}");
        let _ = reports;
        if refresh.health_in_flight.is_empty() || Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(refresh.health_in_flight.is_empty(), "every other probe reported");
}

/// Review round 2, M1: a snapshot given up after its timeout still holds its
/// key in the runtime, so the next submit is coalesced into its rerun. That
/// submit stays in flight and its result applies; the timed-out job's result
/// is ignored. Before the fix, every later snapshot was dropped as refused
/// and the panels froze while reads stayed slow.
#[tokio::test]
async fn c2_coalesced_snapshot_after_timeout_still_applies() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let gate = Arc::new(ReadGate::default());
    let mut refresh = DirectorRefresh::new()
        .with_read_gate(Arc::clone(&gate))
        .with_timeout(Duration::ZERO);
    refresh.start_director_read(&daemon.app, &mut daemon.queue_jobs);
    assert!(refresh.take_director_read(&mut daemon.queue_jobs).is_none());
    assert!(!refresh.director.busy(), "the timed-out snapshot freed its slot");

    // The first job is held on the gate, so this submit coalesces into its
    // rerun. Keep the coalesced job from timing out while it waits.
    refresh.timeout_override = Some(Duration::from_secs(30));
    refresh.start_director_read(&daemon.app, &mut daemon.queue_jobs);
    assert!(refresh.director.busy(), "a coalesced submit stays in flight");
    let in_flight = refresh.director.in_flight.map(|(generation, _)| generation);
    gate.open();
    let deadline = Instant::now() + Duration::from_secs(10);
    let load = loop {
        if let Some(load) = refresh.take_director_read(&mut daemon.queue_jobs) {
            break load;
        }
        assert!(Instant::now() < deadline, "the coalesced snapshot never applied");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    drop(load);
    assert!(in_flight.is_some());
    assert!(!refresh.director.busy());
}

/// cas-3b16 (C2 review round 3, L-c): the lifecycle wiring of the stale-read
/// retry. A batch whose delivery reads keep coming back without fresh data
/// gets one read plus three retries, each started by a refresh pass, and is
/// then delivered as detected; no fifth read is started.
#[tokio::test]
async fn c3b16_lifecycle_retries_a_stale_delivery_read_three_times_then_delivers() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let progress = super::super::loop_watchdog::LoopProgress::new();
    let store_worker = super::super::store_worker::global();
    let reads = Arc::new(AtomicUsize::new(0));
    let mut refresh = DirectorRefresh::new().with_stale_delivery_reads(Arc::clone(&reads));
    let event = crate::ui::factory::director::DirectorEvent::TaskBlocked {
        task_id: "cas-b10c".into(),
        task_title: "blocked".into(),
        worker: "retry-worker".into(),
    };
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();
    for pass in 1..=6 {
        daemon
            .director_refresh_pass(&mut refresh, false, store_worker, &progress)
            .await;
        let expected = (pass + 1).min(4);
        assert_eq!(reads.load(Ordering::SeqCst), expected, "reads after pass {pass}");
    }
    assert!(refresh.delivery_backlog.is_empty(), "the batch was delivered");
    assert!(!refresh.delivery_busy(), "and no read is left in flight");
    assert_eq!(refresh.failed_delivery_reads, 0, "the retry count starts over");
}

/// cas-3b16 (C2 review round 2, L2): the idle-worker liveness scan (agent
/// store, transcripts, `/proc`) runs on the blocking lane beside the delivery
/// read, never on the read lane, and the read is applied with its result.
#[tokio::test]
async fn c3b16_idle_liveness_runs_on_the_blocking_lane() {
    use super::super::jobs::Lane;
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let gate = Arc::new(ReadGate::default());
    let mut refresh = DirectorRefresh::new().with_read_gate(Arc::clone(&gate));
    let event = crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "idle-w".into(),
        active_task: None,
    };
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();

    // Both jobs hold on the gate, so each is seen running on its lane.
    let lanes = daemon.queue_jobs.job_lanes().expect("real job lanes");
    let deadline = Instant::now() + Duration::from_secs(30);
    let (read_lane, blocking_lane) = loop {
        let running: Vec<_> = lanes
            .lane_stats()
            .into_iter()
            .filter_map(|stats| stats.running)
            .collect();
        let on = |lane: Lane| running.iter().find(|job| job.lane == lane).map(|job| job.key.clone());
        if let (Some(read), Some(blocking)) = (on(Lane::Read), on(Lane::Blocking)) {
            break (read, blocking);
        }
        assert!(Instant::now() < deadline, "the jobs never ran: {running:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(read_lane, super::super::factory_jobs::DELIVERY_READ_KEY);
    assert_eq!(blocking_lane, super::super::factory_jobs::IDLE_LIVENESS_KEY);
    assert!(
        refresh.take_delivery_read(&mut daemon.queue_jobs).is_none(),
        "nothing applies before both report"
    );

    gate.open();
    let deadline = Instant::now() + Duration::from_secs(30);
    let (_, inputs) = loop {
        if let Some(read) = refresh.take_delivery_read(&mut daemon.queue_jobs) {
            break read;
        }
        assert!(Instant::now() < deadline, "the delivery read never applied");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert!(
        inputs.idle_liveness_for_test().contains_key("idle-w"),
        "the read is joined with the liveness observed beside it"
    );
}

/// cas-3b16 (C2 review round 1, L7): a prompt batch with nothing to re-check
/// waits behind an earlier batch whose re-check is still running, so prompts
/// are injected in the order they were generated.
#[tokio::test]
async fn c3b16_later_prompt_batch_waits_behind_a_pending_recheck() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::super::loop_watchdog::LoopProgress::new();
    let store_worker = super::super::store_worker::global();
    let gate = Arc::new(ReadGate::default());
    let mut refresh = DirectorRefresh::new().with_read_gate(Arc::clone(&gate));
    let prompt = |text: &str, drop_if_worker_assigned: Option<&str>| crate::ui::factory::director::Prompt {
        merge_ready: false, merge_alert: None,
        target: "supervisor".into(),
        text: text.into(),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: drop_if_worker_assigned.map(str::to_string),
        durable_retry: false,
    };
    for batch in [prompt("first", Some("worker-a")), prompt("second", None)] {
        daemon
            .deliver_director_events(
                &mut refresh,
                Vec::new(),
                (Vec::new(), vec![batch], None),
                Instant::now(),
                0.0,
                store_worker,
                &progress,
            )
            .await;
    }
    assert_eq!(
        refresh.deliverability_pending.len(),
        2,
        "the second batch waits behind the first instead of being injected"
    );

    gate.open();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut order = Vec::new();
    while order.len() < 2 {
        for batch in refresh.take_deliverability(&mut daemon.queue_jobs) {
            order.extend(batch.prompts.into_iter().map(|(prompt, deliverable)| {
                assert!(deliverable);
                prompt.text
            }));
        }
        assert!(Instant::now() < deadline, "the re-checks never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(order, vec!["first".to_string(), "second".to_string()]);
}

/// cas-3b16 (C2 review round 2, L2): a reminder pass carrying edges (this
/// tick's director events) still fires their reminders when the job runtime
/// refuses the scan; edges are consumed once and must not be lost.
#[tokio::test]
async fn c3b16_refused_reminder_scan_keeps_its_edges() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    // A retired supervisor's reminder can fall back to its successor;
    // a retired worker's reminder must remain addressed to that worker.
    // This gives the scan a recipient without reading the director.
    let agents = crate::store::open_agent_store(&cas_dir).unwrap();
    let mut gone = cas_types::Agent::new_with_role(
        "gone".into(),
        "previous-supervisor".into(),
        cas_types::AgentRole::Supervisor,
    );
    agents.register(&gone).unwrap();
    gone.status = cas_types::AgentStatus::Shutdown;
    agents.update(&gone).unwrap();
    let event = crate::ui::factory::director::DirectorEvent::TaskBlocked {
        task_id: "cas-ed9e".into(),
        task_title: "edge".into(),
        worker: "edge-worker".into(),
    };
    let reminders = crate::store::open_reminder_store(&cas_dir).unwrap();
    let reminder_id = reminders
        .create_with_scope(
            "gone",
            None,
            "the task blocked",
            cas_store::ReminderTriggerType::Event,
            None,
            Some(event.event_type()),
            None,
            0,
            None,
            None,
            false,
            None,
        )
        .unwrap();

    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.queue_jobs.close_lanes_for_test();
    daemon.process_reminders(&[event.clone()]);

    let deadline = Instant::now() + Duration::from_secs(30);
    while !reminders
        .get_event_reminders(event.event_type())
        .unwrap()
        .is_empty()
    {
        assert!(Instant::now() < deadline, "the refused scan's event reminder never fired");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let fired = reminders.list_recently_fired(60).unwrap();
    let reminder = fired
        .iter()
        .find(|r| r.id == reminder_id)
        .expect("the refused scan must fire the reminder, not expire it");
    let context = reminder.fired_event.as_ref().expect("the consumed edge is preserved");
    assert_eq!(context["event_type"], event.event_type());
    assert_eq!(context["data"]["task_id"], "cas-ed9e");
}

/// A `WorkerIdle` event for a worker with a task, and cached panel data that
/// knows the worker, so revalidation renders its prompt.
fn idle_worker_with_task(
    daemon: &mut FactoryDaemon,
) -> crate::ui::factory::director::DirectorEvent {
    let mut data = daemon.app.director_data().clone();
    data.agent_id_to_name
        .insert("sess-idle".into(), "idle-w".into());
    daemon.app.set_director_data_for_test(data);
    crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "idle-w".into(),
        active_task: Some(cas_factory::ActiveLeaseSummary {
            task_id: "cas-1d1e".into(),
            task_title: "running".into(),
            task_status: cas_types::TaskStatus::InProgress,
            close_rejected_reason: Some("proof required".into()),
            pending_qa: None,
        }),
    }
}

/// cas-4491f (C2b review L3): an idle-worker liveness job that dies leaves
/// the worker unobserved. The delivery read still applies, and the worker's
/// prompt is delivered (uncertainty delivers).
#[tokio::test]
async fn c4491f_failed_liveness_job_still_delivers_the_prompt() {
    use std::sync::atomic::AtomicUsize;
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let event = idle_worker_with_task(&mut daemon);
    let mut refresh = DirectorRefresh::new()
        .with_stale_delivery_reads(Arc::new(AtomicUsize::new(0)))
        .with_failing_liveness();
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();
    let (events, inputs) = refresh
        .take_delivery_read(&mut daemon.queue_jobs)
        .expect("a failed liveness job does not hold the read");
    assert!(!inputs.idle_liveness_for_test().contains_key("idle-w"));
    let (_, prompts, _) = daemon.app.revalidate_and_prompt_for_delivery_from(&events, inputs);
    assert!(
        prompts.iter().any(|prompt| prompt.text.contains("idle-w")),
        "the unobserved worker's prompt is delivered: {prompts:?}"
    );
}

/// cas-4491f (C2b review L3): the same for a liveness job that never reports
/// and is given up at its timeout.
#[test]
fn c4491f_expired_liveness_job_still_delivers_the_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let event = idle_worker_with_task(&mut daemon);
    let mut refresh = DirectorRefresh::new().with_timeout(Duration::ZERO);
    refresh.delivery.start(7);
    refresh.idle_liveness.start(7);
    assert!(refresh.delivery.finish(
        7,
        (vec![event], crate::ui::factory::app::DeliveryInputs::failed_for_test())
    ));
    let (events, inputs) = refresh
        .take_delivery_read(&mut daemon.queue_jobs)
        .expect("an expired liveness job does not hold the read");
    assert!(inputs.idle_liveness_for_test().is_empty());
    let (_, prompts, _) = daemon.app.revalidate_and_prompt_for_delivery_from(&events, inputs);
    assert!(
        prompts.iter().any(|prompt| prompt.text.contains("idle-w")),
        "the unobserved worker's prompt is delivered: {prompts:?}"
    );
}

/// cas-4491f (C2b review L4): a refresh due while a delivery read's liveness
/// job runs is not dropped. It starts once the job reports, without waiting
/// for the next due tick.
#[tokio::test]
async fn c4491f_refresh_requested_during_liveness_runs_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::super::loop_watchdog::LoopProgress::new();
    let store_worker = super::super::store_worker::global();
    let gate = Arc::new(ReadGate::default());
    let mut refresh = DirectorRefresh::new().with_read_gate(Arc::clone(&gate));
    let event = crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "idle-w".into(),
        active_task: None,
    };
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();
    // The refresh tick lands while the read and its liveness job are held.
    daemon
        .director_refresh_pass(&mut refresh, true, store_worker, &progress)
        .await;
    assert!(!refresh.director.busy(), "no snapshot can start yet");

    gate.open();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut applied = false;
    while !applied {
        assert!(Instant::now() < deadline, "the requested refresh never ran");
        // No refresh is due on these passes: only the deferred request.
        let woke = refresh.has_finished(&mut daemon.queue_jobs);
        if woke {
            applied = daemon
                .director_refresh_pass(&mut refresh, false, store_worker, &progress)
                .await;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
