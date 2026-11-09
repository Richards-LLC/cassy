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
use cas::store::open_store;
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
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas(&mut test_env);

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
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas(&mut test_env);

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
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas(&mut test_env);

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
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas(&mut test_env);

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
    let mut test_env = TestEnvGuard::temp_home();
    let (_temp, service) = setup_cas(&mut test_env);

    let mut req = base_request("unknown mode".to_string(), "x", "x-tag");
    req.mode = Some("yolo".to_string());
    let err = service
        .cas_remember(Parameters(req))
        .await
        .expect_err("unknown mode should error");
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

/// GH #992 (cas-0339): a handoff is never merged into, or blocked by, an
/// existing memory. Saving a new one supersedes the previous current handoff
/// for the same role: the old entry keeps its content as history, gains the
/// `superseded` and `superseded-by:<new>` tags, and leaves the active tier.
/// A handoff of another role is untouched.
#[tokio::test]
async fn a_new_handoff_supersedes_the_previous_one_for_its_role_cas_0339() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, service) = setup_cas(&mut test_env);

    test_env.remove("CAS_AGENT_ROLE");

    let body = |which: &str| {
        format!(
            "CURRENT handoff ({which}). Session cas-src, supervisor noble-heron-32. \
             Burn-down epic cas-f0c7 is mid-assembly; merge the parked hub-web lanes, \
             then cut the release with the rebuilt dist and post the Slack pair."
        )
    };
    let remember = |content: String, entry_type: &str, tags: &str| {
        let mut req = base_request(content, "Session handoff", tags);
        req.entry_type = entry_type.to_string();
        req
    };
    let slug_of = |result: &rmcp::model::CallToolResult| {
        assert_eq!(result.is_error, Some(false), "{result:?}");
        result
            .structured_content
            .as_ref()
            .expect("structured content")["slug"]
            .as_str()
            .expect("slug")
            .to_string()
    };

    let first = service
        .cas_remember(Parameters(remember(body("first"), "handoff", "summary")))
        .await
        .expect("first handoff");
    let first_id = slug_of(&first);
    let worker = service
        .cas_remember(Parameters(remember(
            "Worker handoff: resume cas-0339 on factory/crisp-fox-22.".to_string(),
            "handoff",
            "role:worker",
        )))
        .await
        .expect("worker handoff");
    let worker_id = slug_of(&worker);
    // The legacy spelling, a learning tagged `handoff`, is a handoff too, and
    // its near-identical text is not blocked as an overlap.
    let second = service
        .cas_remember(Parameters(remember(
            body("second"),
            "learning",
            "summary,handoff",
        )))
        .await
        .expect("second handoff");
    let second_id = slug_of(&second);
    let second_text = extract_text(second);
    assert!(
        second_text.contains(&format!("Superseded (kept as history): {first_id}")),
        "{second_text}"
    );

    let store = open_store(&temp.path().join(".cas")).expect("open store");
    let first = store.get(&first_id).expect("first kept as history");
    assert_eq!(first.content, body("first"), "content is never overwritten");
    assert_eq!(first.entry_type, cas::types::EntryType::Context);
    for tag in [
        "summary",
        "handoff",
        "role:supervisor",
        "superseded",
        &format!("superseded-by:{second_id}"),
    ] {
        assert!(
            first.tags.iter().any(|t| t == tag),
            "missing {tag}: {:?}",
            first.tags
        );
    }
    assert!(
        first.valid_until.is_some(),
        "a superseded handoff stops being valid"
    );
    assert_eq!(first.memory_tier, cas::types::MemoryTier::Cold);

    let second = store.get(&second_id).expect("second");
    assert!(
        second.tags.iter().any(|t| t == "role:supervisor"),
        "{:?}",
        second.tags
    );
    assert!(
        !second.tags.iter().any(|t| t == "superseded"),
        "{:?}",
        second.tags
    );
    assert_eq!(second.valid_until, None);

    let worker = store.get(&worker_id).expect("worker");
    assert!(
        !worker.tags.iter().any(|t| t == "superseded"),
        "another role's handoff is untouched: {:?}",
        worker.tags
    );
}

