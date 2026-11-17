use crate::tracing::*;
use chrono::Utc;
use tempfile::TempDir;

#[test]
fn test_trace_store_basic() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    let event = TraceEvent {
        id: "tr-001".to_string(),
        event_type: TraceEventType::Search,
        timestamp: Utc::now(),
        session_id: Some("sess-001".to_string()),
        duration_ms: 150,
        input: r#"{"query":"test"}"#.to_string(),
        output: r#"{"results":5}"#.to_string(),
        metadata: r#"{}"#.to_string(),
        success: true,
        error: None,
    };

    store.record(&event).unwrap();

    let recent = store.get_recent(10).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].id, "tr-001");
}

#[test]
fn test_trace_builder() {
    let trace = TraceBuilder::new(TraceEventType::ContextInjection)
        .session("sess-123")
        .input(&serde_json::json!({"cwd": "/test"}))
        .finish_success(&serde_json::json!({"tokens": 500}));

    assert_eq!(trace.event_type, TraceEventType::ContextInjection);
    assert!(trace.success);
    assert!(trace.session_id.is_some());
}

#[test]
fn test_trace_stats() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    // Record multiple events
    for i in 0..5 {
        store
            .record(&TraceEvent {
                id: format!("tr-{i}"),
                event_type: TraceEventType::Search,
                timestamp: Utc::now(),
                session_id: None,
                duration_ms: 100,
                input: "{}".to_string(),
                output: "{}".to_string(),
                metadata: "{}".to_string(),
                success: i % 2 == 0,
                error: None,
            })
            .unwrap();
    }

    let stats = store.get_stats().unwrap();
    assert!(!stats.is_empty());

    let search_stats = stats
        .iter()
        .find(|s| s.event_type == TraceEventType::Search)
        .unwrap();
    assert_eq!(search_stats.count, 5);
    assert_eq!(search_stats.success_count, 3);
    assert_eq!(search_stats.failure_count, 2);
}

// =========================================================================
// TraceEventType tests
// =========================================================================

#[test]
fn test_trace_event_type_display() {
    assert_eq!(
        TraceEventType::ContextInjection.to_string(),
        "context_injection"
    );
    assert_eq!(TraceEventType::Search.to_string(), "search");
    assert_eq!(
        TraceEventType::RuleApplication.to_string(),
        "rule_application"
    );
    assert_eq!(TraceEventType::Extraction.to_string(), "extraction");
    assert_eq!(
        TraceEventType::MemoryRetrieval.to_string(),
        "memory_retrieval"
    );
    assert_eq!(
        TraceEventType::SkillInvocation.to_string(),
        "skill_invocation"
    );
    assert_eq!(TraceEventType::CommandExecution.to_string(), "command");
    assert_eq!(TraceEventType::ClaudeApiCall.to_string(), "claude_api");
    assert_eq!(TraceEventType::StoreOperation.to_string(), "store_op");
    assert_eq!(TraceEventType::HookEvent.to_string(), "hook");
}

#[test]
fn test_trace_event_type_from_str() {
    assert_eq!(
        TraceEventType::parse("context_injection"),
        Some(TraceEventType::ContextInjection)
    );
    assert_eq!(
        TraceEventType::parse("search"),
        Some(TraceEventType::Search)
    );
    assert_eq!(
        TraceEventType::parse("rule_application"),
        Some(TraceEventType::RuleApplication)
    );
    assert_eq!(
        TraceEventType::parse("command"),
        Some(TraceEventType::CommandExecution)
    );
    assert_eq!(
        TraceEventType::parse("command_execution"),
        Some(TraceEventType::CommandExecution)
    );
    assert_eq!(
        TraceEventType::parse("claude_api"),
        Some(TraceEventType::ClaudeApiCall)
    );
    assert_eq!(
        TraceEventType::parse("claude_api_call"),
        Some(TraceEventType::ClaudeApiCall)
    );
    assert_eq!(
        TraceEventType::parse("store_op"),
        Some(TraceEventType::StoreOperation)
    );
    assert_eq!(
        TraceEventType::parse("store_operation"),
        Some(TraceEventType::StoreOperation)
    );
    assert_eq!(
        TraceEventType::parse("hook"),
        Some(TraceEventType::HookEvent)
    );
    assert_eq!(
        TraceEventType::parse("hook_event"),
        Some(TraceEventType::HookEvent)
    );
    assert_eq!(TraceEventType::parse("invalid"), None);
}

// =========================================================================
// ToolTrace tests
// =========================================================================

