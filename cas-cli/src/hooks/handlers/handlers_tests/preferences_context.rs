use crate::hooks::handlers::*;

#[test]
fn test_is_preference_prompt_edge_cases() {
    // Should handle mixed case
    assert!(is_preference_prompt("NEVER add TODO comments"));
    assert!(is_preference_prompt("ALWAYS use strict types"));

    // Should require meaningful content after keyword
    assert!(!is_preference_prompt("never")); // Just the keyword
    assert!(!is_preference_prompt("always")); // Just the keyword
}

// =========================================================================
// Extracted preference structure tests
// =========================================================================

#[test]
fn test_extracted_preference_deserialize() {
    let json = r#"{
        "content": "Never add TODO comments, always implement full functionality",
        "scope": "global",
        "confidence": 0.95
    }"#;

    let pref: ExtractedPreference = serde_json::from_str(json).unwrap();
    assert_eq!(
        pref.content,
        "Never add TODO comments, always implement full functionality"
    );
    assert_eq!(pref.scope, "global");
    assert_eq!(pref.confidence, 0.95);
}

#[test]
fn test_extracted_preference_with_path_pattern() {
    let json = r#"{
        "content": "Use strict TypeScript",
        "scope": "project",
        "confidence": 0.9,
        "path_pattern": "**/*.ts"
    }"#;

    let pref: ExtractedPreference = serde_json::from_str(json).unwrap();
    assert_eq!(pref.path_pattern, Some("**/*.ts".to_string()));
}

#[test]
fn test_generate_file_change_id_format() {
    let id = generate_file_change_id();
    assert!(id.starts_with("fc-"));
    assert!(id.len() > 10); // Should have timestamp and random parts
}

#[test]
fn test_compute_content_hash_consistency() {
    let hash1 = compute_content_hash("test content");
    let hash2 = compute_content_hash("test content");
    let hash3 = compute_content_hash("different content");

    assert_eq!(hash1, hash2); // Same content = same hash
    assert_ne!(hash1, hash3); // Different content = different hash
}

// =============================================================================
// GIT COMMIT DETECTION TESTS
// =============================================================================

#[test]
fn test_is_git_commit_command_simple() {
    assert!(is_git_commit_command("git commit -m \"message\""));
    assert!(is_git_commit_command("git commit -am \"message\""));
    assert!(is_git_commit_command("git commit --message=\"message\""));
}

#[test]
fn test_is_git_commit_command_with_heredoc() {
    let cmd = r#"git commit -m "$(cat <<'EOF'
Commit message here.
EOF
)""#;
    assert!(is_git_commit_command(cmd));
}

#[test]
fn test_is_git_commit_command_not_commit() {
    assert!(!is_git_commit_command("git status"));
    assert!(!is_git_commit_command("git push"));
    assert!(!is_git_commit_command("git add ."));
    assert!(!is_git_commit_command("git log"));
}

#[test]
fn test_is_git_commit_command_dry_run_excluded() {
    assert!(!is_git_commit_command("git commit --dry-run -m \"test\""));
    assert!(!is_git_commit_command("git commit -m \"test\" --dry-run"));
}

#[test]
fn test_extract_commit_hash_standard_output() {
    let stdout = "[main abc1234] Add new feature\n 1 file changed, 10 insertions(+)";
    assert_eq!(extract_commit_hash(stdout), Some("abc1234".to_string()));
}

#[test]
fn test_extract_commit_hash_with_branch_prefix() {
    let stdout = "[feature/auth 9f8e7d6] Implement login\n 3 files changed";
    assert_eq!(extract_commit_hash(stdout), Some("9f8e7d6".to_string()));
}

#[test]
fn test_extract_commit_hash_full_hash() {
    let stdout = "abcdef1234567890abcdef1234567890abcdef12";
    assert_eq!(
        extract_commit_hash(stdout),
        Some("abcdef1234567890abcdef1234567890abcdef12".to_string())
    );
}

#[test]
fn test_extract_commit_hash_no_match() {
    let stdout = "nothing to commit, working tree clean";
    assert_eq!(extract_commit_hash(stdout), None);

    let stdout2 = "error: pathspec 'foo' did not match any file(s)";
    assert_eq!(extract_commit_hash(stdout2), None);
}

#[test]
fn test_extract_commit_message_simple() {
    assert_eq!(
        extract_commit_message("git commit -m \"Add new feature\""),
        Some("Add new feature".to_string())
    );
}

#[test]
fn test_extract_commit_message_single_quotes() {
    assert_eq!(
        extract_commit_message("git commit -m 'Fix bug'"),
        Some("Fix bug".to_string())
    );
}

