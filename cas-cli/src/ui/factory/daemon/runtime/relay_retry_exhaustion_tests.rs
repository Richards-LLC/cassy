//! cas-af23 / GH #1241: exhaustion is a wake failure, not a stale task premise.
use super::*;
use crate::prompt_revalidation::{
    MergeRequestEnvelope, WorkerActionKind, attach_blocker_envelope,
    attach_merge_request_envelope, attach_worker_action_envelope,
};

fn child(test: &str) -> bool {
    super::super::supervisor_single_turn_tests::fixture_child(&format!(
        "{}::{test}",
        module_path!().split_once("::").unwrap().1
    ))
}

async fn exhausted_message(body: String, summary: &str) {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_root, mut daemon) = super::super::supervisor_single_turn_tests::fixture().await;
    let cas = daemon.app.cas_dir().to_path_buf();
    let mut worker = cas_types::Agent::new_with_role(
        "retry-worker".into(), "worker".into(), cas_types::AgentRole::Worker,
    );
    worker.factory_session = Some(daemon.session_name.clone());
    open_agent_store(&cas).unwrap().register(&worker).unwrap();
    let mut task = cas_types::Task::new("cas-af23".into(), "unmerged delivery".into());
    task.status = cas_types::TaskStatus::AwaitingMerge;
    task.deliverables.factory_branch_anchor = Some("delivery-tip".into());
    crate::store::open_task_store_local(&cas).unwrap().add(&task).unwrap();
    let queue = open_prompt_queue_store(&cas).unwrap();
    let id = queue.enqueue_urgent_with_outcome(
        "worker", "supervisor", &body, Some(&daemon.session_name), Some(summary),
        Some(cas_store::NotificationPriority::High), false,
        Some(&cas_store::QueueOrigin::RegisteredAgent { agent_id: worker.id }),
    ).unwrap().id();
    daemon.app.refresh_data().unwrap();
    daemon.lifecycle_redelivery_counts.insert(id, LIFECYCLE_MAX_RENUDGE_ATTEMPTS);
    daemon.process_prompt_queue().await.unwrap();
    let report = queue.message_delivery_report(id).unwrap().unwrap();
    assert_ne!(report.stage, cas_store::DeliveryStage::Abandoned,
        "an open task's action message must survive exhausted wakes: {report:?}");
    let row = queue.queued_prompt(id).unwrap().unwrap();
    assert!(row.processed_at.is_none(), "retain the actionable body: {row:?}");
    assert!(report.confirmed_at.is_none(), "a spent budget is not a recipient read");
    let detail: String = rusqlite::Connection::open(cas.join("cas.db")).unwrap()
        .query_row("SELECT last_pending_detail FROM prompt_queue WHERE id=?", [id], |r| r.get(0)).unwrap();
    assert!(detail.contains(&format!("notification_id={id}")), "identify the failed wake: {detail}");
    assert!(detail.contains("wake") && detail.contains("exhausted"), "state the actual failure: {detail}");
    assert!(!detail.contains("moved on") && !detail.contains("expired premise"), "no stale-task evidence: {detail}");
    if summary.contains("cas-af23") {
        assert!(detail.contains("cas-af23"), "retain task context: {detail}");
    }
    let unseen = queue.poll_unseen_for_recipient("supervisor", Some(&daemon.session_name), 20).unwrap();
    assert!(unseen.iter().any(|row| row.id == id && row.prompt == body),
        "the recipient must still be able to read the original guidance: {unseen:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn cas_af23_exhausted_merge_request_keeps_actual_task_and_merge_guidance() {
    if !child("cas_af23_exhausted_merge_request_keeps_actual_task_and_merge_guidance") { return; }
    let envelope = MergeRequestEnvelope {
        task_id: "cas-af23".into(), branch_tip: "delivery-tip".into(),
        target_branch: "epic/v46".into(), target_branch_tip: "target-tip".into(),
        anchor_tip: None, commits_not_on_target_base: 1, pr_number: None,
        source_freshness_unavailable: None,
    };
    exhausted_message(attach_merge_request_envelope(
        "Merge delivery-tip into epic/v46 for cas-af23; then re-close.", &envelope,
    ), "merge cas-af23").await;
}

#[tokio::test(flavor = "current_thread")]
async fn cas_af23_exhausted_status_about_open_task_remains_readable() {
    if !child("cas_af23_exhausted_status_about_open_task_remains_readable") { return; }
    exhausted_message(attach_worker_action_envelope(
        "cas-af23 verification timed out; supervisor recovery required.",
        WorkerActionKind::Status, "worker", None,
    ), "cas-af23 verification timed out").await;
}

#[tokio::test(flavor = "current_thread")]
async fn cas_af23_exhausted_blocker_about_open_task_remains_readable() {
    if !child("cas_af23_exhausted_blocker_about_open_task_remains_readable") { return; }
    exhausted_message(attach_blocker_envelope(
        "cas-af23 verification timed out; supervisor recovery required.", "worker", Some("cas-af23"),
    ), "cas-af23 blocked on verification").await;
}

#[tokio::test(flavor = "current_thread")]
async fn cas_af23_exhausted_unknown_task_blocker_fails_open() {
    if !child("cas_af23_exhausted_unknown_task_blocker_fails_open") { return; }
    exhausted_message(attach_blocker_envelope(
        "Need supervisor help; the task could not be resolved.", "worker", None,
    ), "unresolved blocker").await;
}
