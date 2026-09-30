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