#[test]
fn test_tool_trace_new() {
    let trace = ToolTrace::new("sess-123".to_string(), "Edit".to_string(), 5);
    assert!(trace.id.starts_with("tt-"));
    assert_eq!(trace.session_id, "sess-123");
    assert_eq!(trace.tool_name, "Edit");
    assert_eq!(trace.sequence_pos, 5);
    assert!(trace.success);
    assert!(!trace.prev_failed);
}

#[test]
fn test_tool_trace_hash_content() {
    let hash1 = ToolTrace::hash_content("hello world");
    let hash2 = ToolTrace::hash_content("hello world");
    let hash3 = ToolTrace::hash_content("different content");

    assert_eq!(hash1, hash2);
    assert_ne!(hash1, hash3);
    assert_eq!(hash1.len(), 16); // 64 bits as hex = 16 chars
}

#[test]
fn test_tool_trace_classify_error() {
    // Type errors
    assert_eq!(
        ToolTrace::classify_error("type mismatch: expected i32"),
        "type_error"
    );
    assert_eq!(
        ToolTrace::classify_error("cannot find type Foo"),
        "type_error"
    );

    // Import errors
    assert_eq!(
        ToolTrace::classify_error("unresolved import std::foo"),
        "import_error"
    );
    assert_eq!(
        ToolTrace::classify_error("no module named requests"),
        "import_error"
    );
    assert_eq!(
        ToolTrace::classify_error("cannot find Bar in this scope"),
        "import_error"
    );

    // Syntax errors
    assert_eq!(
        ToolTrace::classify_error("syntax error near line 10"),
        "syntax_error"
    );
    assert_eq!(
        ToolTrace::classify_error("unexpected token ';'"),
        "syntax_error"
    );

    // Undefined errors
    assert_eq!(
        ToolTrace::classify_error("undefined variable x"),
        "undefined_error"
    );
    assert_eq!(
        ToolTrace::classify_error("method not found in struct"),
        "undefined_error"
    );

    // Argument errors
    assert_eq!(
        ToolTrace::classify_error("wrong number of arguments"),
        "argument_error"
    );
    assert_eq!(
        ToolTrace::classify_error("invalid parameter count"),
        "argument_error"
    );

    // Borrow errors (Rust)
    assert_eq!(
        ToolTrace::classify_error("cannot borrow as mutable"),
        "borrow_error"
    );
    assert_eq!(
        ToolTrace::classify_error("value moved here"),
        "borrow_error"
    );

    // Runtime errors
    assert_eq!(
        ToolTrace::classify_error("thread panicked at"),
        "runtime_error"
    );
    assert_eq!(
        ToolTrace::classify_error("nil pointer dereference"),
        "runtime_error"
    );

    // Permission errors
    assert_eq!(
        ToolTrace::classify_error("permission denied"),
        "permission_error"
    );

    // Network errors
    assert_eq!(
        ToolTrace::classify_error("connection refused"),
        "network_error"
    );
    assert_eq!(
        ToolTrace::classify_error("network timeout"),
        "network_error"
    );

    // Other
    assert_eq!(ToolTrace::classify_error("some random error"), "other");
}

#[test]
fn test_tool_trace_classify_command() {
    // Build commands
    assert_eq!(ToolTrace::classify_command("cargo build"), "build");
    assert_eq!(ToolTrace::classify_command("cargo check"), "build");
    assert_eq!(ToolTrace::classify_command("npm run build"), "build");
    assert_eq!(ToolTrace::classify_command("make all"), "build");
    assert_eq!(ToolTrace::classify_command("go build ./..."), "build");

    // Test commands
    assert_eq!(ToolTrace::classify_command("cargo test"), "test");
    assert_eq!(ToolTrace::classify_command("npm test"), "test");
    assert_eq!(ToolTrace::classify_command("pytest"), "test");
    assert_eq!(ToolTrace::classify_command("jest"), "test");
    assert_eq!(ToolTrace::classify_command("go test ./..."), "test");
    assert_eq!(ToolTrace::classify_command("mix test"), "test");

    // Run commands
    assert_eq!(ToolTrace::classify_command("cargo run"), "run");
    assert_eq!(ToolTrace::classify_command("npm start"), "run");
    assert_eq!(ToolTrace::classify_command("node server.js"), "run");
    assert_eq!(ToolTrace::classify_command("python main.py"), "run");
    assert_eq!(ToolTrace::classify_command("mix phx.server"), "run");

    // Git commands
    assert_eq!(ToolTrace::classify_command("git commit -m 'test'"), "git");
    assert_eq!(ToolTrace::classify_command("git push origin main"), "git");

    // Other
    assert_eq!(ToolTrace::classify_command("ls -la"), "other");
    assert_eq!(ToolTrace::classify_command("echo hello"), "other");
}

