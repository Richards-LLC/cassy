//! Verification read and refusal contracts through the current MCP dispatcher.

use std::path::PathBuf;

use cas::mcp::{CasCore, CasService};
use cas::store::{init_cas_dir, open_agent_store, open_task_store_local, open_verification_store};
use cas::types::{
    Agent, AgentRole, Task, TaskStatus, Verification, VerificationProofBoundary, VerificationStatus,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorCode};
use serde_json::{Value, json};

use crate::test_env_guard::TestEnvGuard;

struct Fixture {
    root: PathBuf,
    service: CasService,
}

impl Fixture {
    fn new(env: &mut TestEnvGuard) -> Self {
        let project = env.home().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let root = init_cas_dir(&project).unwrap();
        env.set("CAS_ROOT", &root);
        env.set("XDG_CONFIG_HOME", env.home().join(".config"));
        let core = CasCore::with_daemon(root.clone(), None, None);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        open_task_store_local(&root)
            .unwrap()
            .add(&Task::new(
                "cas-c034".to_string(),
                "Verification fixture".to_string(),
            ))
            .unwrap();
        Self { root, service }
    }

    fn as_agent(&self, agent_id: &str) -> Self {
        let core = CasCore::with_daemon(self.root.clone(), None, None);
        core.set_agent_id_for_testing(agent_id.to_string());
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        Self { root: self.root.clone(), service }
    }

    async fn verification(&self, value: Value) -> Result<CallToolResult, rmcp::ErrorData> {
        self.service
            .verification(Parameters(serde_json::from_value(value).unwrap()))
            .await
    }
}

fn text(result: CallToolResult) -> String {
    assert_ne!(result.is_error, Some(true), "{result:?}");
    result
        .content
        .iter()
        .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn verification_add_and_reads_preserve_all_statuses_issues_and_metadata() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let supervisor_id = "fixture-registered-supervisor";
    let mut supervisor = Agent::new(supervisor_id.to_string(), "fixture-supervisor".to_string());
    supervisor.role = AgentRole::Supervisor;
    open_agent_store(&fixture.root)
        .unwrap()
        .register(&supervisor)
        .unwrap();
    fixture
        .service
        .inner
        .set_agent_id_for_testing(supervisor_id.to_string());
    for (index, (status, expected_status)) in [
        (VerificationStatus::Approved, "approved"),
        (VerificationStatus::Rejected, "rejected"),
        (VerificationStatus::Error, "error"),
        (VerificationStatus::Skipped, "skipped"),
    ]
    .into_iter()
    .enumerate()
    {
        let task_id = format!("cas-c0{index:02}");
        let mut task = Task::new(
            task_id.clone(),
            "Notes-only verification fixture".to_string(),
        );
        task.status = TaskStatus::InProgress;
        open_task_store_local(&fixture.root)
            .unwrap()
            .add(&task)
            .unwrap();
        // This notes-only task has a real exact dispatch and registered supervisor.
        // No repository receipt is fabricated and no worker self-attestation is used.
        let dispatch = cas_store::create_verification_dispatch_bound(
            &fixture.root,
            &task_id,
            supervisor_id,
            supervisor_id,
            &VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10),
            false,
        )
        .unwrap();
        let added = text(fixture.verification(json!({
            "action": "add", "task_id": task_id, "status": expected_status,
            "summary": "Thorough review", "confidence": 0.95,
            "duration_ms": 1500, "files": "src/main.rs,src/lib.rs",
            "issues": json!([
                {"file": "src/main.rs", "line": 42, "severity": "blocking", "category": "todo_comment", "problem": "TODO comment found"},
                {"file": "src/lib.rs", "severity": "warning", "category": "magic_number", "problem": "Magic number detected"}
            ]).to_string(),
            "dispatch_id": dispatch.id,
        })).await.unwrap());
        let persisted = open_verification_store(&fixture.root)
            .unwrap()
            .get_latest_for_task(&task_id)
            .unwrap()
            .unwrap();
        assert!(persisted.id.starts_with("ver-") && persisted.id.len() > 4);
        assert!(added.contains(&persisted.id), "{added}");
        assert_eq!(persisted.dispatch_id.as_deref(), Some(dispatch.id.as_str()));
        assert_eq!(persisted.status, status);
        assert_eq!(persisted.confidence, Some(0.95));
        assert_eq!(persisted.duration_ms, Some(1500));
        assert_eq!(persisted.issues.len(), 2);
        assert_eq!(persisted.files_reviewed, ["src/main.rs", "src/lib.rs"]);
        let shown = text(
            fixture
                .verification(json!({"action": "show", "id": persisted.id}))
                .await
                .unwrap(),
        );
        for expected in [
            persisted.id.as_str(),
            "Summary: Thorough review",
            "Confidence: 95%",
            "Duration: 1500ms",
            "Files Reviewed (2)",
            "src/lib.rs",
            "Issues (1 blocking, 1 warnings)",
            "src/main.rs:42",
            "TODO comment found",
            "Magic number detected",
        ] {
            assert!(shown.contains(expected), "missing {expected:?}: {shown}");
        }
        assert!(shown.contains(&format!("Task: {task_id}")), "{shown}");
        assert!(
            shown.contains(&format!("Status: {expected_status}")),
            "{shown}"
        );
    }
}

