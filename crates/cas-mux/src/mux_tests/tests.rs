use crate::harness::SupervisorCli;
use crate::mux::*;
use crate::pane::UserInputKind;
use crate::spec::{Effort, WorkerSpec};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;

// cas-mux tests mutate these process-wide launch probes to make assertions
// independent of the host's installed harness binaries.
static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

struct RestoreEnv {
    key: &'static str,
    previous: Option<OsString>,
}

impl RestoreEnv {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: callers hold `TEST_ENV_LOCK` for the lifetime of the guard,
        // serializing process-wide test environment mutation in this binary.
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, previous }
    }
}

impl Drop for RestoreEnv {
    fn drop(&mut self) {
        // SAFETY: this guard is only constructed while `TEST_ENV_LOCK` is held.
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn env_value<'a>(config: &'a crate::pty::PtyConfig, key: &str) -> Option<&'a str> {
    config
        .env
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

#[test]
fn factory_pane_configs_propagates_machine_registration_credentials() {
    let _env_lock = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let config_home = tempfile::tempdir().expect("temporary config home");
    let config_path = config_home.path().join("code-mode-mcp/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &config_path,
        r#"
[servers.mecha-cassy]
transport = "http"
auth = "env:MECHA_SLACK_TOKEN_TEST_WORKER"

[servers.mecha-cassy.headers]
x-vercel-protection-bypass = "env:MECHA_VERCEL_BYPASS_TEST_WORKER"

[servers.unrelated]
auth = "env:UNRELATED_CREDENTIAL_MUST_NOT_PROPAGATE"
"#,
    )
    .unwrap();
    let _config_home = RestoreEnv::set("XDG_CONFIG_HOME", config_home.path());
    let _token = RestoreEnv::set("MECHA_SLACK_TOKEN_TEST_WORKER", "token-value");
    let _bypass = RestoreEnv::set("MECHA_VERCEL_BYPASS_TEST_WORKER", "bypass-value");
    let _unrelated = RestoreEnv::set("UNRELATED_CREDENTIAL_MUST_NOT_PROPAGATE", "unrelated");

    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        include_director: false,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);
    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker config must be present");

    assert_eq!(
        env_value(worker_config, "MECHA_SLACK_TOKEN_TEST_WORKER"),
        Some("token-value")
    );
    assert_eq!(
        env_value(worker_config, "MECHA_VERCEL_BYPASS_TEST_WORKER"),
        Some("bypass-value")
    );
    assert_eq!(
        env_value(worker_config, "UNRELATED_CREDENTIAL_MUST_NOT_PROPAGATE"),
        None,
        "only machine-registration credentials belong in worker panes"
    );
}

fn codex_factory_session_arg(config: &crate::pty::PtyConfig) -> Option<&str> {
    config
        .args
        .iter()
        .find(|arg| arg.starts_with("mcp_servers.cs.env.CAS_FACTORY_SESSION="))
        .map(String::as_str)
}

// ── cas-d571: effort config flows through Mux::factory() to PTY args ─────────
// Tests the full MuxConfig → Mux::factory_pane_configs() → PtyConfig.args chain.
// Uses `factory_pane_configs` (config-only, no spawn) so tests run without a
// real `claude` or `codex` binary present.

#[test]
fn factory_pane_configs_supervisor_effort_reaches_pty_args() {
    let _env_lock = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _effort_support = RestoreEnv::set("CAS_FACTORY_EFFORT_SUPPORTED", "1");
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        supervisor_effort: Some("low".to_string()),
        worker_effort: Some("high".to_string()),
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, sup_config) = configs
        .iter()
        .find(|(name, _)| name == &config.supervisor_name)
        .expect("supervisor config must be present");
    let effort_idx = sup_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("supervisor PTY args must contain --effort");
    let effort_val = sup_config
        .args
        .get(effort_idx + 1)
        .expect("--effort must be followed by a value in supervisor PTY args");
    assert_eq!(
        effort_val, "low",
        "MuxConfig::supervisor_effort must reach supervisor PTY --effort arg"
    );
}

#[test]
fn factory_pane_configs_worker_effort_reaches_pty_args() {
    let _env_lock = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _effort_support = RestoreEnv::set("CAS_FACTORY_EFFORT_SUPPORTED", "1");
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        supervisor_effort: Some("low".to_string()),
        worker_effort: Some("high".to_string()),
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker-1 config must be present");
    let effort_idx = worker_config
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("worker PTY args must contain --effort");
    let effort_val = worker_config
        .args
        .get(effort_idx + 1)
        .expect("--effort must be followed by a value in worker PTY args");
    assert_eq!(
        effort_val, "high",
        "MuxConfig::worker_effort must reach worker PTY --effort arg"
    );
    // supervisor must be last in the returned vec (workers-first ordering)
    assert_eq!(
        configs.last().unwrap().0,
        config.supervisor_name,
        "supervisor must be the last entry in factory_pane_configs output"
    );
}

#[test]
fn factory_pane_configs_propagates_factory_session_to_process_and_codex_mcp_env() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["codex-worker".to_string()],
        supervisor_name: "codex-supervisor".to_string(),
        factory_session: Some("factory-session-a".to_string()),
        include_director: false,
        supervisor_cli: SupervisorCli::Codex,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };

    let configs = Mux::factory_pane_configs(&config);

    for (name, pty_config) in configs {
        assert_eq!(
            env_value(&pty_config, "CAS_FACTORY_SESSION"),
            Some("factory-session-a"),
            "{name} PTY env must include CAS_FACTORY_SESSION"
        );
        assert!(
            pty_config.args.contains(
                &"mcp_servers.cs.env.CAS_FACTORY_SESSION=\"factory-session-a\"".to_string()
            ),
            "{name} Codex cs MCP env must include CAS_FACTORY_SESSION; args={:?}",
            pty_config.args
        );
    }
}

#[test]
fn factory_pane_configs_propagates_supervisor_name_to_codex_worker_mcp_env() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["codex-worker".to_string()],
        supervisor_name: "codex-supervisor".to_string(),
        include_director: false,
        supervisor_cli: SupervisorCli::Codex,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };

    let configs = Mux::factory_pane_configs(&config);
    let worker = configs
        .iter()
        .find(|(name, _)| name == "codex-worker")
        .expect("worker pane config");

    assert_eq!(
        env_value(&worker.1, "CAS_SUPERVISOR_NAME"),
        Some("codex-supervisor"),
        "worker process env must know the owning supervisor"
    );
    assert!(
        worker
            .1
            .args
            .contains(&"mcp_servers.cs.env.CAS_SUPERVISOR_NAME=\"codex-supervisor\"".to_string()),
        "Codex starts cs with a restricted environment, so the owning supervisor name must be an explicit MCP env override; args={:?}",
        worker.1.args
    );
}

