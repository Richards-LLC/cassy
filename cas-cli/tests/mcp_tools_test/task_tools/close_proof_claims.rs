use crate::support::*;
use cas::mcp::tools::*;
use cas::store::open_task_store;
use cas::types::TaskStatus;
use rmcp::handler::server::wrapper::Parameters;

/// cas-b8e8: a close reason that claims someone else's or future proof needs
/// a receipt; deferrals, receipts and honest limits close. One project, one
/// fresh task per reason.
#[tokio::test]
async fn cas_b8e8_close_proof_claims_need_a_receipt() {
    let mut env = TestEnvGuard::temp_home();
    let (temp, service) = setup_cas(&mut env);
    let unified = cas::mcp::CasService::new(service.clone(), None);
    let root = temp.path().join(".cas");
    std::fs::write(
        root.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .unwrap();
    cas_store::SqliteArtifactStore::open(&root)
        .unwrap()
        .record_local(&cas_store::NewArtifact {
            id: "art-1234abcd".into(),
            task_id: "cas-proof".into(),
            name: "receipt.log".into(),
            mime: "text/plain".into(),
            size_bytes: 1,
            sha256: "a".repeat(64),
        })
        .unwrap();
    std::fs::write(root.join("Proof.log"), "PASS\n").unwrap();
    std::fs::write(env.home().join("Proof.log"), "PASS\n").unwrap();
    let refused = [
        "Supervisor reports the merge-queue journeys passed",
        "Merge queue journeys will pass after this task closes",
        // Unrelated receipts and deferrals do not cover the claim.
        "Local tests passed; artifact://art-1234abcd. Supervisor reports journeys passed",
        "Local tests passed artifact://art-1234abcd; supervisor reports journeys passed",
        "Journeys deferred to merge queue; supervisor reports journeys passed",
        "Supervisor reports journeys passed; run id pending",
        "Supervisor reports journeys passed; GitHub issue #1235",
        "QA says the tests passed",
        "QA reports the tests succeeded",
        "Supervisor reports tests passed, build not verified",
        "The merge queue will run the journeys after close",
    ];
    let accepted = [
        "Implementation complete; merge-queue journeys deferred to merge queue",
        "Supervisor reports the journeys passed; run id 123456789",
        "Supervisor reports journeys passed (artifact://art-1234abcd)",
        "Named tests passed 4/4; full Rust suite deferred to epic assembly",
        "Removed the supervisor reports formatter",
        "Supervisor reports tests passed. Run id 123456789",
        "Reviewer confirms tests passed\nEvidence: artifact://art-1234abcd",
        "CI green https://github.com/example/repo/actions/runs/123456789",
        "Supervisor reports tests passed (run_id=123456789)",
        "Supervisor reports tests passed (CI 123456789)",
        "Supervisor reports tests passed (run id 12)",
        "The test will not run; proof deferred to epic assembly",
        "Supervisor has not verified the tests; proof deferred to epic assembly",
        "Full Rust suite deferred to epic assembly where it will run",
        "Supervisor reports tests passed; evidence: `.cas/Proof.log`.",
        "QA reports tests passed (~/Proof.log)",
    ];
    let missing = [
        (
            "Supervisor reports tests passed art-deadbeef",
            "art-deadbeef",
        ),
        (
            "QA reports tests passed; evidence: .cas/Missing.log",
            ".cas/Missing.log",
        ),
        (
            "QA reports tests passed art-1234abcd .cas/Missing.log",
            ".cas/Missing.log",
        ),
        (
            "QA reports tests passed art-1234abcd art-deadbeef",
            "art-deadbeef",
        ),
    ];
    let cases = refused
        .iter()
        .map(|reason| (*reason, Some("deferred to <stage>")))
        .chain(
            missing
                .iter()
                .map(|(reason, citation)| (*reason, Some(*citation))),
        )
        .chain(accepted.iter().map(|reason| (*reason, None)));
    for (reason, rejection) in cases {
        // One project, one fresh task per reason: the same title by design.
        let request: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
            "action": "create", "title": "Proof claim boundary", "priority": 2,
            "task_type": "task", "risk": "none", "confirm_warning": true
        }))
        .unwrap();
        let created = unified.task(Parameters(request)).await.unwrap();
        let id = extract_task_id(&extract_text(created)).unwrap().to_string();
        service
            .cas_task_start(Parameters(IdRequest { id: id.clone() }))
            .await
            .unwrap();
        // Historical discussion is not a new close claim.
        let store = open_task_store(&root).unwrap();
        let mut task = store.get(&id).unwrap();
        task.notes
            .push_str("\nOld discussion: supervisor reports journeys passed without proof.\n");
        store.update(&task).unwrap();
        let response = service
            .cas_task_close(Parameters(TaskCloseRequest {
                no_delivery: None,
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
        let task = store.get(&id).unwrap();
        if let Some(rejection) = rejection {
            assert!(
                is_error && text.contains("PROOF CLAIM RECEIPT REQUIRED"),
                "{reason}: {text}"
            );
            assert!(text.contains(rejection), "{text}");
            assert_eq!(task.status, TaskStatus::InProgress, "{reason}");
            assert!(
                !task.notes.contains("Closed:"),
                "refusal must precede close writes: {reason}"
            );
            assert!(
                cas_store::get_latest_verification_dispatch(&root, &id)
                    .unwrap()
                    .is_none(),
                "{reason}"
            );
        } else {
            assert!(
                !is_error && text.contains("Closed task:"),
                "{reason}: {text}"
            );
            assert_eq!(task.status, TaskStatus::Closed, "{reason}");
        }
    }
}
