use crate::support::*;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;

#[tokio::test]
async fn test_remember_basic() {
    let (_temp, service) = setup_cas();

    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Test memory content".to_string(),
        entry_type: "learning".to_string(),
        tags: Some("test,memory".to_string()),
        title: Some("Test Title".to_string()),
        importance: 0.7,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    assert!(text.contains("Created entry"));
    assert!(extract_entry_id(&text).is_some(), "Should contain entry ID");
}

#[tokio::test]
async fn test_remember_with_defaults() {
    let (_temp, service) = setup_cas();

    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Simple memory".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    assert!(text.contains("Created entry"));
}

#[tokio::test]
async fn cas_4caa_remember_derives_valid_until_from_until_deadline() {
    use cas::store::open_store;
    use chrono::{Datelike, Timelike};

    let (_temp, service) = setup_cas();
    let cas_dir = service.project_path().to_path_buf();

    let result = service
        .cas_remember(Parameters(RememberRequest {
            scope: "project".to_string(),
            content: "Do not schedule Codex workers until Aug 8 10:07".to_string(),
            entry_type: "context".to_string(),
            tags: None,
            title: Some("temporary worker restriction".to_string()),
            importance: 0.8,
            valid_from: None,
            valid_until: None,
            team_id: None,
            bypass_overlap: None,
            mode: None,
            expected_updated_at: None,
            personal: None,
        }))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    let store = open_store(&cas_dir).expect("open store");
    let entry = store
        .get(extract_entry_id(&text).expect("entry id"))
        .expect("stored entry");
    let valid_until = entry.valid_until.expect("derived valid_until");
    assert_eq!((valid_until.month(), valid_until.day()), (8, 8));
    assert_eq!((valid_until.hour(), valid_until.minute()), (10, 7));
}

#[tokio::test]
async fn test_get_entry() {
    let (_temp, service) = setup_cas();

    // First create an entry
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Test get content".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    let id = extract_entry_id(&text).expect("should have ID");

    // Now get the entry
    let get_req = IdRequest { id: id.to_string() };

    let result = service
        .cas_get(Parameters(get_req))
        .await
        .expect("get should succeed");

    let text = extract_text(result);
    assert!(text.contains("Test get content"));
    assert!(text.contains("Learning"));
}

