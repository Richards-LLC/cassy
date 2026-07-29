use crate::hooks::handlers::*;

#[test]
fn test_format_file_change() {
    let input = HookInput {
        session_id: "test".to_string(),
        cwd: "/test".to_string(),
        workspace_root: None,
        hook_event_name: "PostToolUse".to_string(),
        tool_name: Some("Write".to_string()),
        tool_input: Some(serde_json::json!({"file_path": "/test/main.rs"})),
        tool_response: None,
        transcript_path: None,
        permission_mode: None,
        tool_use_id: None,
        tool_input_truncated: None,
        user_prompt: None,
        source: None,
        reason: None,
        subagent_type: None,
        subagent_prompt: None,
        agent_role: None,
        message: None,
    };

    let observation = format_observation(&input, None);
    assert_eq!(observation, "Write: /test/main.rs");
}

#[test]
fn test_format_bash_skips_simple() {
    let input = HookInput {
        session_id: "test".to_string(),
        cwd: "/test".to_string(),
        workspace_root: None,
        hook_event_name: "PostToolUse".to_string(),
        tool_name: Some("Bash".to_string()),
        tool_input: Some(serde_json::json!({"command": "ls -la"})),
        tool_response: None,
        transcript_path: None,
        permission_mode: None,
        tool_use_id: None,
        tool_input_truncated: None,
        user_prompt: None,
        source: None,
        reason: None,
        subagent_type: None,
        subagent_prompt: None,
        agent_role: None,
        message: None,
    };

    let observation = format_observation(&input, None);
    assert!(observation.is_empty());
}

#[test]
fn test_format_bash_captures_cargo() {
    let input = HookInput {
        session_id: "test".to_string(),
        cwd: "/test".to_string(),
        workspace_root: None,
        hook_event_name: "PostToolUse".to_string(),
        tool_name: Some("Bash".to_string()),
        tool_input: Some(serde_json::json!({"command": "cargo test"})),
        tool_response: None,
        transcript_path: None,
        permission_mode: None,
        tool_use_id: None,
        tool_input_truncated: None,
        user_prompt: None,
        source: None,
        reason: None,
        subagent_type: None,
        subagent_prompt: None,
        agent_role: None,
        message: None,
    };

    let observation = format_observation(&input, None);
    assert_eq!(observation, "Bash: cargo test");
}

#[test]
fn test_extracted_learning_deserialize() {
    let json = r#"{
        "content": "Use generated types from @/types/ instead of manual interfaces",
        "path_pattern": "**/*.tsx",
        "confidence": 0.9,
        "tags": ["react", "typescript"]
    }"#;

    let learning: ExtractedLearning = serde_json::from_str(json).unwrap();
    assert_eq!(
        learning.content,
        "Use generated types from @/types/ instead of manual interfaces"
    );
    assert_eq!(learning.path_pattern, Some("**/*.tsx".to_string()));
    assert_eq!(learning.confidence, 0.9);
    assert_eq!(learning.tags, vec!["react", "typescript"]);
}

#[test]
fn test_extracted_learning_null_path() {
    let json = r#"{
        "content": "Always run mix nb_ts.gen after changing serializers",
        "path_pattern": null,
        "confidence": 0.85,
        "tags": ["elixir", "phoenix"]
    }"#;

    let learning: ExtractedLearning = serde_json::from_str(json).unwrap();
    assert_eq!(learning.path_pattern, None);
}

#[test]
fn test_extracted_learning_array_parse() {
    let json = r#"[
        {"content": "Rule 1", "path_pattern": null, "confidence": 0.8, "tags": []},
        {"content": "Rule 2", "path_pattern": "**/*.rs", "confidence": 0.9, "tags": ["rust"]}
    ]"#;

    let learnings: Vec<ExtractedLearning> = serde_json::from_str(json).unwrap();
    assert_eq!(learnings.len(), 2);
    assert_eq!(learnings[0].content, "Rule 1");
    assert_eq!(learnings[1].path_pattern, Some("**/*.rs".to_string()));
}

#[test]
fn test_truncate_str() {
    assert_eq!(truncate_str("short", 10), "short");
    assert_eq!(truncate_str("this is a long string", 10), "this is...");
    assert_eq!(truncate_str("exactly10c", 10), "exactly10c");
}

