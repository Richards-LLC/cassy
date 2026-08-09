//! Regression tests for explicit atomic overlap resolution in `remember`.

use crate::support::*;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;

fn structured_memory(title: &str, module: &str, body: &str) -> String {
    format!(
        "---\nname: {title}\ndescription: {title}\ntrack: bug\nmodule: {module}\nproblem_type: runtime_error\nseverity: high\nroot_cause: race_condition\n---\n\n## Problem\n{body}\n"
    )
}

fn request(content: String, title: &str, tags: &str) -> RememberRequest {
    RememberRequest {
        scope: "project".to_string(),
        content,
        entry_type: "learning".to_string(),
        tags: Some(tags.to_string()),
        title: Some(title.to_string()),
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    }
}

#[tokio::test]
async fn autofix_merges_high_overlap_into_surviving_entry_with_receipt() {
    let (_temp, service) = setup_cas();
    let title = "sqlite wal ntfs3";
    let tags = "sqlite-wal,ntfs3-fs,mcp-timeout";

    let mut seed = request(
        structured_memory(title, "cas-mcp", "original analysis"),
        title,
        tags,
    );
    seed.bypass_overlap = Some(true);
    let seed_result = service.cas_remember(Parameters(seed)).await.unwrap();
    let seed_slug = seed_result.structured_content.unwrap()["slug"]
        .as_str()
        .unwrap()
        .to_string();

    let mut merge = request(
        structured_memory(title, "cas-mcp", "replacement analysis with new evidence"),
        title,
        tags,
    );
    merge.mode = Some("autofix".to_string());
    let result = service.cas_remember(Parameters(merge)).await.unwrap();

    assert_eq!(result.is_error, Some(false));
    let response = result.structured_content.unwrap();
    assert_eq!(response["status"], "merged");
    assert_eq!(response["slug"], seed_slug);
    assert_eq!(response["receipt"]["merged_into"], response["slug"]);
    assert!(response["receipt"]["updated_at"].as_str().is_some());

    let stored = service
        .cas_get(Parameters(IdRequest { id: seed_slug }))
        .await
        .unwrap();
    assert!(extract_text(stored).contains("replacement analysis with new evidence"));
}

#[tokio::test]
async fn autofix_rejects_a_stale_expected_updated_at_without_mutating() {
    let (_temp, service) = setup_cas();
    let title = "sqlite wal ntfs3";
    let tags = "sqlite-wal,ntfs3-fs,mcp-timeout";

    let mut seed = request(
        structured_memory(title, "cas-mcp", "original analysis"),
        title,
        tags,
    );
    seed.bypass_overlap = Some(true);
    let seed_result = service.cas_remember(Parameters(seed)).await.unwrap();
    let seed_slug = seed_result.structured_content.unwrap()["slug"]
        .as_str()
        .unwrap()
        .to_string();

    let mut merge = request(
        structured_memory(title, "cas-mcp", "must not overwrite original analysis"),
        title,
        tags,
    );
    merge.mode = Some("autofix".to_string());
    merge.expected_updated_at = Some("1970-01-01T00:00:00+00:00".to_string());
    let result = service.cas_remember(Parameters(merge)).await.unwrap();

    assert_eq!(result.is_error, Some(true));
    let response = result.structured_content.unwrap();
    assert_eq!(response["status"], "conflict");
    assert_eq!(response["slug"], seed_slug);
    assert_eq!(response["expected_updated_at"], "1970-01-01T00:00:00+00:00");

    let stored = service
        .cas_get(Parameters(IdRequest { id: seed_slug }))
        .await
        .unwrap();
    let text = extract_text(stored);
    assert!(text.contains("original analysis"));
    assert!(!text.contains("must not overwrite original analysis"));
}

#[tokio::test]
async fn interactive_high_overlap_remains_blocked() {
    let (_temp, service) = setup_cas();
    let title = "sqlite wal ntfs3";
    let tags = "sqlite-wal,ntfs3-fs,mcp-timeout";
    let content = structured_memory(title, "cas-mcp", "original analysis");

    let mut seed = request(content.clone(), title, tags);
    seed.bypass_overlap = Some(true);
    service.cas_remember(Parameters(seed)).await.unwrap();

    let mut duplicate = request(content, title, tags);
    duplicate.mode = Some("interactive".to_string());
    let result = service.cas_remember(Parameters(duplicate)).await.unwrap();

    assert_eq!(result.is_error, Some(true));
    assert_eq!(result.structured_content.unwrap()["status"], "blocked");
}
