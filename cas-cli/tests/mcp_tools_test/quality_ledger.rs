//! Friction-ledger emitters reached through the task and memory MCP tools.
use crate::support::*;
use cas::mcp::tools::*;
use cas_store::{FeedbackSource, FeedbackVerdict, QualityEvent, QualityPayload};
use rmcp::handler::server::wrapper::Parameters;

fn ledger(temp: &tempfile::TempDir) -> Vec<QualityEvent> {
    cas_store::quality_events_between(&temp.path().join(".cas"), None, None).unwrap()
}

fn bug(title: &str, labels: &str) -> TaskCreateRequest {
    serde_json::from_value(serde_json::json!({
        "title": title, "task_type": "bug", "priority": 2, "risk": "none", "labels": labels,
        "scope": "project",
    }))
    .unwrap()
}

/// A bug tagged with the release it escaped into is an escape, whether the tag
/// arrives at create or later. An untagged bug is not.
#[tokio::test]
async fn bug_tagged_with_a_release_is_an_escape() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, service) = setup_cas(&mut test_env);

    service
        .cas_task_create(Parameters(bug("Tagged at create", "found-in:v3.93.0,missed-by:qa")))
        .await
        .unwrap();
    let created = service
        .cas_task_create(Parameters(bug("Tagged later", "area")))
        .await
        .unwrap();
    assert!(ledger(&temp).len() == 1, "an untagged bug is no escape");

    let text = extract_text(created);
    let id = text.split_whitespace().nth(2).unwrap();
    let update: TaskUpdateRequest = serde_json::from_value(serde_json::json!({
        "id": id, "labels": "found-in:3.92.0",
    }))
    .unwrap();
    service.cas_task_update(Parameters(update)).await.unwrap();

    let events = ledger(&temp);
    let found: Vec<_> = events
        .iter()
        .map(|event| match &event.payload {
            QualityPayload::Escape { found_in, missed_stage } => (found_in.as_str(), *missed_stage),
            other => panic!("not an escape: {other:?}"),
        })
        .collect();
    assert_eq!(
        found,
        [("v3.93.0", Some(cas_store::CheckStage::Qa)), ("v3.92.0", None)]
    );
    assert_eq!(events[1].title.as_deref(), Some("Tagged later"));
}

/// A handoff line is a verdict; prose and malformed lines are not, and the note
/// stays out of the wire form.
#[tokio::test]
async fn handoff_lines_become_feature_verdicts() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, service) = setup_cas(&mut test_env);
    let content = "Shipped 3.93.\n\
        feature_feedback: ambient_recall noise 283 injected, none used\n\
        feature_feedback: handoff helped\n\
        feature_feedback: search brilliant\n\
        We discussed feature_feedback: at length.\n";
    let request: RememberRequest = serde_json::from_value(serde_json::json!({
        "scope": "project", "content": content, "entry_type": "handoff", "importance": 0.5,
        "title": "Session handoff",
    }))
    .unwrap();
    let saved = extract_text(service.cas_remember(Parameters(request)).await.unwrap());
    assert!(saved.contains("Recorded 2 feature_feedback"), "{saved}");

    let events = ledger(&temp);
    assert_eq!(events.len(), 2, "{events:?}");
    let QualityPayload::FeatureFeedback { feature, verdict, source, .. } = &events[0].payload else {
        panic!("{:?}", events[0].payload);
    };
    assert_eq!(
        (feature.as_str(), *verdict, *source),
        ("ambient_recall", FeedbackVerdict::Noise, FeedbackSource::Supervisor)
    );
    assert_eq!(events[0].local_note.as_deref(), Some("283 injected, none used"));
    assert!(!serde_json::to_string(&events[0].to_wire()).unwrap().contains("283"));
}