/// GH #1661: replay two supervisors clearing on different machines, then an
/// explicit same-name factory restart on A. Machine/session ownership must
/// survive both supersession and the real SessionStart context builder.
#[tokio::test]
async fn supervisor_handoffs_stay_in_their_machine_lineage_cas_fb21() {
    use cas_core::hooks::{DefaultHooksConfig, HookInput};
    use cas_core::hooks::context::{ContextStores, build_context_with_stores};
    use cas_core::memory::handoff;
    use cas::types::{Agent, AgentRole};

    let mut env = TestEnvGuard::temp_home();
    let (temp, _) = setup_cas(&mut env);
    let root = temp.path().join(".cas");
    let agents = cas::store::open_agent_store(&root).unwrap();
    let memories = open_store(&root).unwrap();
    let register = |id: &str, name: &str, machine: &str, session: &str| {
        let mut agent = Agent::new_with_role(id.into(), name.into(), AgentRole::Supervisor);
        agent.machine_id = Some(machine.into());
        agent.factory_session = Some(session.into());
        agents.register(&agent).unwrap();
        let core = cas::mcp::CasCore::with_daemon(root.clone(), None, None);
        core.set_agent_id_for_testing(id.into());
        core
    };
    let save = |body: &str| {
        let mut req = base_request(body.into(), "Handoff", "role:supervisor");
        req.entry_type = "handoff".into();
        req
    };
    let id = |result: rmcp::model::CallToolResult| {
        assert_eq!(result.is_error, Some(false), "{result:?}");
        result.structured_content.unwrap()["slug"].as_str().unwrap().to_string()
    };
    let a = register("a-old", "supervisor-a", "machine-a", "factory-a");
    let a_old = id(a.cas_remember(Parameters(save("A approval receipt"))).await.unwrap());
    let a_new = id(a.cas_remember(Parameters(save("A newest approval receipt"))).await.unwrap());
    assert!(handoff::is_superseded(&memories.get(&a_old).unwrap()));
    let b = register("b", "supervisor-a", "machine-b", "factory-b");
    let b_id = id(b.cas_remember(Parameters(save("B production approval receipt"))).await.unwrap());
    assert!(!handoff::is_superseded(&memories.get(&a_new).unwrap()));
    let other = register("a-other", "other-supervisor", "machine-a", "factory-other");
    let other_id = id(other.cas_remember(Parameters(save("Other local session approval"))).await.unwrap());
    // Reusing the logical supervisor name is the registry's explicit restart
    // contract; both the native session and factory process lifetime change.
    let restarted = register("a-restart", "supervisor-a", "machine-a", "factory-a-restart");
    let surfaced = |agent_id: Option<&str>, session_id: &str| {
        let surfaced = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = surfaced.clone();
        let callback: cas_core::hooks::context::SurfacedItemCallback = Box::new(move |id, kind, _| {
            if kind == "handoff" { captured.lock().unwrap().push(id.to_string()); }
        });
        let stores = ContextStores {
            project_store: Some(memories.as_ref()),
            agent_store: Some(agents.as_ref()),
            agent_id: agent_id.map(str::to_string),
            ..ContextStores::empty()
        };
        let input = HookInput {
            session_id: session_id.into(),
            agent_role: Some("supervisor".into()),
            ..Default::default()
        };
        build_context_with_stores(&input, &stores, &DefaultHooksConfig::new(), 10,
            Some(&callback), "mcp__cs__").unwrap();
        surfaced.lock().unwrap().clone()
    };
    for (agent, native_session, expected) in [
        (Some("a-old"), "a-after-clear", Some(a_new.as_str())),
        (Some("b"), "b-after-clear", Some(b_id.as_str())),
        (Some("a-other"), "a-other", Some(other_id.as_str())),
        (Some("a-restart"), "a-restart", Some(a_new.as_str())),
        (None, "unknown-session", None),
    ] {
        assert_eq!(surfaced(agent, native_session), expected.into_iter().map(str::to_string).collect::<Vec<_>>());
    }
    let restart_id = id(restarted.cas_remember(Parameters(save("A restarted approval"))).await.unwrap());
    assert!(handoff::is_superseded(&memories.get(&a_new).unwrap()));
    assert!(!handoff::is_superseded(&memories.get(&b_id).unwrap()));
    assert!(!handoff::is_superseded(&memories.get(&other_id).unwrap()));
    assert_eq!(surfaced(Some("a-restart"), "a-restart"), [restart_id]);
}