#[test]
fn build_add_worker_config_propagates_factory_session_for_dynamic_spawns() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 0,
        worker_names: vec![],
        factory_session: Some("factory-session-dynamic".to_string()),
        include_director: false,
        supervisor_cli: SupervisorCli::Claude,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };
    let mut mux = Mux::factory_state_for_test(&config);

    mux.set_default_worker_spec(WorkerSpec::codex_default("dynamic-worker"));
    let pty_config = mux.build_add_worker_config(
        "dynamic-worker",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        None,
    );

    assert_eq!(
        env_value(&pty_config, "CAS_FACTORY_SESSION"),
        Some("factory-session-dynamic")
    );
    assert!(
        pty_config.args.contains(
            &"mcp_servers.cs.env.CAS_FACTORY_SESSION=\"factory-session-dynamic\"".to_string()
        ),
        "dynamic Codex cs MCP env must include CAS_FACTORY_SESSION; args={:?}",
        pty_config.args
    );
}

#[test]
fn factory_session_codex_toml_arg_sanitizes_quote_and_newline() {
    let raw_session = "factory\"bad\nsession";
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["codex-worker".to_string()],
        factory_session: Some(raw_session.to_string()),
        include_director: false,
        supervisor_cli: SupervisorCli::Codex,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };

    let configs = Mux::factory_pane_configs(&config);

    for (name, pty_config) in configs {
        assert_eq!(
            env_value(&pty_config, "CAS_FACTORY_SESSION"),
            Some(raw_session),
            "{name} plain PTY env should keep the raw session value"
        );
        let toml_arg =
            codex_factory_session_arg(&pty_config).expect("Codex cs MCP session env arg");
        assert_eq!(
            toml_arg,
            "mcp_servers.cs.env.CAS_FACTORY_SESSION=\"factory_bad_session\""
        );
        assert_eq!(
            toml_arg.matches('"').count(),
            2,
            "{name} TOML arg should contain only the wrapping quotes"
        );
        assert!(
            !toml_arg.contains('\n'),
            "{name} TOML arg must not contain raw newlines"
        );
    }
}

/// cas-34f7f: when MuxConfig effort fields are None, --effort must be OMITTED
/// from PtyConfig args entirely.  Role-based defaults (xhigh/high) now live in
/// the cascade resolver layer, not in pty.rs spawn functions.
#[test]
fn factory_pane_configs_none_effort_omits_effort_flag() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        supervisor_effort: None,
        worker_effort: None,
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, sup_config) = configs
        .iter()
        .find(|(name, _)| name == &config.supervisor_name)
        .expect("supervisor config must be present");
    assert!(
        !sup_config.args.contains(&"--effort".to_string()),
        "supervisor with None effort must omit --effort (cas-34f7f); got: {:?}",
        sup_config.args
    );

    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker-1 config must be present");
    assert!(
        !worker_config.args.contains(&"--effort".to_string()),
        "worker with None effort must omit --effort (cas-34f7f); got: {:?}",
        worker_config.args
    );
}

#[test]
fn factory_pane_configs_codex_worker_inherits_supervisor_cli_in_cs_env_cas_1544() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Codex,
        worker_cli: crate::harness::SupervisorCli::Codex,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, worker_config) = configs
        .iter()
        .find(|(name, _)| name == "worker-1")
        .expect("worker-1 config must be present");
    let all_args = worker_config.args.join(" ");
    assert!(
        all_args.contains("mcp_servers.cs.env.CAS_FACTORY_SUPERVISOR_CLI=\"codex\""),
        "Codex worker cs MCP env must inherit the supervisor CLI alias for verification guidance: {all_args}"
    );
    assert!(
        worker_config
            .env
            .iter()
            .any(|(k, v)| k == "CAS_FACTORY_SUPERVISOR_CLI" && v == "codex"),
        "worker process env must also carry CAS_FACTORY_SUPERVISOR_CLI=codex: {:?}",
        worker_config.env
    );
}

// ── end cas-d571 ──────────────────────────────────────────────────────────────

// ── cas-3fed: per-worker spec storage + factory wiring ────────────────────────
// Tests the MuxConfig.resolved_worker_specs → factory_pane_configs per-worker
// CLI selection path, and the Mux::add_worker explicit spec override path.

/// Return the effective binary name from a PtyConfig, stripping any `nice`
/// wrapper that `CAS_FACTORY_NICE_WORKER=1` injects in the test environment.
fn effective_command(pty: &crate::pty::PtyConfig) -> &str {
    if pty.command == "nice" {
        // nice -n <level> <binary> [args...] → binary is at index 2
        pty.args.get(2).map(String::as_str).unwrap_or("nice")
    } else {
        &pty.command
    }
}

#[test]
fn factory_pane_configs_uses_per_worker_specs() {
    // worker-1 → Codex, worker-2 → Claude, but MuxConfig.worker_cli is Claude.
    // resolved_worker_specs must override the singular default per worker.
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 2,
        worker_names: vec!["worker-1".to_string(), "worker-2".to_string()],
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        resolved_worker_specs: vec![
            WorkerSpec {
                name: Some("worker-1".to_string()),
                cli: crate::harness::SupervisorCli::Codex,
                model: None,
                effort: None,
                config_dir: Some("/accounts/codex-research".to_string()),
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            },
            WorkerSpec {
                name: Some("worker-2".to_string()),
                cli: crate::harness::SupervisorCli::Claude,
                model: None,
                effort: None,
                config_dir: Some("/accounts/claude-review".to_string()),
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            },
        ],
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let (_, w1) = configs
        .iter()
        .find(|(n, _)| n == "worker-1")
        .expect("worker-1 must be present");
    let (_, w2) = configs
        .iter()
        .find(|(n, _)| n == "worker-2")
        .expect("worker-2 must be present");

    assert_eq!(
        effective_command(w1),
        "codex",
        "worker-1 with Codex spec must use codex binary"
    );
    assert_eq!(
        effective_command(w2),
        "claude",
        "worker-2 with Claude spec must use claude binary"
    );
    assert_eq!(
        spawned_env(w1, "CODEX_HOME"),
        Some("/accounts/codex-research"),
        "worker-1 must receive its own Codex account home"
    );
    assert_eq!(
        spawned_env(w1, "CAS_FACTORY_WORKER_ACCOUNT_DIR"),
        Some("/accounts/codex-research"),
        "worker-1 registration metadata must name its resolved account"
    );
    assert_eq!(
        spawned_env(w2, "CLAUDE_CONFIG_DIR"),
        Some("/accounts/claude-review"),
        "worker-2 must receive its own Claude account directory"
    );
    assert_eq!(
        spawned_env(w2, "CAS_FACTORY_WORKER_ACCOUNT_DIR"),
        Some("/accounts/claude-review"),
        "worker-2 registration metadata must name its resolved account"
    );
}

