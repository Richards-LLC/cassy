//! Public recovery guidance for exact verification-dispatch timeouts.

use cas::mcp::{CasCore, CasService};
use cas::store::{init_cas_dir, open_task_store};
use cas::types::{Task, TaskStatus};
use cas_mcp::types::TaskRequest;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use rusqlite::Connection;

fn task_request(value: serde_json::Value) -> TaskRequest {
    serde_json::from_value(value).expect("valid public task request")
}

fn result_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match &content.raw {
            RawContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn close_recovery_names_only_the_dispatch_it_atomically_timed_out() {
    let root = tempfile::tempdir().expect("temporary CAS root");
    let cas_root = init_cas_dir(root.path()).expect("initialize CAS");
    std::fs::write(
        cas_root.join("config.toml"),
        "[verification]\nenabled = true\n[worktrees]\nenabled = false\n",
    )
    .expect("write config");
    let task_store = open_task_store(&cas_root).expect("task store");
    let mut task = Task::new(
        "cas-timeout-public".to_string(),
        "Return the exact timed-out dispatch".to_string(),
    );
    task.status = TaskStatus::InProgress;
    task.pending_verification = true;
    task_store.add(&task).expect("add task");
    let due = cas_store::create_verification_dispatch(
        &cas_root,
        &task.id,
        "requester-original",
        "owner-original",
        chrono::Utc::now() - chrono::Duration::minutes(1),
    )
    .expect("create due dispatch");

    let conn = Connection::open(cas_root.join("cas.db")).expect("open cas.db");
    conn.execute_batch(
        "CREATE TRIGGER create_public_replacement_after_timeout
         AFTER UPDATE OF state ON verification_dispatches
         WHEN NEW.id = OLD.id
              AND NEW.task_id = 'cas-timeout-public'
              AND OLD.state IN ('pending', 'claimed')
              AND NEW.state = 'timed_out'
         BEGIN
             INSERT INTO verification_dispatches
                 (id, task_id, requester_agent_id, owner_agent_id, state,
                  requested_at, deadline_at, recovery_action)
             VALUES
                 ('vdispatch-public-replacement', 'cas-timeout-public',
                  'requester-replacement', 'owner-replacement', 'pending',
                  '2098-01-01T00:00:00+00:00', '2099-01-01T00:00:00+00:00',
                  'supervisor_redispatch_or_direct');
         END;",
    )
    .expect("install deterministic replacement race");
    drop(conn);

    let service = CasService::new(CasCore::with_daemon(cas_root.clone(), None, None), None);
    let result = service
        .task(Parameters(task_request(serde_json::json!({
            "action": "close",
            "id": task.id,
        }))))
        .await
        .expect("close returns recovery guidance");
    let text = result_text(&result);
    assert!(text.contains("VERIFICATION TIMED OUT"), "{text}");
    assert!(
        text.contains(&due.id),
        "guidance must name the dispatch actually marked timed_out: {text}"
    );
    assert!(
        !text.contains("vdispatch-public-replacement"),
        "guidance must not name the concurrently-created replacement: {text}"
    );
    assert_eq!(
        cas_store::get_verification_dispatch(&cas_root, &due.id)
            .expect("load timed-out dispatch")
            .state,
        cas::types::VerificationDispatchState::TimedOut
    );
    assert_eq!(
        cas_store::get_verification_dispatch(&cas_root, "vdispatch-public-replacement")
            .expect("load replacement")
            .state,
        cas::types::VerificationDispatchState::Pending
    );
}