#[test]
fn test_tool_trace_is_dep_path() {
    assert!(ToolTrace::is_dep_path("/project/deps/lib/file.rs"));
    assert!(ToolTrace::is_dep_path("/app/node_modules/express/index.js"));
    assert!(ToolTrace::is_dep_path("/vendor/bundle/gems/rails/lib.rb"));
    assert!(ToolTrace::is_dep_path(
        "/home/user/.cargo/registry/src/lib.rs"
    ));
    assert!(ToolTrace::is_dep_path(
        "/target/debug/deps/mylib-abc123.rlib"
    ));
    assert!(ToolTrace::is_dep_path(
        "/venv/lib/python3.9/site-packages/requests/api.py"
    ));

    assert!(!ToolTrace::is_dep_path("/project/src/main.rs"));
    assert!(!ToolTrace::is_dep_path("/app/lib/my_code.js"));
}

// =========================================================================
// TraceBuilder tests
// =========================================================================

#[test]
fn test_trace_builder_finish_error() {
    let trace = TraceBuilder::new(TraceEventType::Extraction)
        .session("sess-456")
        .input(&serde_json::json!({"obs_id": "obs-1"}))
        .finish_error("extraction failed");

    assert_eq!(trace.event_type, TraceEventType::Extraction);
    assert!(!trace.success);
    assert_eq!(trace.error, Some("extraction failed".to_string()));
}

#[test]
fn test_trace_builder_with_metadata() {
    let trace = TraceBuilder::new(TraceEventType::Search)
        .session("sess-789")
        .input(&serde_json::json!({"query": "test"}))
        .metadata(&serde_json::json!({"source": "hybrid"}))
        .output(&serde_json::json!({"count": 10}))
        .duration_ms(250)
        .finish_success(&serde_json::json!({"results": 5}));

    assert_eq!(trace.duration_ms, 250);
    assert!(trace.success);
    assert!(trace.metadata.contains("hybrid"));
}

#[test]
fn test_trace_builder_finish_custom() {
    let trace = TraceBuilder::new(TraceEventType::HookEvent).finish(false, Some("timeout"));

    assert!(!trace.success);
    assert_eq!(trace.error, Some("timeout".to_string()));
}

// =========================================================================
// TraceStore additional tests
// =========================================================================

#[test]
fn test_trace_store_get() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    let event = TraceEvent {
        id: "tr-get-001".to_string(),
        event_type: TraceEventType::Search,
        timestamp: Utc::now(),
        session_id: Some("sess-001".to_string()),
        duration_ms: 100,
        input: "{}".to_string(),
        output: "{}".to_string(),
        metadata: "{}".to_string(),
        success: true,
        error: None,
    };
    store.record(&event).unwrap();

    let found = store.get("tr-get-001").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, "tr-get-001");

    let not_found = store.get("nonexistent").unwrap();
    assert!(not_found.is_none());
}

#[test]
fn test_trace_store_get_by_session() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    for i in 0..3 {
        store
            .record(&TraceEvent {
                id: format!("tr-sess-{i}"),
                event_type: TraceEventType::Search,
                timestamp: Utc::now(),
                session_id: Some("session-A".to_string()),
                duration_ms: 100,
                input: "{}".to_string(),
                output: "{}".to_string(),
                metadata: "{}".to_string(),
                success: true,
                error: None,
            })
            .unwrap();
    }

    let results = store.get_by_session("session-A").unwrap();
    assert_eq!(results.len(), 3);

    let results_b = store.get_by_session("session-B").unwrap();
    assert!(results_b.is_empty());
}

#[test]
fn test_trace_store_get_by_type() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    store
        .record(&TraceEvent {
            id: "tr-type-1".to_string(),
            event_type: TraceEventType::Search,
            timestamp: Utc::now(),
            session_id: None,
            duration_ms: 100,
            input: "{}".to_string(),
            output: "{}".to_string(),
            metadata: "{}".to_string(),
            success: true,
            error: None,
        })
        .unwrap();

    store
        .record(&TraceEvent {
            id: "tr-type-2".to_string(),
            event_type: TraceEventType::Extraction,
            timestamp: Utc::now(),
            session_id: None,
            duration_ms: 100,
            input: "{}".to_string(),
            output: "{}".to_string(),
            metadata: "{}".to_string(),
            success: true,
            error: None,
        })
        .unwrap();

    let search_results = store.get_by_type(TraceEventType::Search, 10).unwrap();
    assert_eq!(search_results.len(), 1);

    let extraction_results = store.get_by_type(TraceEventType::Extraction, 10).unwrap();
    assert_eq!(extraction_results.len(), 1);
}