#[test]
fn test_truncate_list() {
    let items = vec!["a", "b", "c"];
    assert_eq!(truncate_list(&items, 5), "a, b, c");
    assert_eq!(truncate_list(&items, 2), "a, b, ... (+1 more)");
}

#[test]
fn test_is_architectural_file() {
    assert!(is_architectural_file("Cargo.toml"));
    assert!(is_architectural_file("/path/to/package.json"));
    assert!(is_architectural_file("/project/src/main.rs")); // needs /src/main. pattern
    assert!(is_architectural_file("lib/my_app/src/store/user.ex")); // contains /src/store/
    assert!(!is_architectural_file("src/components/Button.tsx"));
    assert!(!is_architectural_file("README.md"));
}

// =========================================================================
// should_buffer_observation tests
// =========================================================================

fn make_hook_input(
    tool: &str,
    tool_input: serde_json::Value,
    tool_response: Option<serde_json::Value>,
) -> HookInput {
    HookInput {
        session_id: "test-session".to_string(),
        cwd: "/test".to_string(),
        workspace_root: None,
        hook_event_name: "PostToolUse".to_string(),
        tool_name: Some(tool.to_string()),
        tool_input: Some(tool_input),
        tool_response,
        transcript_path: None,
        permission_mode: None,
        tool_use_id: None,
        tool_input_truncated: None,
        user_prompt: None,
        source: None,
        reason: None,
        subagent_type: None,
        subagent_prompt: None,
        agent_role: None,
        message: None,
    }
}

#[test]
fn test_should_buffer_bash_error() {
    let input = make_hook_input(
        "Bash",
        serde_json::json!({"command": "cargo build"}),
        Some(serde_json::json!({"exitCode": 1})),
    );
    assert!(should_buffer_observation(&input, "Bash"));
}

#[test]
fn test_should_buffer_bash_success_build() {
    let input = make_hook_input(
        "Bash",
        serde_json::json!({"command": "cargo build"}),
        Some(serde_json::json!({"exitCode": 0})),
    );
    assert!(should_buffer_observation(&input, "Bash"));
}

#[test]
fn test_should_buffer_bash_success_test() {
    let input = make_hook_input(
        "Bash",
        serde_json::json!({"command": "cargo test"}),
        Some(serde_json::json!({"exitCode": 0})),
    );
    assert!(should_buffer_observation(&input, "Bash"));
}

#[test]
fn test_should_not_buffer_bash_simple_cmd() {
    let input = make_hook_input(
        "Bash",
        serde_json::json!({"command": "echo hello"}),
        Some(serde_json::json!({"exitCode": 0})),
    );
    assert!(!should_buffer_observation(&input, "Bash"));
}

#[test]
fn test_should_buffer_write_always() {
    let input = make_hook_input(
        "Write",
        serde_json::json!({"file_path": "/test/file.rs", "content": "fn main() {}"}),
        None,
    );
    assert!(should_buffer_observation(&input, "Write"));
}

#[test]
fn test_should_buffer_edit_large() {
    // Edit with 15 lines difference (>= 10 threshold)
    let old_string = "line1\nline2\nline3\nline4\nline5";
    let new_string = "new1\nnew2\nnew3\nnew4\nnew5\nnew6\nnew7\nnew8\nnew9\nnew10\nnew11\nnew12\nnew13\nnew14\nnew15\nnew16\nnew17\nnew18\nnew19\nnew20";
    let input = make_hook_input(
        "Edit",
        serde_json::json!({"file_path": "/test/file.rs", "old_string": old_string, "new_string": new_string}),
        None,
    );
    assert!(should_buffer_observation(&input, "Edit"));
}

#[test]
fn test_should_not_buffer_edit_small() {
    // Small edit (2 lines difference)
    let input = make_hook_input(
        "Edit",
        serde_json::json!({"file_path": "/test/file.rs", "old_string": "a\nb", "new_string": "c\nd\ne\nf"}),
        None,
    );
    assert!(!should_buffer_observation(&input, "Edit"));
}

#[test]
fn test_should_not_buffer_read() {
    let input = make_hook_input(
        "Read",
        serde_json::json!({"file_path": "/test/file.rs"}),
        None,
    );
    assert!(!should_buffer_observation(&input, "Read"));
}

