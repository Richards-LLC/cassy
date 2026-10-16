//! cas-af23 / GH #1241: exhaustion is a wake failure, not a stale task premise.
use super::*;
use crate::prompt_revalidation::{
    MergeRequestEnvelope, WorkerActionKind, attach_blocker_envelope, attach_merge_request_envelope,
    attach_worker_action_envelope,
};

fn child(test: &str) -> bool {
    super::super::supervisor_single_turn_tests::fixture_child(&format!(
        "{}::{test}",
        module_path!().split_once("::").unwrap().1
    ))
}

async fn exhausted_message(body: String, summary: &str, context: &[&str]) {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = super::super::supervisor_single_turn_tests::fixture().await;
    let cas = daemon.app.cas_dir().to_path_buf();
    let mut worker = cas_types::Agent::new_with_role(
        "retry-worker".into(),
        "worker".into(),
        cas_types::AgentRole::Worker,
    );
    worker.factory_session = Some(daemon.session_name.clone());
    open_agent_store(&cas).unwrap().register(&worker).unwrap();
    let mut task = cas_types::Task::new("cas-af23".into(), "unmerged delivery".into());
    task.status = cas_types::TaskStatus::AwaitingMerge;
    task.deliverables.factory_branch_anchor = Some("delivery-tip".into());
    crate::store::open_task_store_local(&cas)
        .unwrap()
        .add(&task)
        .unwrap();
    let queue = open_prompt_queue_store(&cas).unwrap();
    let id = queue
        .enqueue_urgent_with_outcome(
            "worker",
            "supervisor",
            &body,
            Some(&daemon.session_name),
            Some(summary),
            Some(cas_store::NotificationPriority::High),
            false,
            Some(&cas_store::QueueOrigin::RegisteredAgent {
                agent_id: worker.id,
            }),
        )
        .unwrap()
        .id();
    daemon.app.refresh_data().unwrap();
    daemon
        .lifecycle_redelivery_counts
        .insert(id, LIFECYCLE_MAX_RENUDGE_ATTEMPTS);
    daemon.process_prompt_queue().await.unwrap();
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert_ne!(
        report.stage,
        cas_store::DeliveryStage::Abandoned,
        "an open task's action message must survive exhausted wakes: {report:?}"
    );
    let row = queue.queued_prompt(id).unwrap().unwrap();
    assert!(
        row.processed_at.is_none(),
        "retain the actionable body: {row:?}"
    );
    assert!(
        report.confirmed_at.is_none(),
        "a spent budget is not a recipient read"
    );
    let detail = report
        .pending_detail
        .as_deref()
        .expect("exhaustion must explain the wake failure");
    assert!(
        detail.contains(&format!("notification_id={id}")),
        "identify the failed wake: {detail}"
    );
    assert!(
        detail.contains("wake") && detail.contains("exhausted"),
        "state the actual failure: {detail}"
    );
    assert!(
        !detail.contains("moved on") && !detail.contains("expired premise"),
        "no stale-task evidence: {detail}"
    );
    for expected in context {
        assert!(
            detail.contains(expected),
            "retain {expected} context: {detail}"
        );
    }
    assert_eq!(
        crate::store::open_task_store_local(&cas)
            .unwrap()
            .get(&task.id)
            .unwrap()
            .status,
        cas_types::TaskStatus::AwaitingMerge
    );
    for _ in 0..3 {
        daemon.process_prompt_queue().await.unwrap();
    }
    assert_eq!(
        queue.peek_all(20).unwrap().len(),
        1,
        "no recursive wake-failure notice"
    );
    let unseen = queue
        .poll_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20)
        .unwrap();
    assert!(
        unseen.iter().any(|row| row.id == id && row.prompt == body),
        "the recipient must still be able to read the original guidance: {unseen:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cas_af23_exhausted_merge_request_keeps_actual_task_and_merge_guidance() {
    if !child("cas_af23_exhausted_merge_request_keeps_actual_task_and_merge_guidance") {
        return;
    }
    let envelope = MergeRequestEnvelope {
        task_id: "cas-af23".into(),
        branch_tip: "delivery-tip".into(),
        target_branch: "epic/v46".into(),
        target_branch_tip: "target-tip".into(),
        anchor_tip: None,
        commits_not_on_target_base: 1,
        pr_number: None,
        source_freshness_unavailable: None,
    };
    exhausted_message(
        attach_merge_request_envelope(
            "Merge delivery-tip into epic/v46 for cas-af23; then re-close.",
            &envelope,
        ),
        "green delivery awaits review",
        &["cas-af23", "delivery-tip", "epic/v46"],
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cas_af23_exhausted_status_about_open_task_remains_readable() {
    if !child("cas_af23_exhausted_status_about_open_task_remains_readable") {
        return;
    }
    exhausted_message(
        attach_worker_action_envelope(
            "cas-af23 verification timed out; supervisor recovery required.",
            WorkerActionKind::Status,
            "worker",
            None,
        ),
        "cas-af23 verification timed out",
        &["cas-af23"],
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cas_af23_exhausted_status_uses_envelope_task_without_summary_id() {
    if !child("cas_af23_exhausted_status_uses_envelope_task_without_summary_id") {
        return;
    }
    exhausted_message(
        attach_worker_action_envelope(
            "Supervisor recovery required.",
            WorkerActionKind::Status,
            "worker",
            Some("cas-af23"),
        ),
        "verification timed out",
        &["cas-af23"],
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cas_af23_exhausted_blocker_about_open_task_remains_readable() {
    if !child("cas_af23_exhausted_blocker_about_open_task_remains_readable") {
        return;
    }
    exhausted_message(
        attach_blocker_envelope(
            "cas-af23 verification timed out; supervisor recovery required.",
            "worker",
            Some("cas-af23"),
        ),
        "cas-af23 blocked on verification",
        &["cas-af23"],
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cas_af23_exhausted_unknown_task_blocker_fails_open() {
    if !child("cas_af23_exhausted_unknown_task_blocker_fails_open") {
        return;
    }
    exhausted_message(
        attach_blocker_envelope(
            "Need supervisor help; the task could not be resolved.",
            "worker",
            None,
        ),
        "unresolved blocker",
        &["task unspecified"],
    )
    .await;
}

#[tokio::test]
async fn cas_af23_missing_lifecycle_task_has_no_positive_stale_evidence() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let root = tempfile::tempdir().unwrap();
    let cas = crate::store::init_cas_dir(root.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas);
    let body = format!(
        "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-absent\" \
         old=\"in_progress\" new=\"awaiting_merge\" actor=\"worker\" \
         notification_id=\"3386\" occurrence=\"{}\">parked</task-lifecycle>",
        chrono::Utc::now().to_rfc3339(),
    );
    let queue = open_prompt_queue_store(&cas).unwrap();
    let id = queue
        .enqueue_urgent_with_outcome(
            "lifecycle-wake:task_awaiting_merge:cas-absent",
            "supervisor",
            &body,
            Some(&daemon.session_name),
            Some("cas-absent awaiting merge"),
            Some(cas_store::NotificationPriority::High),
            false,
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .unwrap()
        .id();
    daemon.process_prompt_queue().await.unwrap();
    let row = queue.queued_prompt(id).unwrap().unwrap();
    assert!(
        row.processed_at.is_none(),
        "missing task is not evidence that it moved on: {row:?}"
    );
    assert!(
        queue
            .poll_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20)
            .unwrap()
            .iter()
            .any(|row| row.id == id && row.prompt == body)
    );
}

async fn daemon_relay(
    death: bool,
    status: cas_types::TaskStatus,
) -> cas_store::MessageDeliveryReport {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let root = tempfile::tempdir().unwrap();
    let cas = crate::store::init_cas_dir(root.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas);
    let mut task = cas_types::Task::new("cas-af23".into(), "lifecycle premise".into());
    task.status = status;
    crate::store::open_task_store_local(&cas)
        .unwrap()
        .add(&task)
        .unwrap();
    let body = if death {
        crate::prompt_revalidation::format_worker_died_relay(
            "dead-id",
            "departed-worker",
            "worker_died:dead-id:1",
            "heartbeat stale",
            &[task.id],
            &[],
            3386,
        )
    } else {
        format!(
            "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-af23\" \
             old=\"in_progress\" new=\"awaiting_merge\" actor=\"worker\" \
             notification_id=\"3386\" occurrence=\"{}\">parked</task-lifecycle>",
            task.updated_at.to_rfc3339(),
        )
    };
    let queue = open_prompt_queue_store(&cas).unwrap();
    let id = queue
        .enqueue_urgent_with_outcome(
            "lifecycle-wake:task_awaiting_merge:cas-af23",
            "supervisor",
            &body,
            Some(&daemon.session_name),
            Some("action needed for cas-af23"),
            Some(cas_store::NotificationPriority::High),
            false,
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .unwrap()
        .id();
    daemon
        .lifecycle_redelivery_counts
        .insert(id, LIFECYCLE_MAX_RENUDGE_ATTEMPTS);
    daemon.process_prompt_queue().await.unwrap();
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    if report.stage != cas_store::DeliveryStage::Abandoned {
        assert!(
            queue
                .poll_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20)
                .unwrap()
                .iter()
                .any(|row| row.id == id && row.prompt == body)
        );
    }
    report
}

#[tokio::test]
async fn cas_af23_current_lifecycle_survives_exhaustion() {
    let report = daemon_relay(false, cas_types::TaskStatus::AwaitingMerge).await;
    assert_ne!(
        report.stage,
        cas_store::DeliveryStage::Abandoned,
        "{report:?}"
    );
    let detail = report.pending_detail.unwrap();
    assert!(
        detail.contains("cas-af23") && detail.contains("wake retries exhausted"),
        "{detail}"
    );
    assert!(!detail.contains("expired premise"), "{detail}");
}

#[tokio::test]
async fn cas_af23_positive_stale_lifecycle_keeps_withdrawal_notice() {
    let report = daemon_relay(false, cas_types::TaskStatus::InProgress).await;
    assert_eq!(
        report.stage,
        cas_store::DeliveryStage::Abandoned,
        "{report:?}"
    );
    let detail = report.pending_detail.unwrap();
    assert!(
        detail.contains("cas-af23") && detail.contains("task moved on"),
        "{detail}"
    );
    assert!(detail.contains("expired premise"), "{detail}");
}

#[tokio::test]
async fn cas_af23_worker_death_keeps_its_dedicated_failure_notice() {
    let report = daemon_relay(true, cas_types::TaskStatus::InProgress).await;
    assert_eq!(
        report.stage,
        cas_store::DeliveryStage::Abandoned,
        "{report:?}"
    );
    let detail = report.pending_detail.unwrap();
    assert!(
        detail.contains("departed-worker") && detail.contains("worker is still gone"),
        "{detail}"
    );
    assert!(
        !detail.contains("expired premise") && !detail.contains("unknown task"),
        "{detail}"
    );
}