#[tokio::test]
async fn verification_list_and_latest_distinguish_empty_history_and_ordered_verdicts() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let empty = text(
        fixture
            .verification(json!({"action": "latest", "task_id": "cas-c034"}))
            .await
            .unwrap(),
    );
    assert_eq!(empty, "No verifications found for task cas-c034");
    let empty_list = text(
        fixture
            .verification(json!({"action": "list", "task_id": "cas-c034"}))
            .await
            .unwrap(),
    );
    assert_eq!(empty_list, "No verifications for task cas-c034");
    let store = open_verification_store(&fixture.root).unwrap();
    let mut approved = Verification::approved(
        "ver-newer".to_string(),
        "cas-c034".to_string(),
        "Issues fixed".to_string(),
    );
    approved.created_at = "2020-01-03T00:00:00Z".parse().unwrap();
    let mut rejected = Verification::new("ver-older".to_string(), "cas-c034".to_string());
    rejected.status = VerificationStatus::Rejected;
    rejected.summary = "Issues found".to_string();
    rejected.created_at = "2020-01-02T00:00:00Z".parse().unwrap();
    // Insert newest first so ordering cannot accidentally follow insertion order.
    store.add(&approved).unwrap();
    store.add(&rejected).unwrap();
    store
        .add(&Verification::new(
            "ver-other".to_string(),
            "cas-ffff".to_string(),
        ))
        .unwrap();

    let listed = text(
        fixture
            .verification(json!({"action": "list", "task_id": "cas-c034"}))
            .await
            .unwrap(),
    );
    let latest = text(
        fixture
            .verification(json!({"action": "latest", "task_id": "cas-c034"}))
            .await
            .unwrap(),
    );

    assert!(listed.contains("2 total"), "{listed}");
    assert!(
        listed.find("ver-newer").unwrap() < listed.find("ver-older").unwrap(),
        "{listed}"
    );
    assert!(!listed.contains("ver-other"), "{listed}");
    for expected in ["ID: ver-newer", "Status: approved", "Summary: Issues fixed"] {
        assert!(latest.contains(expected), "{latest}");
    }
    assert!(!latest.contains("ver-older"));
    assert_eq!(store.get_for_task("cas-c034").unwrap().len(), 2);
}

#[tokio::test]
async fn verification_refuses_missing_rows_and_invalid_status_without_writing() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    for (request, diagnostic) in [
        (
            json!({"action": "add", "task_id": "cas-ffff", "status": "approved", "summary": "Must not write"}),
            "Task not found",
        ),
        (
            json!({"action": "show", "id": "ver-nonexistent"}),
            "Verification not found",
        ),
        (
            json!({"action": "add", "task_id": "cas-c034", "status": "invalid_status", "summary": "Must not write"}),
            "Invalid verification status",
        ),
    ] {
        let error = fixture
            .verification(request)
            .await
            .expect_err("invalid request must fail");
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert!(error.message.contains(diagnostic), "{error:?}");
    }
    let store = open_verification_store(&fixture.root).unwrap();
    assert!(store.get_for_task("cas-c034").unwrap().is_empty());
    assert!(store.get_for_task("cas-ffff").unwrap().is_empty());
    assert!(
        cas_store::get_latest_verification_dispatch(&fixture.root, "cas-c034")
            .unwrap()
            .is_none()
    );
}