#[test]
fn factory_pane_configs_falls_back_to_singular_when_specs_empty() {
    // resolved_worker_specs is empty → all workers use worker_cli = Codex.
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 2,
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Codex,
        resolved_worker_specs: vec![],
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    for (name, pty_config) in &configs {
        if name == &config.supervisor_name {
            assert_eq!(
                effective_command(pty_config),
                "claude",
                "supervisor must use claude binary"
            );
        } else {
            assert_eq!(
                effective_command(pty_config),
                "codex",
                "worker {name} with empty resolved_worker_specs must fall back to worker_cli=Codex"
            );
            // PtyConfig::codex ignores the effort argument (_effort) intentionally;
            // verify --effort does NOT appear in the codex worker args (cas-206d coverage).
            assert!(
                !pty_config.args.iter().any(|a| a == "--effort"),
                "codex worker must NOT have --effort in args (codex ignores effort)"
            );
        }
    }
}

#[test]
fn add_worker_uses_explicit_spec() {
    // Mux default is Claude (builtin_default), but build_add_worker_config with
    // an explicit Codex spec must produce a codex PtyConfig.
    let mux = Mux::new(24, 80);

    let codex_spec = WorkerSpec {
        name: Some("dynamic-worker".to_string()),
        cli: crate::harness::SupervisorCli::Codex,
        model: None,
        effort: Some(Effort::High),
        config_dir: None,
        requester_config_dir: None,
        requester_secure_storage_dir: None,
    };

    let pty_config = mux.build_add_worker_config(
        "dynamic-worker",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(codex_spec),
    );

    assert_eq!(
        effective_command(&pty_config),
        "codex",
        "explicit Codex spec must override Claude default in dynamic add_worker path"
    );

    // Without explicit spec, the default (Claude) must be used.
    let claude_config = mux.build_add_worker_config(
        "another-worker",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        None,
    );
    assert_eq!(
        effective_command(&claude_config),
        "claude",
        "no explicit spec must fall back to Mux default (Claude)"
    );
}

// ── end cas-3fed ──────────────────────────────────────────────────────────────

// ── cas-3fed autofix: priority-2 branch coverage ─────────────────────────────

#[test]
fn effective_worker_spec_uses_worker_specs_map() {
    // Priority 2: per-worker entry in Mux::worker_specs wins over the
    // default when no explicit spec is supplied (priority 1 absent).
    let mut mux = Mux::new(24, 80);
    // builtin_default → Claude; override just "worker-map" to Codex.
    let codex_spec = WorkerSpec {
        name: Some("worker-map".to_string()),
        cli: crate::harness::SupervisorCli::Codex,
        model: None,
        effort: None,
        config_dir: None,
        requester_config_dir: None,
        requester_secure_storage_dir: None,
    };
    mux.set_worker_spec("worker-map", codex_spec);

    // No explicit spec → should pick up the map entry.
    let effective = mux.effective_worker_spec("worker-map", None);
    assert_eq!(
        effective.cli,
        crate::harness::SupervisorCli::Codex,
        "worker_specs map entry must take priority over default when no explicit spec is passed"
    );

    // A name not in the map should still fall through to the default.
    let default_effective = mux.effective_worker_spec("unknown-worker", None);
    assert_eq!(
        default_effective.cli,
        crate::harness::SupervisorCli::Claude,
        "unknown worker must fall back to Mux default (Claude builtin_default)"
    );
}

// ── end priority-2 coverage ───────────────────────────────────────────────────

// ── cas-b68a: add_worker persists the resolved spec ──────────────────────────

