use crate::support::*;
use cas::mcp::tools::*;
use cas::store::open_task_store;
use cas::types::TaskStatus;
use rmcp::handler::server::wrapper::Parameters;

async fn close_with_reason(reason: &str, rejected: bool) {
    let mut env = TestEnvGuard::temp_home();
    let (temp, service) = setup_cas(&mut env);
    let root = temp.path().join(".cas");
    std::fs::write(
        root.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .unwrap();
    let created = service
        .cas_task_create(Parameters(simple_task_req("Proof claim boundary")))
        .await
        .unwrap();
    let id = extract_task_id(&extract_text(created)).unwrap().to_string();
    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .unwrap();
    let response = service
        .cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some(reason.to_string()),
            stranded_branch_override: None,
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .unwrap();
    let is_error = response.is_error == Some(true);
    let text = extract_text(response);
    let task = open_task_store(&root).unwrap().get(&id).unwrap();
    if rejected {
        assert!(
            is_error && text.contains("PROOF CLAIM RECEIPT REQUIRED"),
            "{reason}: {text}"
        );
        assert!(text.contains("deferred to <stage>"), "{text}");
        assert_eq!(task.status, TaskStatus::InProgress);
        assert!(
            !task.notes.contains("Closed:"),
            "refusal must precede close writes"
        );
        assert!(
            cas_store::get_latest_verification_dispatch(&root, &id)
                .unwrap()
                .is_none()
        );
    } else {
        assert!(
            !is_error && text.contains("Closed task:"),
            "{reason}: {text}"
        );
        assert_eq!(task.status, TaskStatus::Closed);
    }
}

#[tokio::test]
async fn cas_b8e8_close_refuses_third_party_proof_without_receipt() {
    close_with_reason("Supervisor reports the merge-queue journeys passed", true).await;
}

#[tokio::test]
async fn cas_b8e8_close_refuses_future_proof_without_receipt() {
    close_with_reason(
        "Merge queue journeys will pass after this task closes",
        true,
    )
    .await;
}

#[tokio::test]
async fn cas_b8e8_deferred_acceptance_and_receipted_claims_close() {
    for reason in [
        "Implementation complete; merge-queue journeys deferred to merge queue",
        "Supervisor reports the journeys passed; run id 123456789",
        "Supervisor reports journeys passed (artifact://art-1234abcd)",
        "Named tests passed 4/4; full Rust suite deferred to epic assembly",
        "Removed the supervisor reports formatter",
    ] {
        close_with_reason(reason, false).await;
    }
}
