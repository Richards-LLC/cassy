//! cas-e382 — MCP response contract tests for `action=remember`.
//!
//! Verifies the structured `MemoryRememberResponse` payload carried on
//! `CallToolResult::structured_content`, including:
//!
//! - Created (low overlap)
//! - Created (moderate overlap, cross-refs populated)
//! - Blocked (high overlap, dimension breakdown + recommendation)
//! - `mode=interactive` is equivalent to default
//! - `mode=autofix` remains valid for a low-overlap create
//! - `mode` with an unknown value errors
//! - Serde round-trip / snapshot for each runtime variant

use crate::support::*;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;

fn structured_memory(title: &str, module: &str, body: &str) -> String {
    format!(
        "---\nname: {title}\ndescription: {title}\ntrack: bug\nmodule: {module}\nproblem_type: runtime_error\nseverity: high\nroot_cause: race_condition\ndate: 2026-04-09\n---\n\n## Problem\n{body}\n"
    )
}

fn base_request(content: String, title: &str, tags: &str) -> RememberRequest {
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

// ---------------------------------------------------------------------------
// Runtime tests — drive the handler and inspect CallToolResult
// ---------------------------------------------------------------------------

#[tokio::test]
async fn created_low_overlap_returns_structured_created() {
    let (_temp, service) = setup_cas();

    let req = base_request(
        "totally unrelated memory about nothing in particular".to_string(),
        "solo",
        "solo",
    );
    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("low-overlap insert should succeed");

    assert_eq!(result.is_error, Some(false), "created is not an error");
    let sc = result
        .structured_content
        .as_ref()
        .expect("created response carries structured_content");
    assert_eq!(sc["status"], "created");
    assert!(!sc["slug"].as_str().unwrap().is_empty(), "slug present");
    assert_eq!(
        sc["related_memories"].as_array().unwrap().len(),
        0,
        "low overlap has no cross-refs"
    );
    assert_eq!(sc["refresh_recommended"], serde_json::Value::Bool(false));

    // Deserialize into the strongly-typed enum to exercise the wire format.
    let parsed: MemoryRememberResponse =
        serde_json::from_value(sc.clone()).expect("structured_content deserializes");
    match parsed {
        MemoryRememberResponse::Created {
            related_memories,
            refresh_recommended,
            ..
        } => {
            assert!(related_memories.is_empty());
            assert!(!refresh_recommended);
        }
        other => panic!("expected Created, got {other:?}"),
    }
}

#[tokio::test]
async fn blocked_high_overlap_returns_structured_blocked() {
    let (_temp, service) = setup_cas();

    let body = "sqlite wal hangs on ntfs3 in cas-mcp/src/server.rs because posix_lock is not supported";
    let content = structured_memory("sqlite wal ntfs3", "cas-mcp", body);

    // Seed: bypass the check for the first insert so the store has a
    // candidate to collide with.
    let mut first = base_request(
        content.clone(),
        "sqlite wal ntfs3",
        "sqlite-wal,ntfs3-fs,mcp-timeout",
    );
    first.bypass_overlap = Some(true);
    service
        .cas_remember(Parameters(first))
        .await
        .expect("seed insert with bypass");

    // Second: identical, no bypass — should be blocked and carry a
    // structured Blocked response.
    let second = base_request(
        content,
        "sqlite wal ntfs3",
        "sqlite-wal,ntfs3-fs,mcp-timeout",
    );
    let result = service
        .cas_remember(Parameters(second))
        .await
        .expect("blocks are returned as Ok(CallToolResult{is_error:true})");

    assert_eq!(result.is_error, Some(true));
    let sc = result
        .structured_content
        .as_ref()
        .expect("blocked response carries structured_content");
    assert_eq!(sc["status"], "blocked");
    assert_eq!(sc["reason"], "high_overlap");
    assert!(
        !sc["existing_slug"].as_str().unwrap().is_empty(),
        "existing_slug present"
    );
    let net = sc["dimension_scores"]["net"].as_u64().unwrap();
    assert!(
        (4..=5).contains(&net),
        "net score {net} should be in [4,5] for high overlap"
    );
    assert!(
        sc["dimension_scores"]["problem_statement"]
            .as_u64()
            .is_some()
    );
    assert!(sc["dimension_scores"]["penalty"].as_i64().is_some());
    let action = sc["recommended_action"].as_str().unwrap();
    assert!(
        action == "update_existing" || action == "surface_for_user_decision",
        "unexpected recommended_action: {action}"
    );
    assert!(sc["other_high_scoring"].as_array().is_some());

    let parsed: MemoryRememberResponse =
        serde_json::from_value(sc.clone()).expect("Blocked deserializes into enum");
    assert!(
        matches!(parsed, MemoryRememberResponse::Blocked { .. }),
        "expected Blocked variant from high-overlap path"
    );

    // Text fallback remains for legacy clients.
    let text = extract_text(result);
    assert!(text.contains("Overlap detected"));
    assert!(text.contains("Existing slug:"));
}

#[tokio::test]
async fn mode_interactive_is_equivalent_to_default() {
    let (_temp, service) = setup_cas();

    let mut req = base_request(
        "an interactive mode memory with unique content".to_string(),
        "unique",
        "unique-tag",
    );
    req.mode = Some("interactive".to_string());
    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("interactive is valid");
    assert_eq!(result.is_error, Some(false));
    let sc = result.structured_content.as_ref().unwrap();
    assert_eq!(sc["status"], "created");
}

#[tokio::test]
async fn mode_autofix_allows_low_overlap_create() {
    let (_temp, service) = setup_cas();

    let mut req = base_request(
        "autofix mode attempt".to_string(),
        "autofix",
        "autofix-tag",
    );
    req.mode = Some("autofix".to_string());
    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("autofix is valid and low overlap creates normally");
    assert_eq!(result.is_error, Some(false));
    assert_eq!(result.structured_content.unwrap()["status"], "created");
}

#[tokio::test]
async fn mode_unknown_value_errors() {
    let (_temp, service) = setup_cas();

    let mut req = base_request("unknown mode".to_string(), "x", "x-tag");
    req.mode = Some("yolo".to_string());
    let err = service
        .cas_remember(Parameters(req))
        .await
        .expect_err("unknown mode should error");
    assert!(err.message.to_string().contains("unknown mode"));
}

// ---------------------------------------------------------------------------
// Pure serde round-trip snapshots — lock the wire format
// ---------------------------------------------------------------------------

#[test]
fn snapshot_created_low_overlap() {
    let r = MemoryRememberResponse::Created {
        slug: "cas-aaaa".to_string(),
        related_memories: vec![],
        refresh_recommended: false,
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "status": "created",
            "slug": "cas-aaaa",
            "related_memories": [],
            "refresh_recommended": false,
        })
    );
    let back: MemoryRememberResponse = serde_json::from_value(v).unwrap();
    assert_eq!(back, r);
}