/// Regression for cas-b68a: a dynamically-spawned Codex worker in a Claude-DEFAULT
/// factory must resolve as Codex via `effective_worker_spec(name, None)` AFTER the
/// spawn.
///
/// This is the load-bearing proof for AC2's live path. The daemon's harness-aware
/// message router (`FactoryApp::harness_for`) resolves a worker's harness through
/// `effective_worker_spec(name, None)`. Before the fix, `add_worker` *used* an
/// explicit per-spawn spec but never persisted it into `worker_specs`, so a Codex
/// worker added to a Claude-default factory (the exact bug scenario) resolved back
/// to the Claude default — and the entire routing fix would be inert, silently
/// inboxing messages the codex process can never read.
#[cfg(unix)]
#[test]
fn add_worker_persists_explicit_spec_so_effective_resolves_codex() {
    use std::os::unix::fs::PermissionsExt;

    let _env_lock = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bin_dir =
        std::env::temp_dir().join(format!("cas-mux-add-worker-test-{}", std::process::id()));
    std::fs::create_dir_all(&bin_dir).expect("fake-bin directory must be creatable");
    for binary in ["cas", "codex"] {
        let path = bin_dir.join(binary);
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("fake worker binary must be writable");
        let mut permissions = std::fs::metadata(&path)
            .expect("fake worker binary metadata must be readable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions)
            .expect("fake worker binary must be executable");
    }
    // Keep system utilities such as the optional `nice` wrapper available;
    // the fake directory remains first so the test never launches a real Codex.
    let mut path_entries = vec![bin_dir.clone()];
    if let Some(current_path) = std::env::var_os("PATH") {
        path_entries.extend(std::env::split_paths(&current_path));
    }
    let test_path = std::env::join_paths(path_entries)
        .expect("test PATH entries must be representable on this platform");
    let _path = RestoreEnv::set("PATH", test_path);

    let mut mux = Mux::new(24, 80);
    // Session default is Claude — the explicit per-spawn spec must persist and win.
    mux.set_default_worker_spec(WorkerSpec {
        name: None,
        cli: crate::harness::SupervisorCli::Claude,
        model: None,
        effort: None,
        config_dir: None,
        requester_config_dir: None,
        requester_secure_storage_dir: None,
    });

    // Pre-condition: with nothing registered, an unknown worker resolves to the
    // Claude default — i.e. without persistence the post-spawn lookup is wrong.
    assert_eq!(
        mux.effective_worker_spec("alice", None).cli,
        crate::harness::SupervisorCli::Claude,
        "precondition: unregistered worker resolves to the Claude session default"
    );

    // Dynamically spawn "alice" with an explicit Codex spec (the SpawnWorkers path).
    let dir = std::env::temp_dir();
    let spec = WorkerSpec::codex_default("alice");
    mux.add_worker("alice", dir, None, "supervisor", None, Some(spec))
        .expect("add_worker should spawn the worker pane");

    // Post-spawn: harness resolution (what FactoryApp::harness_for does) must now
    // report Codex, with NO explicit spec passed — proving the spec was persisted.
    assert_eq!(
        mux.effective_worker_spec("alice", None).cli,
        crate::harness::SupervisorCli::Codex,
        "after add_worker, a later harness lookup must resolve the persisted Codex \
         spec — not fall back to the Claude default (cas-b68a)"
    );

    drop(mux);
    std::fs::remove_dir_all(bin_dir).expect("fake-bin directory must be removable");
}

// ── cas-35fe: custom worker_names branch ─────────────────────────────────────

#[test]
fn factory_pane_configs_custom_worker_names() {
    // Use names that differ from auto-generated "worker-1"/"worker-2" so that
    // a regression swapping the custom-names branch back to auto-generation
    // would cause the assertions to fail.
    let config = MuxConfig {
        cwd: std::path::PathBuf::from("/tmp/test"),
        workers: 2,
        worker_names: vec!["alice".to_string(), "bob".to_string()],
        include_director: false,
        supervisor_cli: crate::harness::SupervisorCli::Claude,
        worker_cli: crate::harness::SupervisorCli::Claude,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&config);

    let names: Vec<&str> = configs.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"alice"),
        "factory_pane_configs must honour custom worker name 'alice'"
    );
    assert!(
        names.contains(&"bob"),
        "factory_pane_configs must honour custom worker name 'bob'"
    );
    assert!(
        !names.contains(&"worker-1"),
        "factory_pane_configs must NOT auto-generate names when worker_names is non-empty"
    );
    assert!(
        !names.contains(&"worker-2"),
        "factory_pane_configs must NOT auto-generate names when worker_names is non-empty"
    );
}

// ── end cas-35fe ──────────────────────────────────────────────────────────────

// ── cas-5175: set_default_worker_spec → add_worker effort propagation ─────────

#[test]
fn add_worker_effort_propagates_to_pty_args() {
    // Verify that effort set on the Mux-wide default flows through
    // effective_worker_spec → build_add_worker_config → PtyConfig args.
    // Uses the config-only build_add_worker_config helper (no PTY spawn).
    use crate::spec::Effort;

    let _env_lock = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _effort_support = RestoreEnv::set("CAS_FACTORY_EFFORT_SUPPORTED", "1");
    let mut mux = Mux::new(24, 80);
    mux.set_default_worker_spec(crate::spec::WorkerSpec {
        name: None,
        cli: crate::harness::SupervisorCli::Claude,
        model: None,
        effort: Some(Effort::Low),
        config_dir: None,
        requester_config_dir: None,
        requester_secure_storage_dir: None,
    });

    let pty = mux.build_add_worker_config(
        "effort-worker",
        std::path::PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        None, // no explicit override → falls through to default
    );

    let effort_idx = pty
        .args
        .iter()
        .position(|a| a == "--effort")
        .expect("--effort must appear in PTY args when effort is set on the Mux default");
    let effort_val = pty
        .args
        .get(effort_idx + 1)
        .expect("--effort must be followed by a value");
    assert_eq!(
        effort_val, "low",
        "Effort::Low must reach PTY args as \"low\" via the default spec path"
    );
}

fn spawned_env<'a>(config: &'a crate::pty::PtyConfig, key: &str) -> Option<&'a str> {
    config
        .env
        .iter()
        .rev()
        .find_map(|(candidate, value)| (candidate == key).then_some(value.as_str()))
}

#[test]
fn explicit_config_dir_beats_requester_config_dir() {
    let mux = Mux::new(24, 80);
    let config = mux.build_add_worker_config(
        "claude-explicit",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(WorkerSpec {
            name: None,
            cli: crate::harness::SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: Some("~/.claude-explicit".to_string()),
            requester_config_dir: Some("~/.claude-supervisor".to_string()),
            requester_secure_storage_dir: None,
        }),
    );

    assert!(
        spawned_env(&config, "CLAUDE_CONFIG_DIR")
            .is_some_and(|dir| dir.ends_with("/.claude-explicit")),
        "explicit config dir must be tilde-expanded: {:?}",
        spawned_env(&config, "CLAUDE_CONFIG_DIR")
    );
    assert_eq!(
        spawned_env(&config, "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE"),
        Some("explicit")
    );
}

#[test]
fn requester_config_dir_applies_when_explicit_param_is_omitted() {
    let mux = Mux::new(24, 80);
    let config = mux.build_add_worker_config(
        "claude-supervisor",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(WorkerSpec {
            name: None,
            cli: crate::harness::SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: None,
            requester_config_dir: Some("~/.claude-supervisor".to_string()),
            requester_secure_storage_dir: None,
        }),
    );

    assert!(
        spawned_env(&config, "CLAUDE_CONFIG_DIR")
            .is_some_and(|dir| dir.ends_with("/.claude-supervisor")),
        "requester config dir must be tilde-expanded: {:?}",
        spawned_env(&config, "CLAUDE_CONFIG_DIR")
    );
    assert_eq!(
        spawned_env(&config, "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE"),
        Some("supervisor")
    );
}