#[test]
fn test_should_buffer_git_commit() {
    let input = make_hook_input(
        "Bash",
        serde_json::json!({"command": "git commit -m 'fix: bug'"}),
        Some(serde_json::json!({"exitCode": 0})),
    );
    assert!(should_buffer_observation(&input, "Bash"));
}

#[test]
fn test_should_buffer_npm_test() {
    let input = make_hook_input(
        "Bash",
        serde_json::json!({"command": "npm test"}),
        Some(serde_json::json!({"exitCode": 0})),
    );
    assert!(should_buffer_observation(&input, "Bash"));
}

#[test]
fn test_should_buffer_mix_test() {
    let input = make_hook_input(
        "Bash",
        serde_json::json!({"command": "mix test"}),
        Some(serde_json::json!({"exitCode": 0})),
    );
    assert!(should_buffer_observation(&input, "Bash"));
}

// =========================================================================
// is_important_prompt tests
// =========================================================================

#[test]
fn test_is_important_prompt_task_indicators() {
    assert!(is_important_prompt("implement a new feature"));
    assert!(is_important_prompt("create a user login page"));
    assert!(is_important_prompt("add authentication to the API"));
    assert!(is_important_prompt("fix the bug in the parser"));
    assert!(is_important_prompt("update the database schema"));
    assert!(is_important_prompt("refactor the legacy code"));
    assert!(is_important_prompt("build the Docker image"));
    assert!(is_important_prompt("write unit tests for this"));
    assert!(is_important_prompt("help me understand the code"));
    assert!(is_important_prompt("can you explain this function?"));
}

#[test]
fn test_is_important_prompt_questions() {
    assert!(is_important_prompt("how does this work?"));
    assert!(is_important_prompt("what is the purpose of this file?"));
    assert!(is_important_prompt("why is this test failing?"));
    assert!(is_important_prompt("where is the config located?"));
    assert!(is_important_prompt("should I use async here?"));
    assert!(is_important_prompt("is there a better approach?"));
}

#[test]
fn test_is_important_prompt_long() {
    // Long prompts (>100 chars) are considered important
    let long_prompt = "a".repeat(101);
    assert!(is_important_prompt(&long_prompt));
}

#[test]
fn test_is_not_important_prompt_short() {
    assert!(!is_important_prompt("yes"));
    assert!(!is_important_prompt("ok"));
    assert!(!is_important_prompt("sure"));
    assert!(!is_important_prompt("thanks"));
    assert!(!is_important_prompt("no"));
}

// =========================================================================
// format_bash_command tests
// =========================================================================

#[test]
fn test_format_bash_skips_navigation() {
    let skip_commands = vec![
        "cd /tmp",
        "pwd",
        "ls",
        "cat file.txt",
        "head -10 file",
        "tail -f log",
    ];
    for cmd in skip_commands {
        let input = make_hook_input("Bash", serde_json::json!({"command": cmd}), None);
        let obs = format_observation(&input, None);
        assert!(obs.is_empty(), "Should skip command: {cmd}");
    }
}

#[test]
fn test_format_bash_skips_cas_commands() {
    let input = make_hook_input(
        "Bash",
        serde_json::json!({"command": "cas search test"}),
        None,
    );
    let obs = format_observation(&input, None);
    assert!(obs.is_empty());
}

#[test]
fn test_format_bash_skips_readonly_git() {
    let readonly_git = vec![
        "git status",
        "git log",
        "git diff",
        "git show",
        "git branch",
    ];
    for cmd in readonly_git {
        let input = make_hook_input("Bash", serde_json::json!({"command": cmd}), None);
        let obs = format_observation(&input, None);
        assert!(obs.is_empty(), "Should skip read-only git: {cmd}");
    }
}

#[test]
fn test_format_bash_captures_git_commit() {
    let input = make_hook_input(
        "Bash",
        serde_json::json!({"command": "git commit -m 'test'"}),
        None,
    );
    let obs = format_observation(&input, None);
    assert_eq!(obs, "Bash: git commit -m 'test'");
}

#[test]
fn test_format_bash_truncates_long() {
    let long_cmd = "a".repeat(250);
    let input = make_hook_input("Bash", serde_json::json!({"command": long_cmd}), None);
    let obs = format_observation(&input, None);
    assert!(obs.len() < 220); // "Bash: " + 200 chars + "..."
    assert!(obs.ends_with("..."));
}

// =========================================================================
// format_file_read tests
// =========================================================================

