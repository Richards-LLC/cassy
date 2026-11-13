use super::*;

/// Test that Stop hook blocks when epic has incomplete subtasks
///
/// Verifies the full scenario:
/// 1. Epic with 2 subtasks, agent starts and closes subtask 1
/// 2. Subtask 2 remains open
/// 3. Stop hook should block because the agent's epic has incomplete subtasks
///
/// Uses direct hook invocation for reliability (MCP task creation is tested
/// separately in test_mcp_basic_task_operations).
#[test]
#[ignore]
fn test_mcp_stop_hook_blocks_incomplete_epic() {
    let env = HookTestEnv::new();

    let agent_id = HOOK_TEST_SESSION_ID.to_string();
    env.register_agent(&agent_id, "test-agent-mcp-epic", "primary");

    // Create an epic with 2 subtasks
    let epic_id =
        env.cas
            .create_task_with_options("E2E Test Epic", Some("epic"), None, false);
    println!("Created epic: {}", epic_id);

    let subtask1_id = env.cas.create_task("Subtask 1");
    env.add_dependency(&subtask1_id, &epic_id, "parent-child");
    println!("Created subtask 1: {}", subtask1_id);

    let subtask2_id = env.cas.create_task("Subtask 2");
    env.add_dependency(&subtask2_id, &epic_id, "parent-child");
    println!("Created subtask 2: {}", subtask2_id);

    // Agent starts subtask 1, adds epic to working_epics
    env.cas.start_task(&subtask1_id);
    env.set_task_assignee(&subtask1_id, &agent_id);
    env.add_working_epic(&agent_id, &epic_id);
    println!("Agent started subtask 1");

    // Agent closes subtask 1 (completed their work)
    env.cas.close_task(&subtask1_id);
    println!("Agent closed subtask 1");

    // Write session file and enable exit blockers
    let session_file = env.dir().join(".cas").join("current_session");
    std::fs::write(&session_file, &agent_id).expect("Failed to write session file");

    let config_path = env.dir().join(".cas").join("config.toml");
    std::fs::write(&config_path, "[tasks]\nblock_exit_on_open = true\n")
        .expect("Failed to write config");

    // Run Stop hook - should block because subtask 2 is still open
    let (_, stop_stdout) = env.run_stop_hook();
    println!("Stop hook stdout: {}", stop_stdout);

    // Verify subtask 2 is still open
    let tasks = env.get_tasks();
    println!("\nTasks in database:");
    for (id, title, status) in &tasks {
        println!("  {} - {} ({})", id, title, status);
    }
    let subtask2_open = tasks
        .iter()
        .any(|(_, title, status)| title.contains("Subtask 2") && status != "closed");
    assert!(
        subtask2_open,
        "Subtask 2 should still be open (not closed)"
    );

    assert!(stop_hook_blocks(&stop_stdout), "{stop_stdout}");
}

/// Helper to extract CAS task ID from text (cas-XXXX format)
fn extract_cas_id(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"(cas-[a-f0-9]{4})").ok()?;
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Test basic MCP tool usage in a real Claude session
///
/// Simple sanity check that CAS MCP tools work in a real session.
/// Tests task creation and starting (not closing, which triggers verification).
#[tokio::test]
#[ignore]
async fn test_mcp_basic_task_operations() {
    let env = HookTestEnv::new();

    println!("Test environment: {:?}", env.dir());

    let result = session(
        QueryOptions::new()
            .model("haiku")
            .cwd(env.dir())
                        .mcp_config(env.mcp_config_path())
            .max_turns(6)
            .allow_tool("mcp__cas__task")
            .allow_tool("mcp__cas__memory")
            .allow_tool("mcp__cas__search"),
        |mut sess| async move {
            // Create a task
            println!("\n=== Creating task via MCP ===");
            sess.send(
                "Use the mcp__cas__task tool with action='create' and title='MCP Test Task'. \
                Tell me the task ID that was created.",
            )
            .await?;

            let mut task_id = String::new();
            loop {
                let msg = sess.await_response(None).await?;
                if let Message::Assistant(ref a) = msg {
                    let text = a.text();
                    println!("Assistant: {}", text);
                    if let Some(id) = extract_cas_id(&text) {
                        task_id = id;
                    }
                }
                if matches!(msg, Message::Result(_)) {
                    break;
                }
            }

            assert!(!task_id.is_empty(), "Should have created a task");
            println!("Created task: {}", task_id);

            // Start the task
            println!("\n=== Starting task via MCP ===");
            sess.send(&format!(
                "Use mcp__cas__task tool with action='start' and id='{}'. Confirm it started.",
                task_id
            ))
            .await?;

            loop {
                let msg = sess.await_response(None).await?;
                if let Message::Assistant(ref a) = msg {
                    println!("Assistant: {}", a.text());
                }
                if let Message::Result(ref r) = msg {
                    return Ok(r.clone());
                }
            }
        },
    )
    .await;

    match result {
        Ok(r) => {
            println!("\n=== Test passed ===");
            println!("Final result: {:?}", r.result);

            // Verify task was created and started
            let tasks = env.get_tasks();
            println!("\nTasks in database:");
            for (id, title, status) in &tasks {
                println!("  {} - {} ({})", id, title, status);
            }

            let test_task = tasks
                .iter()
                .find(|(_, title, _)| title.contains("MCP Test Task"));
            assert!(test_task.is_some(), "Test task should exist in database");

            let (_, _, status) = test_task.unwrap();
            assert_eq!(status, "in_progress", "Test task should be in_progress");
        }
        Err(e) => {
            panic!("Session failed: {:?}", e);
        }
    }
}