#[test]
fn snapshot_created_with_crossrefs_and_refresh() {
    let r = MemoryRememberResponse::Created {
        slug: "cas-bbbb".to_string(),
        related_memories: vec!["cas-xxxx".to_string(), "cas-yyyy".to_string()],
        refresh_recommended: true,
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "status": "created",
            "slug": "cas-bbbb",
            "related_memories": ["cas-xxxx", "cas-yyyy"],
            "refresh_recommended": true,
        })
    );
    let back: MemoryRememberResponse = serde_json::from_value(v).unwrap();
    assert_eq!(back, r);
}

#[test]
fn snapshot_blocked_high_overlap() {
    let r = MemoryRememberResponse::Blocked {
        reason: BlockReason::HighOverlap,
        existing_slug: "cas-cccc".to_string(),
        dimension_scores: DimensionBreakdown {
            problem_statement: 1,
            root_cause: 1,
            solution_approach: 1,
            referenced_files: 1,
            tags: 0,
            penalty: 0,
            net: 4,
        },
        recommended_action: RecommendedAction::UpdateExisting,
        other_high_scoring: vec!["cas-dddd".to_string()],
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "status": "blocked",
            "reason": "high_overlap",
            "existing_slug": "cas-cccc",
            "dimension_scores": {
                "problem_statement": 1,
                "root_cause": 1,
                "solution_approach": 1,
                "referenced_files": 1,
                "tags": 0,
                "penalty": 0,
                "net": 4,
            },
            "recommended_action": "update_existing",
            "other_high_scoring": ["cas-dddd"],
        })
    );
    let back: MemoryRememberResponse = serde_json::from_value(v).unwrap();
    assert_eq!(back, r);
}

#[test]
fn recommended_action_surface_for_user_decision_round_trips() {
    let r = MemoryRememberResponse::Blocked {
        reason: BlockReason::HighOverlap,
        existing_slug: "cas-ffff".to_string(),
        dimension_scores: DimensionBreakdown {
            problem_statement: 1,
            root_cause: 1,
            solution_approach: 1,
            referenced_files: 1,
            tags: 1,
            penalty: 0,
            net: 5,
        },
        recommended_action: RecommendedAction::SurfaceForUserDecision,
        other_high_scoring: vec![],
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["recommended_action"], "surface_for_user_decision");
    let back: MemoryRememberResponse = serde_json::from_value(v).unwrap();
    assert_eq!(back, r);
}