/// A question is durable review evidence, but does not reject the delivery or
/// spend the dispatch. The supervisor answers with a verdict on that same id.
#[tokio::test]
async fn supervisor_call_can_be_answered_on_the_same_dispatch() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let agents = open_agent_store(&fixture.root).unwrap();
    let supervisor_id = "question-supervisor";
    let mut supervisor = Agent::new(supervisor_id.into(), supervisor_id.into());
    supervisor.role = AgentRole::Supervisor;
    agents.register(&supervisor).unwrap();
    for (index, mode) in ["supervisor", "capability", "handoff"].into_iter().enumerate() {
        let task_id = format!("cas-question-{index}");
        let mut task = Task::new(task_id.clone(), "Question during review".into());
        task.status = TaskStatus::InProgress;
        task.pending_verification = true;
        open_task_store_local(&fixture.root).unwrap().add(&task).unwrap();
        let dispatch = cas_store::create_verification_dispatch_bound(
            &fixture.root, &task_id, supervisor_id, supervisor_id,
            &VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10), false,
        ).unwrap();
        let mut capability = None;
        let caller = if mode == "supervisor" {
            supervisor_id.to_string()
        } else {
            let child_id = format!("question-verifier-{index}");
            agents.register(&Agent::new_sub_agent(
                child_id.clone(), "task-verifier".into(), supervisor_id.into(),
            )).unwrap();
            if mode == "capability" {
                let issued = cas_store::issue_verifier_capability(&fixture.root, &task_id, supervisor_id).unwrap();
                cas_store::bind_verifier_capability(&fixture.root, &issued.token, &child_id).unwrap();
                cas_store::claim_verification_dispatch(
                    &fixture.root, &task_id, supervisor_id, &child_id, &issued.capability.id,
                ).unwrap();
                capability = Some(issued.token);
            } else {
                cas_store::issue_server_verifier_handoff(
                    &fixture.root, &task_id, &dispatch.id, supervisor_id, &format!("question-tool-{index}"),
                ).unwrap();
                cas_store::bind_server_verifier_handoff(&fixture.root, supervisor_id, &child_id).unwrap();
            }
            child_id
        };
        let question_caller = fixture.as_agent(&caller);
        let question = "Does the recorded scope split cover the unexercised Accounts row?";
        text(question_caller.verification(json!({
            "action":"add", "task_id":task_id, "dispatch_id":dispatch.id,
            "status":"supervisor_call", "summary":question, "verifier_capability":capability,
        })).await.expect("a scope question is accepted"));
        let history = open_verification_store(&fixture.root).unwrap();
        let call = history.get_latest_for_task(&task_id).unwrap().unwrap();
        assert_eq!(call.status.to_string(), "supervisor_call");
        assert_eq!(call.summary, question);
        assert!(open_task_store_local(&fixture.root).unwrap().get(&task_id).unwrap().pending_verification);
        let waiting = cas_store::get_verification_dispatch(&fixture.root, &dispatch.id).unwrap();
        assert!(matches!(waiting.state, cas::types::VerificationDispatchState::Pending | cas::types::VerificationDispatchState::Claimed));
        assert!(cas_store::quality_events_between(&fixture.root, None, None).unwrap().is_empty(), "a question ran no check");

        // A worker cannot turn the question into an approval.
        let worker_id = format!("question-worker-{index}");
        agents.register(&Agent::new(worker_id.clone(), worker_id.clone())).unwrap();
        let worker = fixture.as_agent(&worker_id);
        assert!(worker.verification(json!({
            "action":"add", "task_id":task_id, "dispatch_id":dispatch.id,
            "status":"approved", "summary":"unauthorized answer",
        })).await.is_err());

        let supervisor = fixture.as_agent(supervisor_id);
        let answer = json!({
            "action":"add", "task_id":task_id, "dispatch_id":dispatch.id,
            "status":"approved", "summary":"Yes. The recorded scope split is accepted; review passes.",
        });
        text(supervisor.verification(answer.clone()).await.expect("supervisor answers the existing dispatch"));
        text(supervisor.verification(answer).await.expect("lost-response retry is idempotent"));
        assert_eq!(history.get(&call.id).unwrap().summary, question, "retain the original question");
        let verdict = cas_store::get_verification_for_dispatch(&fixture.root, &dispatch.id).unwrap().unwrap();
        assert_eq!(verdict.status, VerificationStatus::Approved);
        assert!(!open_task_store_local(&fixture.root).unwrap().get(&task_id).unwrap().pending_verification);
        assert_eq!(cas_store::get_verification_dispatch(&fixture.root, &dispatch.id).unwrap().state, cas::types::VerificationDispatchState::Resolved);
        assert!(supervisor.verification(json!({
            "action":"add", "task_id":task_id, "dispatch_id":dispatch.id,
            "status":"rejected", "summary":"conflicting replay",
        })).await.is_err(), "terminal verdict remains single use");
    }
}