#[test]
fn test_format_file_read_skips_common() {
    let skip_extensions = vec![
        "README.md",
        "config.json",
        "settings.yaml",
        "data.yml",
        "Cargo.toml",
    ];
    for file in skip_extensions {
        let input = make_hook_input("Read", serde_json::json!({"file_path": file}), None);
        let obs = format_file_read(&input, None);
        assert!(obs.is_empty(), "Should skip: {file}");
    }
}

#[test]
fn test_format_file_read_captures_code() {
    let input = make_hook_input(
        "Read",
        serde_json::json!({"file_path": "/src/main.rs"}),
        None,
    );
    let obs = format_file_read(&input, None);
    assert_eq!(obs, "Read: /src/main.rs");
}

// =========================================================================
// estimate_tokens tests
// =========================================================================

#[test]
fn test_estimate_tokens() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("test"), 1); // 4 chars / 4 = 1
    assert_eq!(estimate_tokens("12345678"), 2); // 8 chars / 4 = 2
    assert_eq!(estimate_tokens("a".repeat(100).as_str()), 25); // 100 chars / 4 = 25
}

// =========================================================================
// SessionSummary tests
// =========================================================================

#[test]
fn test_session_summary_default() {
    let summary = SessionSummary::default();
    assert!(summary.summary.is_empty());
    assert!(summary.decisions.is_empty());
    assert!(summary.tasks_completed.is_empty());
    assert!(summary.key_learnings.is_empty());
    assert!(summary.follow_up_tasks.is_empty());
}

#[test]
fn test_session_summary_serialize() {
    let summary = SessionSummary {
        summary: "Completed task".to_string(),
        decisions: vec!["Use async".to_string()],
        tasks_completed: vec!["Fix bug".to_string()],
        key_learnings: vec!["Pattern X works".to_string()],
        follow_up_tasks: vec!["Add tests".to_string()],
    };
    let json = serde_json::to_string(&summary).unwrap();
    assert!(json.contains("Completed task"));
    assert!(json.contains("Use async"));
}

#[test]
fn test_session_summary_deserialize() {
    let json = r#"{
        "summary": "Test summary",
        "decisions": ["Decision 1"],
        "tasks_completed": [],
        "key_learnings": ["Learning 1", "Learning 2"],
        "follow_up_tasks": []
    }"#;
    let summary: SessionSummary = serde_json::from_str(json).unwrap();
    assert_eq!(summary.summary, "Test summary");
    assert_eq!(summary.decisions.len(), 1);
    assert_eq!(summary.key_learnings.len(), 2);
}

// =========================================================================
// is_architectural_file additional tests
// =========================================================================

#[test]
fn test_is_architectural_file_config_files() {
    // Test all config patterns
    assert!(is_architectural_file("pyproject.toml"));
    assert!(is_architectural_file("go.mod"));
    assert!(is_architectural_file("tsconfig.json"));
    assert!(is_architectural_file("webpack.config.js"));
    assert!(is_architectural_file("vite.config.ts"));
    assert!(is_architectural_file(".eslintrc"));
    assert!(is_architectural_file(".prettierrc"));
    assert!(is_architectural_file("CMakeLists.txt"));
}

#[test]
fn test_is_architectural_file_patterns() {
    // Test architectural patterns - must contain /src/ prefix for most patterns
    assert!(is_architectural_file("/src/lib.rs"));
    assert!(is_architectural_file("/project/src/mod.rs"));
    assert!(is_architectural_file("app/src/types/user.ts"));
    assert!(is_architectural_file("src/schema.prisma"));
    assert!(is_architectural_file("db/migrations/001_create_users.sql"));
    assert!(is_architectural_file("/src/api/routes.ts"));
    assert!(is_architectural_file("/src/models/user.rs"));
    assert!(is_architectural_file("/src/handlers/auth.go"));
}

#[test]
fn test_is_not_architectural_file() {
    assert!(!is_architectural_file("test.rs"));
    assert!(!is_architectural_file("utils.js"));
    assert!(!is_architectural_file("helper.py"));
    assert!(!is_architectural_file("style.css"));
}

// =========================================================================
// truncate_str edge cases
// =========================================================================