#[test]
fn requester_secure_storage_selector_preserves_unset_empty_and_set() {
    for (secure_storage_dir, expected_env, expect_remove) in [
        (None, None, true),
        (Some(""), Some(""), false),
        (
            Some("~/.claude-keychain"),
            Some("~/.claude-keychain"),
            false,
        ),
    ] {
        let mux = Mux::new(24, 80);
        let config = mux.build_add_worker_config(
            "claude-secure-selector",
            PathBuf::from("/tmp/test"),
            None,
            "supervisor",
            None,
            Some(WorkerSpec {
                name: None,
                cli: crate::harness::SupervisorCli::Claude,
                model: None,
                effort: None,
                config_dir: None,
                requester_config_dir: Some("~/.claude-work".to_string()),
                requester_secure_storage_dir: secure_storage_dir.map(str::to_string),
            }),
        );

        let actual = spawned_env(&config, "CLAUDE_SECURESTORAGE_CONFIG_DIR");
        assert_eq!(
            actual.map(|value| value.rsplit('/').next().unwrap_or(value)),
            expected_env.map(|value| value.rsplit('/').next().unwrap_or(value))
        );
        assert_eq!(
            config
                .env_remove
                .iter()
                .any(|key| key == "CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            expect_remove
        );
    }
}

#[test]
fn omitted_config_dirs_leave_claude_environment_untouched() {
    let mux = Mux::new(24, 80);
    let config = mux.build_add_worker_config(
        "claude-inherit",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(WorkerSpec {
            name: None,
            cli: crate::harness::SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        }),
    );

    assert_eq!(spawned_env(&config, "CLAUDE_CONFIG_DIR"), None);
    assert_eq!(
        spawned_env(&config, "CAS_FACTORY_CLAUDE_CONFIG_DIR_SOURCE"),
        None
    );
}

#[test]
fn codex_ignores_resolved_claude_config_dir() {
    let mux = Mux::new(24, 80);
    let config = mux.build_add_worker_config(
        "codex-no-claude-dir",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(WorkerSpec {
            name: None,
            cli: crate::harness::SupervisorCli::Codex,
            model: None,
            effort: None,
            config_dir: Some("~/.claude-explicit".to_string()),
            requester_config_dir: Some("~/.claude-supervisor".to_string()),
            requester_secure_storage_dir: None,
        }),
    );

    assert_eq!(spawned_env(&config, "CLAUDE_CONFIG_DIR"), None);
}

// ── end cas-5175 ──────────────────────────────────────────────────────────────

#[test]
fn test_mux_new() {
    let mux = Mux::new(24, 80);
    assert_eq!(mux.size(), (24, 80));
    assert!(mux.focused().is_none());
}

#[test]
fn test_mux_add_pane() {
    let mut mux = Mux::new(24, 80);
    let pane = Pane::director("test", 24, 80).unwrap();
    mux.add_pane(pane);

    assert!(mux.get("test").is_some());
    assert_eq!(mux.focused_id(), Some("test"));
}

#[test]
fn test_mux_focus_navigation() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("pane1", 24, 40).unwrap());
    mux.add_pane(Pane::director("pane2", 24, 40).unwrap());

    assert_eq!(mux.focused_id(), Some("pane1"));

    mux.focus_next();
    assert_eq!(mux.focused_id(), Some("pane2"));

    mux.focus_next();
    assert_eq!(mux.focused_id(), Some("pane1")); // Wraps around

    mux.focus_prev();
    assert_eq!(mux.focused_id(), Some("pane2"));
}

#[test]
fn test_pane_count() {
    let mut mux = Mux::new(24, 80);
    assert_eq!(mux.pane_count(), 0);

    mux.add_pane(Pane::director("pane1", 24, 40).unwrap());
    assert_eq!(mux.pane_count(), 1);

    mux.add_pane(Pane::director("pane2", 24, 40).unwrap());
    assert_eq!(mux.pane_count(), 2);

    mux.remove_pane("pane1");
    assert_eq!(mux.pane_count(), 1);
}

#[test]
fn test_remove_pane_focus_transfer() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("pane1", 24, 40).unwrap());
    mux.add_pane(Pane::director("pane2", 24, 40).unwrap());

    // Focus is on pane1 (first added)
    assert_eq!(mux.focused_id(), Some("pane1"));

    // Remove focused pane, focus should transfer to next
    mux.remove_pane("pane1");
    assert_eq!(mux.focused_id(), Some("pane2"));
    assert_eq!(mux.pane_count(), 1);
}

// ── cas-c931: urgent interrupt-and-redirect by-name primitives ──────────────

#[test]
fn pane_bytes_received_present_for_known_pane_none_for_missing() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("w1", 24, 80).unwrap());

    // Known pane: returns Some (0 until output is drained).
    assert_eq!(mux.pane_bytes_received("w1"), Some(0));
    // Unknown pane: None.
    assert_eq!(mux.pane_bytes_received("ghost"), None);
}

/// Helper: a real PTY-backed pane (running `cat`) so write paths exercise an
/// actual backend. `Pane::director` uses `PaneBackend::None`, which rejects
/// writes — useless for the urgent-redirect write tests. Returns None if a PTY
/// can't be spawned in this environment so the test skips rather than flakes.
fn cat_pane(name: &str) -> Option<Pane> {
    Pane::shell(name, PathBuf::from("/tmp"), Some("cat"), 24, 80).ok()
}