/// Test session-based agent identification with auto-registration
///
/// Verifies that the MCP server auto-registers the agent using a session-based
/// UUID identifier (not the old cc-PPID format). The agent is auto-registered
/// on the first MCP tool call.
#[tokio::test]
#[ignore]
async fn test_session_based_agent_identification() {
    let env = HookTestEnv::new();

    println!("Test environment created at: {:?}", env.dir());

    // Run a minimal Claude session - just call whoami to trigger auto-registration
    let result = session(
        QueryOptions::new()
            .model("haiku")
            .cwd(env.dir())
            .mcp_config(env.mcp_config_path())
            .max_turns(3)
            .allow_tool("mcp__cas__coordination"),
        |mut sess| async move {
            sess.send(
                "Use mcp__cas__coordination with action='whoami'. Report the full agent ID.",
            )
            .await?;

            loop {
                let msg = sess.await_response(None).await?;
                if let Message::Assistant(ref a) = msg {
                    println!("Assistant: {}", a.text());
                }
                if let Message::Result(ref r) = msg {
                    return Ok(r.clone());
                }
            }
        },
    )
    .await;

    match result {
        Ok(_r) => {
            println!("\n=== Session completed, verifying session-based agent ID ===");

            // Check agents in database
            let db_path = env.dir().join(".cas").join("cas.db");
            let conn = rusqlite::Connection::open(&db_path).expect("Failed to open database");

            let agents: Vec<(String, Option<u32>)> = {
                let mut stmt = conn
                    .prepare("SELECT id, ppid FROM agents")
                    .unwrap();
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            };

            println!("\nAgents in database:");
            for (id, ppid) in &agents {
                println!("  ID: {} (PPID: {:?})", id, ppid);
            }

            assert!(
                !agents.is_empty(),
                "Should have at least one agent registered"
            );

            let agent_id = &agents[0].0;

            // Agent ID should be UUID format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
            let parts: Vec<&str> = agent_id.split('-').collect();
            assert!(
                parts.len() >= 4,
                "Agent ID should be UUID format (session_id), got: {}",
                agent_id
            );

            assert!(
                agent_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
                "Agent ID should be hex/hyphen UUID format, got: {}",
                agent_id
            );

            // Should NOT use old cc-PPID format
            assert!(
                !agent_id.starts_with("cc-"),
                "Agent ID should be session_id (UUID), not PPID-based, got: {}",
                agent_id
            );

            println!("\n=== SUCCESS: Session-based agent identification verified ===");
            println!("Agent ID (session_id): {}", agent_id);
        }
        Err(e) => {
            panic!("Session failed: {:?}", e);
        }
    }
}

// =============================================================================
// Hook Command E2E Tests (Direct hook invocation, no Claude API needed)
// =============================================================================

// Test that PreToolUse hook blocks Read when in verification jail.
// This test directly invokes the hook command to verify the jail blocking logic.
// No Claude API needed - tests the actual hook implementation.