#[test]
fn test_extract_commit_message_long_flag() {
    assert_eq!(
        extract_commit_message("git commit --message=\"Update docs\""),
        Some("Update docs".to_string())
    );
}

#[test]
fn test_extract_commit_message_heredoc() {
    let cmd = r#"git commit -m "$(cat <<'EOF'
This is a multi-line
commit message
EOF
)""#;
    let result = extract_commit_message(cmd);
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("multi-line"));
}

#[test]
fn test_extract_commit_message_no_message() {
    assert_eq!(extract_commit_message("git commit"), None);
    assert_eq!(extract_commit_message("git status"), None);
}

// =========================================================================
// build_session_summary_context tests
// =========================================================================

#[test]
fn test_build_session_summary_context_disabled() {
    use crate::store::mock::MockStore;

    let store = MockStore::new();
    let config = Config::default();

    // Default config has generate_summary = false
    let result = build_session_summary_context(&store, &config, "test-session");
    assert!(result.is_none());
}

#[test]
fn test_build_session_summary_context_enabled() {
    use crate::store::mock::MockStore;

    let store = MockStore::new();
    let mut config = Config::default();

    // Enable generate_summary by creating a HookConfig with it enabled
    let mut hook_config = crate::config::HookConfig::default();
    hook_config.stop.generate_summary = true;
    config.hooks = Some(hook_config);

    let result = build_session_summary_context(&store, &config, "test-session");
    assert!(result.is_some());
    let context = result.unwrap();
    assert!(context.contains("session-summary required=\"true\""));
    assert!(context.contains("session-summarizer"));
}

#[test]
fn test_build_session_summary_context_already_has_summary() {
    use crate::store::mock::MockStore;

    // Create a store with an existing session summary
    let mut entry = Entry::new(
        "entry-001".to_string(),
        "Previous session summary".to_string(),
    );
    entry.session_id = Some("test-session".to_string());
    entry.tags = vec!["session-summary".to_string()];
    let store = MockStore::with_entries(vec![entry]);

    let mut config = Config::default();
    let mut hook_config = crate::config::HookConfig::default();
    hook_config.stop.generate_summary = true;
    config.hooks = Some(hook_config);

    // Should return None since a summary already exists
    let result = build_session_summary_context(&store, &config, "test-session");
    assert!(result.is_none());
}

#[test]
fn test_build_session_summary_context_different_session() {
    use crate::store::mock::MockStore;

    // Create a store with a summary for a different session
    let mut entry = Entry::new("entry-001".to_string(), "Other session summary".to_string());
    entry.session_id = Some("other-session".to_string());
    entry.tags = vec!["session-summary".to_string()];
    let store = MockStore::with_entries(vec![entry]);

    let mut config = Config::default();
    let mut hook_config = crate::config::HookConfig::default();
    hook_config.stop.generate_summary = true;
    config.hooks = Some(hook_config);

    // Should return Some since no summary exists for "test-session"
    let result = build_session_summary_context(&store, &config, "test-session");
    assert!(result.is_some());
}

#[test]
fn test_build_session_summary_context_has_summary_tag() {
    use crate::store::mock::MockStore;

    // Create a store with an entry that has just the "summary" tag
    let mut entry = Entry::new("entry-001".to_string(), "Summary entry".to_string());
    entry.session_id = Some("test-session".to_string());
    entry.tags = vec!["summary".to_string()];
    let store = MockStore::with_entries(vec![entry]);

    let mut config = Config::default();
    let mut hook_config = crate::config::HookConfig::default();
    hook_config.stop.generate_summary = true;
    config.hooks = Some(hook_config);

    // Should return None since a summary (with "summary" tag) already exists
    let result = build_session_summary_context(&store, &config, "test-session");
    assert!(result.is_none());
}

#[test]
fn test_build_session_summary_context_store_error() {
    use crate::store::mock::MockStore;
    use cas_store::StoreError;

    // Create a store and inject an error
    let store = MockStore::new();
    store.inject_error(StoreError::Other("Test store error".to_string()));

    let mut config = Config::default();
    let mut hook_config = crate::config::HookConfig::default();
    hook_config.stop.generate_summary = true;
    config.hooks = Some(hook_config);

    // Should return Some with error context (not None - fail explicitly, don't skip)
    let result = build_session_summary_context(&store, &config, "test-session");
    assert!(
        result.is_some(),
        "Store error should return error context, not None"
    );
    let context = result.unwrap();
    assert!(
        context.contains("session-summary-error"),
        "Error context should indicate error: {context}"
    );
    assert!(
        context.contains("Failed to check"),
        "Error context should explain the failure: {context}"
    );
}