#[test]
fn test_truncate_str_edge_cases() {
    assert_eq!(truncate_str("", 10), "");
    assert_eq!(truncate_str("a", 10), "a");
    assert_eq!(truncate_str("ab", 10), "ab");
    assert_eq!(truncate_str("abc", 3), "abc");
    assert_eq!(truncate_str("abcd", 3), "...");
    assert_eq!(truncate_str("abcde", 4), "a...");
}

#[test]
fn test_truncate_str_handles_unicode_boundary() {
    let input = format!("{}✅ done", "a".repeat(99));
    assert_eq!(truncate_str(&input, 103), format!("{}...", "a".repeat(99)));
}

// =========================================================================
// truncate_list edge cases
// =========================================================================

#[test]
fn test_truncate_list_edge_cases() {
    let empty: Vec<&str> = vec![];
    assert_eq!(truncate_list(&empty, 5), "");

    let single = vec!["one"];
    assert_eq!(truncate_list(&single, 5), "one");
    assert_eq!(truncate_list(&single, 1), "one");

    let many = vec!["a", "b", "c", "d", "e"];
    assert_eq!(truncate_list(&many, 3), "a, b, c, ... (+2 more)");
    assert_eq!(truncate_list(&many, 5), "a, b, c, d, e");
    assert_eq!(truncate_list(&many, 10), "a, b, c, d, e");
}

// =========================================================================
// format_observation edge cases
// =========================================================================

#[test]
fn test_format_observation_unknown_tool() {
    let input = make_hook_input("Unknown", serde_json::json!({}), None);
    let obs = format_observation(&input, None);
    assert!(obs.is_empty());
}

#[test]
fn test_format_observation_no_tool_input() {
    let input = HookInput {
        session_id: "test".to_string(),
        cwd: "/test".to_string(),
        workspace_root: None,
        hook_event_name: "PostToolUse".to_string(),
        tool_name: Some("Write".to_string()),
        tool_input: None,
        tool_response: None,
        transcript_path: None,
        permission_mode: None,
        tool_use_id: None,
        tool_input_truncated: None,
        user_prompt: None,
        source: None,
        reason: None,
        subagent_type: None,
        subagent_prompt: None,
        agent_role: None,
        message: None,
    };
    let obs = format_observation(&input, None);
    assert!(obs.is_empty());
}

#[test]
fn test_format_observation_edit() {
    let input = make_hook_input(
        "Edit",
        serde_json::json!({"file_path": "/src/lib.rs", "old_string": "old", "new_string": "new"}),
        None,
    );
    let obs = format_observation(&input, None);
    assert_eq!(obs, "Edit: /src/lib.rs");
}

// =========================================================================
// User preference detection tests
// =========================================================================

#[test]
fn test_is_preference_prompt_never_pattern() {
    assert!(is_preference_prompt("never add TODOs to the code"));
    assert!(is_preference_prompt(
        "Never add TODO comments, implement fully"
    ));
    assert!(is_preference_prompt("don't ever add placeholder code"));
}

#[test]
fn test_is_preference_prompt_always_pattern() {
    assert!(is_preference_prompt("always implement full functionality"));
    assert!(is_preference_prompt("Always use TypeScript strict mode"));
    assert!(is_preference_prompt("always run tests before committing"));
}

#[test]
fn test_is_preference_prompt_dont_pattern() {
    assert!(is_preference_prompt(
        "don't add TODOs, implement the actual code"
    ));
    assert!(is_preference_prompt("Do not use console.log for debugging"));
    assert!(is_preference_prompt("dont leave placeholder comments"));
}

#[test]
fn test_is_preference_prompt_prefer_pattern() {
    assert!(is_preference_prompt(
        "I prefer functional components over class components"
    ));
    assert!(is_preference_prompt("prefer async/await over .then()"));
}

#[test]
fn test_is_preference_prompt_instead_pattern() {
    assert!(is_preference_prompt("use Result instead of panic"));
    assert!(is_preference_prompt("use match instead of if-let chains"));
}

#[test]
fn test_is_preference_prompt_negative_cases() {
    // Regular task requests should not be detected as preferences
    assert!(!is_preference_prompt("fix the bug in login"));
    assert!(!is_preference_prompt("add a new button to the header"));
    assert!(!is_preference_prompt("what does this function do?"));
    assert!(!is_preference_prompt("run the tests"));
    assert!(!is_preference_prompt("yes")); // Too short
    assert!(!is_preference_prompt("ok go ahead")); // Acknowledgment
}