#[test]
fn test_trace_store_search() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    store
        .record(&TraceEvent {
            id: "tr-search-1".to_string(),
            event_type: TraceEventType::Search,
            timestamp: Utc::now(),
            session_id: None,
            duration_ms: 100,
            input: r#"{"query": "unique_pattern_xyz"}"#.to_string(),
            output: "{}".to_string(),
            metadata: "{}".to_string(),
            success: true,
            error: None,
        })
        .unwrap();

    let results = store.search("unique_pattern_xyz", 10).unwrap();
    assert_eq!(results.len(), 1);

    let no_results = store.search("nonexistent_pattern", 10).unwrap();
    assert!(no_results.is_empty());
}

#[test]
fn test_trace_store_count() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    assert_eq!(store.count().unwrap(), 0);

    for i in 0..5 {
        store
            .record(&TraceEvent {
                id: format!("tr-count-{i}"),
                event_type: TraceEventType::Search,
                timestamp: Utc::now(),
                session_id: None,
                duration_ms: 100,
                input: "{}".to_string(),
                output: "{}".to_string(),
                metadata: "{}".to_string(),
                success: true,
                error: None,
            })
            .unwrap();
    }

    assert_eq!(store.count().unwrap(), 5);
}

#[test]
fn test_trace_store_tool_traces() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    let tool_trace = ToolTrace::new("sess-tool".to_string(), "Edit".to_string(), 1);
    store.record_tool_trace(&tool_trace).unwrap();

    let traces = store.get_tool_traces("sess-tool", 10).unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].tool_name, "Edit");

    let last = store.get_last_tool_trace("sess-tool").unwrap();
    assert!(last.is_some());
    assert_eq!(last.unwrap().tool_name, "Edit");
}

#[test]
fn test_trace_store_surfaced_items() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    let item = SurfacedItem {
        session_id: "sess-surf".to_string(),
        item_id: "mem-001".to_string(),
        item_type: "memory".to_string(),
        item_preview: Some("Test memory".to_string()),
        surfaced_at: Utc::now(),
        feedback_given: false,
    };
    store.record_surfaced_item(&item).unwrap();

    let unfeedback = store.get_unfeedback_surfaced_items(10).unwrap();
    assert_eq!(unfeedback.len(), 1);

    store.mark_surfaced_feedback("mem-001").unwrap();

    let unfeedback_after = store.get_unfeedback_surfaced_items(10).unwrap();
    assert!(unfeedback_after.is_empty());
}

#[test]
fn test_trace_store_observation_buffer() {
    let temp = TempDir::new().unwrap();
    let store = TraceStore::open(&temp.path().join("traces.db")).unwrap();

    let obs = BufferedObservation {
        session_id: "sess-obs".to_string(),
        tool_name: "Write".to_string(),
        file_path: Some("/src/main.rs".to_string()),
        content: "fn main() {}".to_string(),
        exit_code: None,
        is_error: false,
        timestamp: Utc::now(),
    };
    store.buffer_observation(&obs).unwrap();

    let count = store.observation_buffer_count("sess-obs").unwrap();
    assert_eq!(count, 1);

    let observations = store.get_buffered_observations("sess-obs").unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].tool_name, "Write");

    store.clear_observation_buffer("sess-obs").unwrap();
    let count_after = store.observation_buffer_count("sess-obs").unwrap();
    assert_eq!(count_after, 0);
}

// =========================================================================
// TraceStats tests
// =========================================================================

#[test]
fn test_trace_stats_success_rate() {
    let stats = TraceStats {
        event_type: TraceEventType::Search,
        count: 10,
        avg_duration_ms: 100.0,
        success_count: 8,
        failure_count: 2,
    };
    assert!((stats.success_rate() - 0.8).abs() < 0.001);

    let zero_stats = TraceStats {
        event_type: TraceEventType::Search,
        count: 0,
        avg_duration_ms: 0.0,
        success_count: 0,
        failure_count: 0,
    };
    assert!((zero_stats.success_rate() - 0.0).abs() < 0.001);
}

// =========================================================================
// TraceTimer tests
// =========================================================================





// =========================================================================
// generate_trace_id tests
// =========================================================================

#[test]
fn test_generate_trace_id() {
    let id1 = generate_trace_id();
    let id2 = generate_trace_id();

    assert!(id1.starts_with("tr-"));
    assert!(id2.starts_with("tr-"));
    // May be same if called quickly, but format should be correct
}