#[tokio::test]
async fn test_get_nonexistent_entry() {
    let (_temp, service) = setup_cas();

    let req = IdRequest {
        id: "nonexistent-id".to_string(),
    };

    let result = service.cas_get(Parameters(req)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_update_entry() {
    let (_temp, service) = setup_cas();

    // Create entry
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Original content".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    let id = extract_entry_id(&text).expect("should have ID");

    // Update entry
    let update_req = EntryUpdateRequest {
        id: id.to_string(),
        content: Some("Updated content".to_string()),
        tags: Some("updated,test".to_string()),
        importance: Some(0.9),
    };

    let result = service
        .cas_update(Parameters(update_req))
        .await
        .expect("update should succeed");

    let text = extract_text(result);
    assert!(text.contains("Updated"));
    assert!(text.contains("content"));

    // Verify update
    let get_req = IdRequest { id: id.to_string() };

    let result = service
        .cas_get(Parameters(get_req))
        .await
        .expect("get should succeed");

    let text = extract_text(result);
    assert!(text.contains("Updated content"));
}

#[tokio::test]
async fn test_archive_and_unarchive() {
    let (_temp, service) = setup_cas();

    // Create entry
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Archive test".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    let id = extract_entry_id(&text).expect("should have ID");

    // Archive
    let archive_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_archive(Parameters(archive_req))
        .await
        .expect("archive should succeed");

    let text = extract_text(result);
    assert!(text.contains("Archived"));

    // Unarchive
    let unarchive_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_unarchive(Parameters(unarchive_req))
        .await
        .expect("unarchive should succeed");

    let text = extract_text(result);
    assert!(text.contains("Restored"));
}

#[tokio::test]
async fn test_helpful_and_harmful() {
    let (_temp, service) = setup_cas();

    // Create entry
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Feedback test".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    let id = extract_entry_id(&text).expect("should have ID");

    // Mark helpful
    let helpful_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_helpful(Parameters(helpful_req))
        .await
        .expect("helpful should succeed");

    let text = extract_text(result);
    assert!(text.contains("helpful"));

    // Mark harmful
    let harmful_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_harmful(Parameters(harmful_req))
        .await
        .expect("harmful should succeed");

    let text = extract_text(result);
    assert!(text.contains("harmful"));
}

#[tokio::test]
async fn test_list_entries() {
    let (_temp, service) = setup_cas();

    // Create a few entries
    for i in 0..3 {
        let req = RememberRequest {
            scope: "project".to_string(),
            content: format!("List test entry {i}"),
            entry_type: "learning".to_string(),
            tags: None,
            title: None,
            importance: 0.5,
            valid_from: None,
            valid_until: None,
            team_id: None,
            bypass_overlap: None,
            mode: None,
            expected_updated_at: None,
            personal: None,
        };
        service
            .cas_remember(Parameters(req))
            .await
            .expect("remember should succeed");
    }

    // List entries
    let list_req = LimitRequest {
        scope: "all".to_string(),
        limit: Some(10),
        sort: None,
        sort_order: None,
        team_id: None,
    };
    let result = service
        .cas_list(Parameters(list_req))
        .await
        .expect("list should succeed");

    let text = extract_text(result);
    assert!(text.contains("Entries"));
    assert!(text.contains("List test entry"));
}

#[tokio::test]
async fn test_recent_entries() {
    let (_temp, service) = setup_cas();

    // Create entries
    for i in 0..3 {
        let req = RememberRequest {
            scope: "project".to_string(),
            content: format!("Recent test entry {i}"),
            entry_type: "learning".to_string(),
            tags: None,
            title: None,
            importance: 0.5,
            valid_from: None,
            valid_until: None,
            team_id: None,
            bypass_overlap: None,
            mode: None,
            expected_updated_at: None,
            personal: None,
        };
        service
            .cas_remember(Parameters(req))
            .await
            .expect("remember should succeed");
    }

    // Get recent
    let recent_req = RecentRequest { n: 5 };
    let result = service
        .cas_recent(Parameters(recent_req))
        .await
        .expect("recent should succeed");

    let text = extract_text(result);
    assert!(text.contains("Recent entries"));
}

#[tokio::test]
async fn test_delete_entry() {
    let (_temp, service) = setup_cas();

    // Create entry
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Delete test".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    let id = extract_entry_id(&text).expect("should have ID");

    // Delete
    let delete_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_delete(Parameters(delete_req))
        .await
        .expect("delete should succeed");

    let text = extract_text(result);
    assert!(text.contains("Deleted"));

    // Verify deleted
    let get_req = IdRequest { id: id.to_string() };
    let result = service.cas_get(Parameters(get_req)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_set_tier() {
    let (_temp, service) = setup_cas();

    // Create entry
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Tier test".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    let id = extract_entry_id(&text).expect("should have ID");

    // Set tier to cold
    let tier_req = MemoryTierRequest {
        id: id.to_string(),
        tier: "cold".to_string(),
    };
    let result = service
        .cas_set_tier(Parameters(tier_req))
        .await
        .expect("set_tier should succeed");

    let text = extract_text(result);
    assert!(text.contains("cold") || text.contains("tier"));
}

// ============================================================================
// Pre-insert overlap detection (cas-4721)
// ============================================================================

fn frontmatter_memory(title: &str, module: &str, body: &str) -> String {
    format!(
        "---\nname: {title}\ndescription: {title}\ntrack: bug\nmodule: {module}\nproblem_type: runtime_error\nseverity: high\nroot_cause: race_condition\ndate: 2026-04-09\n---\n\n## Problem\n{body}\n"
    )
}

#[tokio::test]
async fn test_overlap_blocks_duplicate_insert() {
    let (_temp, service) = setup_cas();

    let body =
        "sqlite wal hangs on ntfs3 in cas-mcp/src/server.rs due to posix_lock incompatibility";
    let content = frontmatter_memory("sqlite wal ntfs3 timeout", "cas-mcp", body);

    let first = RememberRequest {
        scope: "project".to_string(),
        content: content.clone(),
        entry_type: "learning".to_string(),
        tags: Some("sqlite-wal,ntfs3-fs,mcp-timeout".to_string()),
        title: Some("sqlite wal ntfs3 timeout".to_string()),
        importance: 0.7,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: Some(true),
        mode: None,
        expected_updated_at: None,
        personal: None,
    };
    service
        .cas_remember(Parameters(first))
        .await
        .expect("first insert should succeed (bypass_overlap=true)");

    // Second insert — identical content, no bypass. Should be blocked.
    let second = RememberRequest {
        scope: "project".to_string(),
        content,
        entry_type: "learning".to_string(),
        tags: Some("sqlite-wal,ntfs3-fs,mcp-timeout".to_string()),
        title: Some("sqlite wal ntfs3 timeout".to_string()),
        importance: 0.7,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };
    let result = service
        .cas_remember(Parameters(second))
        .await
        .expect("duplicate block is now returned as Ok(CallToolResult{is_error:true}) (cas-e382)");
    assert_eq!(
        result.is_error,
        Some(true),
        "block result must carry is_error=true"
    );
    let structured = result
        .structured_content
        .as_ref()
        .expect("blocked response must carry structured_content");
    assert_eq!(structured["status"], "blocked");
    assert_eq!(structured["reason"], "high_overlap");
    assert!(structured["existing_slug"].as_str().is_some());
    assert!(structured["dimension_scores"]["net"].as_u64().unwrap() >= 4);
    let text = extract_text(result);
    assert!(
        text.contains("Overlap detected"),
        "text fallback preserved: {text}"
    );
    assert!(
        text.contains("Existing slug:"),
        "text fallback preserved: {text}"
    );
}

#[tokio::test]
async fn test_bypass_overlap_allows_duplicate() {
    let (_temp, service) = setup_cas();

    let content = frontmatter_memory(
        "duplicate memory",
        "cas-core",
        "identical body referencing cas-core/src/dedup.rs and search_candidates_by_module",
    );

    for _ in 0..2 {
        let req = RememberRequest {
            scope: "project".to_string(),
            content: content.clone(),
            entry_type: "learning".to_string(),
            tags: Some("sqlite-wal,ntfs3-fs".to_string()),
            title: Some("duplicate memory".to_string()),
            importance: 0.5,
            valid_from: None,
            valid_until: None,
            team_id: None,
            bypass_overlap: Some(true),
            mode: None,
            expected_updated_at: None,
            personal: None,
        };
        service
            .cas_remember(Parameters(req))
            .await
            .expect("bypass=true should always succeed");
    }
}

#[tokio::test]
async fn test_unrelated_memory_inserts_normally() {
    let (_temp, service) = setup_cas();

    let first = RememberRequest {
        scope: "project".to_string(),
        content: frontmatter_memory(
            "first topic",
            "cas-mcp",
            "unrelated problem about tantivy index shards",
        ),
        entry_type: "learning".to_string(),
        tags: Some("tantivy-index".to_string()),
        title: Some("first topic".to_string()),
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: Some(true),
        mode: None,
        expected_updated_at: None,
        personal: None,
    };
    service
        .cas_remember(Parameters(first))
        .await
        .expect("first insert should succeed");

    let second = RememberRequest {
        scope: "project".to_string(),
        content: frontmatter_memory(
            "completely different subject",
            "cas-core",
            "entirely different problem about hook context building",
        ),
        entry_type: "learning".to_string(),
        tags: Some("hook-context".to_string()),
        title: Some("completely different subject".to_string()),
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };
    let result = service
        .cas_remember(Parameters(second))
        .await
        .expect("unrelated insert should succeed");
    let text = extract_text(result);
    assert!(
        text.contains("Created entry"),
        "expected success, got: {text}"
    );
}

// ============================================================================
// cas-442e — End-to-end integration: cross-reference (moderate overlap) path
//
// memory_remember_contract.rs already covers Created (low), Blocked (high),
// bypass_overlap, and the mode parameter. This test exercises the one path
// those do not: a moderate-overlap insert that proceeds with bidirectional
// cross-references populated on the Created response's related_memories.
// ============================================================================
#[tokio::test]
async fn test_moderate_overlap_creates_with_crossref() {
    let (_temp, service) = setup_cas();

    // First memory: a bug-track entry in cas-mcp. Seed with bypass so we
    // don't race the overlap gate on the first insert.
    let first_content = frontmatter_memory(
        "shared tantivy reload behavior",
        "cas-mcp",
        "reload_policy matters for cas-mcp/src/server.rs when the writer commits",
    );
    let first = RememberRequest {
        scope: "project".to_string(),
        content: first_content,
        entry_type: "learning".to_string(),
        tags: Some("tantivy-index,reload-policy".to_string()),
        title: Some("shared tantivy reload behavior".to_string()),
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: Some(true),
        mode: None,
        expected_updated_at: None,
        personal: None,
    };
    let first_result = service
        .cas_remember(Parameters(first))
        .await
        .expect("first insert should succeed");
    let first_slug = first_result
        .structured_content
        .as_ref()
        .expect("Created carries structured_content")
        .get("slug")
        .and_then(|v| v.as_str())
        .expect("slug present")
        .to_string();

    // Second memory: shares the same title (problem-statement match) and
    // the same central file ref, but lives in a different module (cas-core)
    // on a different track (knowledge vs bug) with a different root_cause.
    // That puts the raw score at ~4 and the two penalties (-1 module, -1
    // track) drop the net into the 2-3 moderate band.
    let second_content = format!(
        "---\nname: shared tantivy reload behavior\ndescription: reload_policy reload behavior\ntrack: knowledge\nmodule: cas-core\nproblem_type: best_practice\nseverity: medium\nroot_cause: config_error\ndate: 2026-04-09\n---\n\n## Guidance\nreload_policy impacts cas-mcp/src/server.rs on concurrent commits\n"
    );
    let second = RememberRequest {
        scope: "project".to_string(),
        content: second_content,
        entry_type: "learning".to_string(),
        tags: Some("tantivy-index,hook-context".to_string()),
        title: Some("shared tantivy reload behavior".to_string()),
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };
    let second_result = service
        .cas_remember(Parameters(second))
        .await
        .expect("moderate overlap should still succeed");

    assert_eq!(
        second_result.is_error,
        Some(false),
        "moderate overlap is not a block"
    );
    let structured = second_result
        .structured_content
        .as_ref()
        .expect("Created carries structured_content");
    assert_eq!(structured["status"], "created");
    let related = structured["related_memories"]
        .as_array()
        .expect("related_memories is an array");
    // The moderate-overlap path must populate related_memories with the
    // existing slug(s) the new entry cross-referenced.
    assert!(
        !related.is_empty(),
        "moderate overlap must record at least one cross-reference (got: {structured:?})"
    );
    assert!(
        related
            .iter()
            .any(|v| v.as_str() == Some(first_slug.as_str())),
        "cross-ref should include the first memory's slug {first_slug}"
    );
}

// ============================================================================
// Team auto-promote regression tests (cas-6d96)
//
// Verify that `cas_remember()` in a team-linked project defaults to
// team scope, and that `personal=true` opts back out.
// ============================================================================

/// In a project with an active team configured, `cas remember` with no
/// explicit `team_id` must auto-fill `team_id` from `CloudConfig.active_team_id()`.
#[tokio::test]
async fn test_remember_team_linked_project_auto_promotes_to_team() {
    use cas::cloud::CloudConfig;
    use cas::store::open_store;

    let (temp, service) = setup_cas();
    let cas_dir = service.project_path().to_path_buf();

    const TEAM_ID: &str = "auto-promote-team-0000-000000000001";

    // Write a CloudConfig with a team_id into .cas/cloud.json
    let mut cloud_cfg = CloudConfig::default();
    cloud_cfg.set_team(TEAM_ID, "auto-promote-squad");
    cloud_cfg
        .save_to_cas_dir(&cas_dir)
        .expect("save cloud config");

    // Remember without explicit team_id — should auto-promote.
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "team auto-promote regression test".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: Some("team-auto-promote".to_string()),
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    assert!(text.contains("Created entry"), "expected success: {text}");

    // Extract the slug and read back the entry to check team_id.
    let slug = extract_entry_id(&text).expect("slug in output");
    let store = open_store(&cas_dir).expect("open store");
    let entry = store.get(slug).expect("entry must exist");

    assert_eq!(
        entry.team_id.as_deref(),
        Some(TEAM_ID),
        "team_id must be auto-promoted to {TEAM_ID}, got {:?}",
        entry.team_id
    );

    // Keep temp alive until end of test.
    drop(temp);
}

/// `personal=true` opts out of team auto-promote even in a team-linked project.
#[tokio::test]
async fn test_remember_personal_flag_opts_out_of_team_auto_promote() {
    use cas::cloud::CloudConfig;
    use cas::store::open_store;

    let (temp, service) = setup_cas();
    let cas_dir = service.project_path().to_path_buf();

    const TEAM_ID: &str = "personal-opt-out-team-0000-000000000002";

    let mut cloud_cfg = CloudConfig::default();
    cloud_cfg.set_team(TEAM_ID, "opt-out-squad");
    cloud_cfg
        .save_to_cas_dir(&cas_dir)
        .expect("save cloud config");

    // Remember with personal=true — must NOT auto-promote.
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "personal opt-out regression test".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: Some("personal-opt-out".to_string()),
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: Some(true),
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    assert!(text.contains("Created entry"), "expected success: {text}");

    let slug = extract_entry_id(&text).expect("slug in output");
    let store = open_store(&cas_dir).expect("open store");
    let entry = store.get(slug).expect("entry must exist");

    assert_eq!(
        entry.team_id, None,
        "personal=true must keep team_id=None even in a team-linked project"
    );

    drop(temp);
}

/// Explicit `team_id` in the request wins over auto-promote.
#[tokio::test]
async fn test_remember_explicit_team_id_wins_over_auto_promote() {
    use cas::cloud::CloudConfig;
    use cas::store::open_store;

    let (temp, service) = setup_cas();
    let cas_dir = service.project_path().to_path_buf();

    const AUTO_TEAM: &str = "auto-team-0000-000000000003";
    const EXPLICIT_TEAM: &str = "explicit-team-0000-000000000003";

    let mut cloud_cfg = CloudConfig::default();
    cloud_cfg.set_team(AUTO_TEAM, "auto-squad");
    cloud_cfg
        .save_to_cas_dir(&cas_dir)
        .expect("save cloud config");

    let req = RememberRequest {
        scope: "project".to_string(),
        content: "explicit team_id wins test".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: Some("explicit-team-id".to_string()),
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: Some(EXPLICIT_TEAM.to_string()),
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    let result = service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let text = extract_text(result);
    assert!(text.contains("Created entry"), "expected success: {text}");

    let slug = extract_entry_id(&text).expect("slug in output");
    let store = open_store(&cas_dir).expect("open store");
    let entry = store.get(slug).expect("entry must exist");

    assert_eq!(
        entry.team_id.as_deref(),
        Some(EXPLICIT_TEAM),
        "explicit team_id must win over auto-promote; got {:?}",
        entry.team_id
    );

    drop(temp);
}