/// Real PTY stand-in carrying the production Codex framing/submit delay.
fn codex_cat_pane(name: &str) -> Option<Pane> {
    let pty = crate::pty::Pty::spawn(
        name,
        crate::pty::PtyConfig {
            command: "cat".to_string(),
            args: vec![],
            cwd: Some(PathBuf::from("/tmp")),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .ok()?;
    Pane::with_pty(
        name,
        PaneKind::Supervisor,
        pty,
        24,
        80,
        SupervisorCli::Codex,
    )
    .ok()
}

struct SubmitFailWriter {
    writes: usize,
}

impl std::io::Write for SubmitFailWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        if self.writes == 1 {
            Ok(buf.len())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "deterministic delayed submit failure",
            ))
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn settled_pane_snapshot(
    mux: &mut Mux,
    pane_id: &str,
) -> cas_factory_protocol::TerminalSnapshot {
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    mux.poll_batch();
    mux.get_pane_snapshot(pane_id).expect("pane snapshot").0
}

#[cfg(target_os = "linux")]
fn proc_state_and_group(pid: u32) -> Option<(char, u32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1.trim_start();
    let mut fields = after_comm.split_whitespace();
    let state = fields.next()?.chars().next()?;
    fields.next()?;
    let pgid = fields.next()?.parse().ok()?;
    Some((state, pgid))
}

#[cfg(target_os = "linux")]
#[test]
fn kill_all_terminates_a_synthetic_long_lived_child_group() {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec!["-c".to_string(), "sleep 300 & wait".to_string()],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let Ok(pty) = crate::pty::Pty::spawn("synthetic-worker", config) else {
        return;
    };
    let pane = Pane::with_pty(
        "synthetic-worker",
        PaneKind::Worker,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .unwrap();
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);
    let pgid = mux
        .pane_process_group_id("synthetic-worker")
        .expect("PTY worker must expose its process group");

    let mut long_lived_child = None;
    for _ in 0..40 {
        long_lived_child = std::fs::read_dir("/proc").ok().and_then(|entries| {
            entries.flatten().find_map(|entry| {
                let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
                (pid != pgid
                    && proc_state_and_group(pid)
                        .is_some_and(|(state, group)| state != 'Z' && group == pgid))
                .then_some(pid)
            })
        });
        if long_lived_child.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let child_pid = long_lived_child.expect("synthetic shell must spawn its long-lived child");

    mux.kill_all();

    for _ in 0..40 {
        if proc_state_and_group(child_pid).is_none_or(|(state, _)| state == 'Z') {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("factory-exit kill_all left synthetic child {child_pid} alive");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn graceful_kill_worker_waits_before_escalating_a_term_ignoring_group() {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            "trap '' TERM HUP; sleep 300 & wait".to_string(),
        ],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let Ok(pty) = crate::pty::Pty::spawn("synthetic-grace-worker", config) else {
        return;
    };
    let pane = Pane::with_pty(
        "synthetic-grace-worker",
        PaneKind::Worker,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .unwrap();
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);
    let pgid = mux
        .pane_process_group_id("synthetic-grace-worker")
        .expect("PTY worker must expose its process group");

    // Give the shell time to install its TERM/HUP ignores before teardown.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let started = std::time::Instant::now();
    mux.kill_worker("synthetic-grace-worker", false)
        .await
        .unwrap();

    assert!(
        started.elapsed() >= std::time::Duration::from_millis(2_750),
        "TERM-ignoring group must receive a real grace period before escalation"
    );
    for _ in 0..40 {
        if proc_state_and_group(pgid).is_none_or(|(state, _)| state == 'Z') {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("graceful escalation left synthetic process group {pgid} alive");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn graceful_kill_worker_returns_when_the_group_honors_term() {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec!["-c".to_string(), "sleep 300 & wait".to_string()],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let Ok(pty) = crate::pty::Pty::spawn("synthetic-term-worker", config) else {
        return;
    };
    let pane = Pane::with_pty(
        "synthetic-term-worker",
        PaneKind::Worker,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .unwrap();
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let started = std::time::Instant::now();
    mux.kill_worker("synthetic-term-worker", false)
        .await
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "TERM-compliant groups should not consume the escalation grace window"
    );
}

#[tokio::test]
async fn break_turn_targets_pane_by_name_independent_of_focus() {
    let mut mux = Mux::new(24, 80);
    let (Some(w1), Some(w2)) = (cat_pane("w1"), cat_pane("w2")) else {
        return; // no PTY available — skip
    };
    mux.add_pane(w1);
    mux.add_pane(w2);

    // Focus is on w1, but we break w2 by name — must succeed regardless of focus.
    assert_eq!(mux.focused_id(), Some("w1"));
    assert!(mux.break_turn("w2").await.is_ok());

    // Unknown target is a pane-not-found error, not a panic.
    assert!(mux.break_turn("ghost").await.is_err());
}

#[tokio::test]
async fn interrupt_and_inject_errors_on_missing_pane() {
    let mut mux = Mux::new(24, 80);
    let res = mux
        .interrupt_and_inject("nope", "hi", std::time::Duration::from_millis(0))
        .await;
    assert!(
        res.is_err(),
        "missing pane must surface an error, not panic"
    );
}

#[tokio::test]
async fn interrupt_and_inject_breaks_then_injects_by_name() {
    let mut mux = Mux::new(24, 80);
    let Some(w1) = cat_pane("w1") else {
        return; // no PTY available — skip
    };
    mux.add_pane(w1);

    mux.deliver_user_input_to("w1", b"operator draft", UserInputKind::KeyStream)
        .await
        .expect("mark dirty through attached-client input");
    assert!(
        mux.panes
            .get("w1")
            .expect("pane exists")
            .is_composer_dirty()
    );

    // Zero settle keeps the test fast; the ordering (Esc then inject) is
    // enforced by interrupt_and_inject internally. The urgent path must bypass
    // non-urgent composer deferral and its Esc must clear the stale draft.
    let res = mux
        .interrupt_and_inject(
            "w1",
            "STOP and switch tasks",
            std::time::Duration::from_millis(0),
        )
        .await;
    assert!(res.is_ok(), "urgent redirect to a live pane must succeed");
    assert!(
        !mux.panes
            .get("w1")
            .expect("pane exists")
            .is_composer_dirty(),
        "urgent break-turn Esc clears the composer before redirect"
    );
}

#[tokio::test]
async fn urgent_queue_inject_preserves_draft_then_interrupts_after_submit_cas_eacc() {
    let mut mux = Mux::new(24, 80);
    let Some(w1) = codex_cat_pane("urgent-safe") else {
        return;
    };
    mux.add_pane(w1);

    mux.deliver_user_input_to(
        "urgent-safe",
        b"unfinished human sentence",
        UserInputKind::KeyStream,
    )
    .await
    .expect("type operator draft");
    let draft_before = settled_pane_snapshot(&mut mux, "urgent-safe").await;

    let deferred = mux
        .interrupt_and_inject_preserving_composer(
            "urgent-safe",
            "urgent lifecycle redirect",
            std::time::Duration::ZERO,
        )
        .await
        .expect("dirty composer is a durable deferral, not an error");
    assert_eq!(deferred, InjectOutcome::DeferredComposerDirty);
    assert!(
        mux.get("urgent-safe").unwrap().is_composer_dirty(),
        "urgent delivery must not send Esc and clear the human draft"
    );
    assert!(
        !mux.get("urgent-safe").unwrap().is_turn_in_flight(),
        "urgent delivery must not submit the human draft"
    );
    let draft_after = settled_pane_snapshot(&mut mux, "urgent-safe").await;
    assert_eq!(
        draft_after, draft_before,
        "urgent deferral must preserve the Codex supervisor's visible draft byte-for-byte"
    );

    mux.deliver_user_input_to("urgent-safe", b"\r", UserInputKind::KeyStream)
        .await
        .expect("human submits at the safe boundary");
    let delivered = mux
        .interrupt_and_inject_preserving_composer(
            "urgent-safe",
            "urgent lifecycle redirect",
            std::time::Duration::ZERO,
        )
        .await
        .expect("urgent row delivers after the safe boundary");
    assert_eq!(delivered, InjectOutcome::Delivered);
    assert!(
        mux.get("urgent-safe").unwrap().is_turn_in_flight(),
        "urgent semantics remain interrupt-and-inject once the composer is clean"
    );
}

// ── cas-1a4d: non-urgent injects defer around operator drafts ───────────────

#[tokio::test]
async fn nonurgent_inject_defers_until_composer_clears_cas_1a4d() {
    let mut mux = Mux::new(24, 80);
    let Some(pane) = cat_pane("operator-pane") else {
        return;
    };
    mux.add_pane(pane);

    mux.deliver_user_input_to(
        "operator-pane",
        b"half-written thought",
        UserInputKind::KeyStream,
    )
    .await
    .expect("type draft");
    let outcome = mux
        .inject("operator-pane", "non-urgent report")
        .await
        .expect("dirty inject should be deferred");
    assert_eq!(outcome, InjectOutcome::DeferredComposerDirty);
    assert!(
        !mux.panes
            .get("operator-pane")
            .expect("pane exists")
            .is_turn_in_flight(),
        "deferred report must not submit into the dirty composer"
    );

    mux.send_input_to("operator-pane", b"\x1b")
        .await
        .expect("standalone Esc clears draft");
    let outcome = mux.inject("operator-pane", "non-urgent report").await;

    assert_eq!(
        outcome.expect("durable queue retry after clear"),
        InjectOutcome::Delivered
    );
    assert!(
        mux.panes
            .get("operator-pane")
            .expect("pane exists")
            .is_turn_in_flight(),
        "retained report delivers as soon as the composer becomes clean"
    );
}

#[tokio::test]
async fn nonurgent_inject_never_expires_a_dirty_composer_cas_eacc() {
    let mut mux = Mux::new(24, 80);
    let Some(pane) = codex_cat_pane("timeout-pane") else {
        return;
    };
    mux.add_pane(pane);

    mux.deliver_user_input_to("timeout-pane", b"draft", UserInputKind::KeyStream)
        .await
        .expect("type draft");
    let draft_before = settled_pane_snapshot(&mut mux, "timeout-pane").await;
    let first = mux
        .inject("timeout-pane", "director lifecycle event one")
        .await
        .expect("dirty inject defers");
    let repeated = mux
        .inject("timeout-pane", "director lifecycle event two")
        .await
        .expect("repeated dirty inject also defers");

    assert_eq!(first, InjectOutcome::DeferredComposerDirty);
    assert_eq!(repeated, InjectOutcome::DeferredComposerDirty);
    assert!(
        mux.get("timeout-pane").unwrap().is_composer_dirty(),
        "elapsed time and repeated events must never make the draft writable"
    );
    assert!(
        !mux.get("timeout-pane").unwrap().is_turn_in_flight(),
        "neither event may append to or submit the unfinished human draft"
    );
    let draft_after = settled_pane_snapshot(&mut mux, "timeout-pane").await;
    assert_eq!(
        draft_after, draft_before,
        "repeated director events must leave the Codex supervisor draft byte-for-byte intact"
    );

    mux.deliver_user_input_to("timeout-pane", b"\r", UserInputKind::KeyStream)
        .await
        .expect("human submits at the safe boundary");
    assert_eq!(
        mux.inject("timeout-pane", "director lifecycle event one")
            .await
            .unwrap(),
        InjectOutcome::Delivered
    );
    assert_eq!(
        mux.inject("timeout-pane", "director lifecycle event two")
            .await
            .unwrap(),
        InjectOutcome::DeferredComposerDirty,
        "a second event stays separate while the first awaits its Codex submit CR"
    );
    tokio::time::sleep(std::time::Duration::from_millis(550)).await;
    assert_eq!(
        mux.inject("timeout-pane", "director lifecycle event two")
            .await
            .unwrap(),
        InjectOutcome::Delivered,
        "the next FIFO event releases exactly once after the prior submit boundary"
    );
}

#[tokio::test]
async fn failed_submit_keeps_later_injections_deferred_cas_eacc() {
    let mut mux = Mux::new(24, 80);
    let Some(pane) = codex_cat_pane("submit-failure") else {
        return;
    };
    pane.replace_pty_writer_for_test(Box::new(SubmitFailWriter { writes: 0 }))
        .await
        .expect("install deterministic writer");
    mux.add_pane(pane);

    assert_eq!(
        mux.inject("submit-failure", "director lifecycle event one")
            .await
            .expect("payload write succeeds before delayed CR failure"),
        InjectOutcome::Delivered
    );
    assert!(
        mux.get("submit-failure").unwrap().inject_submit_pending(),
        "the lane closes as soon as the payload is written"
    );

    tokio::time::sleep(std::time::Duration::from_millis(550)).await;
    assert!(
        mux.get("submit-failure").unwrap().inject_submit_pending(),
        "a failed submit CR must keep the pane unsafe instead of reopening the lane"
    );
    assert_eq!(
        mux.inject("submit-failure", "director lifecycle event two")
            .await
            .expect("unsafe pane causes durable deferral"),
        InjectOutcome::DeferredComposerDirty,
        "later non-urgent lifecycle payload must not coalesce with the unsubmitted first payload"
    );
    assert_eq!(
        mux.interrupt_and_inject_preserving_composer(
            "submit-failure",
            "urgent lifecycle event",
            std::time::Duration::ZERO,
        )
        .await
        .expect("urgent row also stays durable"),
        InjectOutcome::DeferredComposerDirty,
        "urgent semantics must not override the hard unsafe state after submit failure"
    );
}

#[tokio::test]
async fn clean_inject_surfaces_write_failure_cas_0b64() {
    let mut mux = Mux::new(24, 80);
    mux.add_pane(Pane::director("no-backend", 24, 80).expect("director pane"));

    // Pane::director has no writable PTY. State tracking happens before the
    // expected write error so this gives us a deterministic dirty target and
    // a deterministic deferred-delivery failure.
    assert!(
        mux.deliver_user_input_to("no-backend", b"draft", UserInputKind::KeyStream)
            .await
            .is_err()
    );
    assert_eq!(
        mux.inject("no-backend", "must stay durable")
            .await
            .expect("dirty target returns a deferred outcome"),
        InjectOutcome::DeferredComposerDirty
    );
    mux.get("no-backend")
        .unwrap()
        .observe_raw_client_input(b"\x1b");
    assert!(!mux.get("no-backend").unwrap().is_composer_dirty());
    assert!(
        mux.inject("no-backend", "must stay durable").await.is_err(),
        "a clean target must surface the failed real write so the durable queue retries"
    );
}

#[tokio::test]
async fn pane_without_client_input_injects_immediately_cas_1a4d() {
    let mut mux = Mux::new(24, 80);
    let Some(pane) = cat_pane("worker-pane") else {
        return;
    };
    mux.add_pane(pane);

    let outcome = mux
        .inject("worker-pane", "ordinary worker message")
        .await
        .expect("clean worker inject");

    assert_eq!(outcome, InjectOutcome::Delivered);
    assert!(
        mux.panes
            .get("worker-pane")
            .expect("pane exists")
            .is_turn_in_flight(),
        "a pane with no attached-client input keeps immediate delivery"
    );
}

#[tokio::test]
async fn deferred_payload_is_not_retained_across_teardown_or_respawn_cas_0b64() {
    let mut mux = Mux::new(24, 80);
    let Some(old_pane) = cat_pane("reused-name") else {
        return;
    };
    mux.add_pane(old_pane);
    mux.deliver_user_input_to("reused-name", b"operator draft", UserInputKind::KeyStream)
        .await
        .expect("mark old pane dirty");

    assert_eq!(
        mux.inject("reused-name", "old process payload")
            .await
            .expect("defer to durable queue"),
        InjectOutcome::DeferredComposerDirty
    );
    mux.remove_pane("reused-name");

    let Some(new_pane) = cat_pane("reused-name") else {
        return;
    };
    mux.add_pane(new_pane);
    mux.flush_deferred_injections().await;

    assert!(
        !mux.panes
            .get("reused-name")
            .expect("respawned pane exists")
            .is_turn_in_flight(),
        "old deferred payload must never be flushed into a same-name respawn"
    );
}

// ── cas-4208: post-break settle must observe real quiescence, not a blind
// sleep, for harnesses without a real textbox submit (Codex) ────────────────

/// A real PTY-backed pane running a script that emits a short burst of
/// output over ~600ms and then goes quiet (staying alive afterward so the
/// pane doesn't exit mid-test) — a stand-in for Codex's post-Esc
/// "Conversation interrupted" transition, which keeps redrawing for a while
/// before the composer is actually ready for fresh input again. A live
/// repro against the real `codex` binary (task cas-4208 notes) showed a
/// flat sleep can race exactly that transition and silently swallow the
/// follow-up submit; these tests pin the fix against a cheap, deterministic
/// stand-in rather than requiring the real binary. Returns `None` if `sh`
/// isn't available so the test skips rather than flakes.
fn ticking_pane(name: &str, harness: SupervisorCli) -> Option<Pane> {
    let config = crate::pty::PtyConfig {
        command: "sh".to_string(),
        args: vec![
            "-c".to_string(),
            "i=0; while [ $i -lt 6 ]; do i=$((i+1)); echo tick$i; sleep 0.1; done; sleep 5"
                .to_string(),
        ],
        cwd: Some(PathBuf::from("/tmp")),
        env: vec![],
        env_remove: vec![],
        rows: 24,
        cols: 80,
    };
    let pty = crate::pty::Pty::spawn(name, config).ok()?;
    Pane::with_pty(name, PaneKind::Worker, pty, 24, 80, harness).ok()
}

/// The multi-line payload shape from the actual cas-4208 incident (a blank
/// line plus indented follow-up lines) — pinned specifically so a future
/// single-line probe can't re-certify this path as working (the original
/// cas-8d76 live verification used exactly that too-simple single-line
/// probe, which is why this regression shipped unnoticed).
const MULTILINE_REDIRECT: &str =
    "STOP. Abandon that and instead run:\n\n  echo redirected\n\nThen reply INTERRUPTED-OK.";

#[tokio::test]
async fn interrupt_and_inject_waits_for_real_output_quiescence_on_codex() {
    let Some(pane) = ticking_pane("cas4208-codex-tick", SupervisorCli::Codex) else {
        return; // no PTY available — skip
    };
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    // Deliberately far shorter than the ~600ms output burst the pane emits:
    // proves the wait is driven by observed quiescence, not just this floor.
    let floor = std::time::Duration::from_millis(50);
    let start = std::time::Instant::now();
    let res = mux
        .interrupt_and_inject("cas4208-codex-tick", MULTILINE_REDIRECT, floor)
        .await;
    let elapsed = start.elapsed();

    assert!(res.is_ok(), "urgent redirect to a live pane must succeed");
    assert!(
        elapsed >= std::time::Duration::from_millis(500),
        "Codex (supports_textbox_submit=false) must wait for the pane's own \
         output burst to actually go quiet (~600ms here) rather than injecting \
         right after the {floor:?} floor — got elapsed={elapsed:?}. This is the \
         cas-4208 regression: a flat sleep races a still-transitioning child \
         and can leave the correction sitting unsent in the composer."
    );
}

#[tokio::test]
async fn interrupt_and_inject_keeps_flat_floor_for_textbox_submit_harnesses() {
    // Same ticking child, tagged Claude this time — must NOT wait for the
    // burst. Regression guard for cas-4208 AC4: Claude/Grok behavior (a flat
    // sleep already recovers reliably per that task's live control-group
    // evidence) must stay exactly as before.
    let Some(pane) = ticking_pane("cas4208-claude-tick", SupervisorCli::Claude) else {
        return; // no PTY available — skip
    };
    let mut mux = Mux::new(24, 80);
    mux.add_pane(pane);

    let floor = std::time::Duration::from_millis(50);
    let start = std::time::Instant::now();
    let res = mux
        .interrupt_and_inject("cas4208-claude-tick", MULTILINE_REDIRECT, floor)
        .await;
    let elapsed = start.elapsed();

    assert!(res.is_ok(), "urgent redirect to a live pane must succeed");
    assert!(
        elapsed < std::time::Duration::from_millis(400),
        "Claude/Grok (supports_textbox_submit=true) must keep the old flat-floor \
         behavior unchanged — got elapsed={elapsed:?}, which suggests the new \
         quiescence poll leaked into a harness that must not use it"
    );
}