// =========================================================================
// build_learning_review_context tests
// =========================================================================

#[test]
fn test_build_learning_review_context_disabled() {
    use crate::store::mock::MockStore;

    let store = MockStore::new();
    let config = Config::default();

    // Default config has learning_review_enabled = false
    let result = build_learning_review_context(&store, &config);
    assert!(result.is_none());
}

#[test]
fn test_build_learning_review_context_below_threshold() {
    use crate::store::mock::MockStore;
    use crate::types::EntryType;

    // Create 3 unreviewed learnings (below threshold of 5)
    let entries: Vec<Entry> = (0..3)
        .map(|i| {
            let mut entry = Entry::new(format!("entry-{i}"), format!("Learning {i}"));
            entry.entry_type = EntryType::Learning;
            entry.last_reviewed = None; // Unreviewed
            entry
        })
        .collect();
    let store = MockStore::with_entries(entries);

    let mut config = Config::default();
    let mut hook_config = crate::config::HookConfig::default();
    hook_config.stop.learning_review_enabled = true;
    hook_config.stop.learning_review_threshold = 5;
    config.hooks = Some(hook_config);

    // Should return None since count (3) < threshold (5)
    let result = build_learning_review_context(&store, &config);
    assert!(result.is_none());
}

#[test]
fn test_build_learning_review_context_at_threshold() {
    use crate::store::mock::MockStore;
    use crate::types::EntryType;

    // Create 5 unreviewed learnings (at threshold of 5)
    let entries: Vec<Entry> = (0..5)
        .map(|i| {
            let mut entry = Entry::new(format!("entry-{i}"), format!("Learning {i}"));
            entry.entry_type = EntryType::Learning;
            entry.last_reviewed = None; // Unreviewed
            entry
        })
        .collect();
    let store = MockStore::with_entries(entries);

    let mut config = Config::default();
    let mut hook_config = crate::config::HookConfig::default();
    hook_config.stop.learning_review_enabled = true;
    hook_config.stop.learning_review_threshold = 5;
    config.hooks = Some(hook_config);

    // Should return Some since count (5) >= threshold (5)
    let result = build_learning_review_context(&store, &config);
    assert!(result.is_some());
    let context = result.unwrap();
    assert!(context.contains("learning-review required=\"true\""));
    assert!(context.contains("learning-reviewer"));
    for i in 0..5 {
        assert!(
            context.contains(&format!("entry-{i}")),
            "learning-reviewer prompt must include every unreviewed learning ID: {context}"
        );
    }
}

#[test]
fn test_build_learning_review_context_above_threshold() {
    use crate::store::mock::MockStore;
    use crate::types::EntryType;

    // Create 10 unreviewed learnings (above threshold of 5)
    let entries: Vec<Entry> = (0..10)
        .map(|i| {
            let mut entry = Entry::new(format!("entry-{i}"), format!("Learning {i}"));
            entry.entry_type = EntryType::Learning;
            entry.last_reviewed = None; // Unreviewed
            entry
        })
        .collect();
    let store = MockStore::with_entries(entries);

    let mut config = Config::default();
    let mut hook_config = crate::config::HookConfig::default();
    hook_config.stop.learning_review_enabled = true;
    hook_config.stop.learning_review_threshold = 5;
    config.hooks = Some(hook_config);

    // Should return Some since count (10) >= threshold (5)
    let result = build_learning_review_context(&store, &config);
    assert!(result.is_some());
    let context = result.unwrap();
    assert!(context.contains("learning-review required=\"true\""));
    assert!(context.contains("10 entries"));
}

#[test]
fn test_build_learning_review_context_threshold_zero() {
    use crate::store::mock::MockStore;

    let store = MockStore::new();
    let mut config = Config::default();
    let mut hook_config = crate::config::HookConfig::default();
    hook_config.stop.learning_review_enabled = true;
    hook_config.stop.learning_review_threshold = 0; // Invalid threshold
    config.hooks = Some(hook_config);

    // Should return error context since threshold = 0 is invalid
    let result = build_learning_review_context(&store, &config);
    assert!(
        result.is_some(),
        "threshold=0 should return error context, not None"
    );
    let context = result.unwrap();
    assert!(
        context.contains("learning-review-error"),
        "Error context should indicate error: {context}"
    );
    assert!(
        context.contains("threshold"),
        "Error context should mention threshold: {context}"
    );
}
