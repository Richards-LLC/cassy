//! cas-d757 (GH #1219): factory workers may not run operator-authenticated
//! CLIs (vercel and neonctl) or call their MCP tools; supervisors may.
use crate::config::Config;
use crate::hooks::handlers::handle_pre_tool_use;
use crate::test_support::TestEnvGuard;
use cas_core::hooks::types::{HookInput, HookOutput};
use serde_json::{Value, json};

fn input(tool: &str, args: Value, cwd: &std::path::Path) -> HookInput {
    HookInput {
        session_id: "worker-denied-session".into(),
        cwd: cwd.display().to_string(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some(tool.into()),
        tool_input: Some(args),
        ..HookInput::default()
    }
}

fn bash(command: &str, cwd: &std::path::Path) -> HookInput {
    input("Bash", json!({ "command": command }), cwd)
}

fn denial(output: HookOutput) -> Option<String> {
    let value = serde_json::to_value(output).unwrap();
    (value
        .pointer("/hookSpecificOutput/permissionDecision")
        .and_then(Value::as_str)
        == Some("deny"))
    .then(|| {
        value
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .and_then(Value::as_str)
            .unwrap()
            .to_string()
    })
}

fn role_env(role: &str) -> (TestEnvGuard, tempfile::TempDir) {
    let mut env = TestEnvGuard::temp_home();
    let config_home = tempfile::tempdir().unwrap();
    env.set("XDG_CONFIG_HOME", config_home.path());
    env.set("CAS_AGENT_ROLE", role);
    env.set("CAS_AGENT_NAME", if role == "worker" { "quiet-wren-12" } else { "calm-owl-1" });
    env.set("CAS_FACTORY_SESSION", "cas-src-d757");
    (env, config_home)
}

const WRAPPED_CLIS: &[(&str, &str)] = &[
    ("vercel", "vercel env ls production"),
    ("vercel", "env VERCEL_ORG_ID=team_1 vercel env pull .env.local"),
    ("vercel", "sudo -u deploy vercel ls"),
    ("vercel", "sh -c 'vercel env ls production'"),
    ("vercel", "cd web && npx vercel logs"),
    ("vercel", "/usr/local/bin/vercel whoami"),
    ("neonctl", "neonctl projects list"),
    ("neonctl", "env NEON_PROJECT_ID=fixture neonctl branches list"),
    ("neonctl", "sh -c 'neonctl projects list'"),
    ("neonctl", "npx -y neonctl@latest projects list"),
    ("neonctl", "pnpm dlx neonctl projects list"),
    ("neonctl", "/usr/local/bin/neonctl projects list"),
];

/// A worker running a stock denied CLI through wrappers or a path
/// is refused with the CLI named and the blocker route, with and without a
/// CAS root.
#[test]
fn a_worker_is_refused_default_clis_however_wrapped() {
    let (_env, home) = role_env("worker");
    let project = tempfile::tempdir().unwrap();
    let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
    for (cli, command) in WRAPPED_CLIS {
        for root in [Some(cas_root.as_path()), None] {
            let reason = denial(handle_pre_tool_use(&bash(command, home.path()), root).unwrap())
                .unwrap_or_else(|| panic!("{command} was allowed (root {root:?})"));
            assert!(reason.contains(&format!("`{cli}`")), "{reason}");
            assert!(reason.contains("blocker=true"), "{reason}");
            assert!(reason.contains("target=supervisor"), "{reason}");
        }
    }
    for command in ["git log --grep vercel", "cat vercel.json", "git log --grep neonctl", "cat .env.cas-db", "npm run build"] {
        let output = handle_pre_tool_use(&bash(command, home.path()), Some(&cas_root)).unwrap();
        assert!(
            !denial(output).is_some_and(|reason| reason.contains("WORKER-DENIED")),
            "{command}"
        );
    }
}

/// Supervisors and non-factory sessions are unaffected.
#[test]
fn a_supervisor_and_a_plain_session_may_run_default_denied_clis() {
    for role in ["supervisor", ""] {
        let (mut env, home) = role_env(role);
        if role.is_empty() {
            env.remove("CAS_AGENT_ROLE");
            env.remove("CAS_FACTORY_SESSION");
        }
        for (_, command) in WRAPPED_CLIS {
            let output = handle_pre_tool_use(&bash(command, home.path()), None).unwrap();
            assert!(
                !denial(output).is_some_and(|reason| reason.contains("WORKER-DENIED")),
                "{role:?}: {command}"
            );
        }
        let output = handle_pre_tool_use(
            &input("mcp__claude_ai_Vercel__list_deployments", json!({}), home.path()),
            None,
        )
        .unwrap();
        assert!(!denial(output).is_some_and(|reason| reason.contains("WORKER-DENIED")), "{role:?}");
        for (tool, args) in [
            ("mcp__Neon__list_projects", json!({})),
            ("mcp__cs__factory", json!({"action": "db_branch_create", "task_id": "cas-fixture"})),
        ] {
            let output = handle_pre_tool_use(&input(tool, args, home.path()), None).unwrap();
            assert!(!denial(output).is_some_and(|reason| reason.contains("WORKER-DENIED")), "{role:?}: {tool}");
        }
    }
}

/// A matching MCP or connector tool is refused for a worker, directly or
/// through mcp_execute; other tools are not.
#[test]
fn a_worker_is_refused_a_matching_mcp_tool() {
    let (_env, home) = role_env("worker");
    for (tool, args) in [
        ("mcp__claude_ai_Vercel__list_deployments", json!({})),
        ("mcp__vercel__get_project", json!({"id": "web"})),
        ("mcp__Neon__list_projects", json!({})),
        ("mcp__claude_ai_Neon__list_branches", json!({})),
        ("mcp__cs__mcp_execute", json!({"code": "neon.list_projects({})"})),
        ("mcp__cs__mcp_execute", json!({"code": "vercel.list_projects({})"})),
    ] {
        let reason = denial(handle_pre_tool_use(&input(tool, args, home.path()), None).unwrap())
            .unwrap_or_else(|| panic!("{tool} was allowed"));
        assert!(reason.contains("WORKER-DENIED TOOL"), "{reason}");
        assert!(reason.contains("blocker=true"), "{reason}");
    }
    let output = handle_pre_tool_use(
        &input("mcp__github__get_issue", json!({"number": 1}), home.path()),
        None,
    )
    .unwrap();
    assert!(!denial(output).is_some_and(|reason| reason.contains("WORKER-DENIED")));
}

/// The lists come from `factory.worker_denied_clis` and
/// `factory.worker_denied_mcp_tools`; an operator can change them.
#[test]
fn the_denied_lists_follow_project_config() {
    let (_env, home) = role_env("worker");
    let project = tempfile::tempdir().unwrap();
    let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
    let mut config = Config::load(&cas_root).unwrap_or_default();
    config.set("factory.worker_denied_clis", "netlify").unwrap();
    config.set("factory.worker_denied_mcp_tools", "mcp__neon__*").unwrap();
    config.save(&cas_root).unwrap();

    let netlify = denial(
        handle_pre_tool_use(&bash("netlify deploy --prod", home.path()), Some(&cas_root)).unwrap(),
    )
    .unwrap();
    assert!(netlify.contains("`netlify`"), "{netlify}");
    let vercel =
        handle_pre_tool_use(&bash("vercel ls", home.path()), Some(&cas_root)).unwrap();
    assert!(!denial(vercel).is_some_and(|reason| reason.contains("WORKER-DENIED")));
    let neon = handle_pre_tool_use(&bash("neonctl projects list", home.path()), Some(&cas_root)).unwrap();
    assert!(!denial(neon).is_some_and(|reason| reason.contains("WORKER-DENIED")));
    assert!(
        denial(
            handle_pre_tool_use(
                &input("mcp__neon__run_sql", json!({}), home.path()),
                Some(&cas_root)
            )
            .unwrap()
        )
        .is_some_and(|reason| reason.contains("mcp__neon__*"))
    );
    config.set("factory.worker_denied_clis", "").unwrap();
    config.set("factory.worker_denied_mcp_tools", "").unwrap();
    config.save(&cas_root).unwrap();
    for (tool, args) in [
        ("Bash", json!({"command": "neonctl projects list"})),
        ("mcp__neon__list_projects", json!({})),
    ] {
        let output = handle_pre_tool_use(&input(tool, args, home.path()), Some(&cas_root)).unwrap();
        assert!(!denial(output).is_some_and(|reason| reason.contains("WORKER-DENIED")), "{tool}");
    }
}
