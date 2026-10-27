//! Cassy - Coding Agent System
//!
//! A library for AI agents to build persistent memory across sessions.
//!
//! This crate provides unified task tracking, memory management, rules, and skills
//! for AI coding agents.

// Build-time enforcement of the panic=unwind invariant the MCP tool
// dispatch panic catcher depends on. The `not(test)` exemption exists
// because Rust forces panic=unwind when compiling the lib under
// `cargo test --lib`; the guard still fires for `cargo build`,
// `cargo check`, and integration-test dependency compilations (where
// cfg(test) is false on the lib).
#[cfg(all(not(test), panic = "abort"))]
compile_error!(
    "cas requires `panic = \"unwind\"` (see EPIC cas-c351). The MCP dispatch \
     panic catcher at cas-cli/src/mcp/tools/service/panic_catch.rs depends \
     on stack unwinding; `panic = \"abort\"` disables it and makes `cas serve` \
     crash on the first handler panic with no server-side trace. Remove \
     `panic = \"abort\"` from the build profile."
);

// Core modules
pub mod account_registry;
pub mod accounts;
pub mod agent_id;
pub(crate) mod ai_enrichment;
pub(crate) mod ambient_recall;
pub mod artifacts;
pub mod async_runtime;
mod bounded_process;
pub mod bridge;
pub mod builtins;
pub mod cli;
pub mod cloud;
pub mod capability;
pub mod config;
pub mod consolidation;
pub mod coordination;
pub mod daemon;
pub mod duplicate_check;
pub mod error;
pub mod extraction;
mod light_lane;
#[doc(hidden)]
pub use light_lane::run_detached_worker;
pub mod factory_auth_health;
pub mod factory_hook_canary;
pub(crate) mod factory_build_guard;
pub(crate) mod factory_build_memory;
pub(crate) mod factory_worker_check;
pub(crate) mod factory_capacity_wait;
pub mod factory_context_reset;
pub mod factory_isolation;
pub mod factory_permission_relay;
pub mod factory_preflight;
pub mod factory_daemon_health;
pub(crate) mod factory_binary_skew;
pub mod factory_session_scope;
pub mod factory_supervisor_overlap;
pub(crate) mod supervisor_notices;
pub(crate) mod supervisor_continuity;
pub(crate) mod supervisor_branch_recovery;
pub(crate) mod supervisor_message_wakes;
pub(crate) mod factory_scratch;
pub mod factory_review_scratch;
pub mod factory_target_cache;
pub mod fs_space;
pub mod gh_graphql;
pub mod github_issue_attach;
pub(crate) mod github_repo;
pub(crate) mod git_evidence;
pub mod git_log;
pub mod harness_policy;
pub mod history;
pub mod task_ownership;
pub mod task_simulators;
pub mod db_branch;
pub mod deliverable;
pub mod hooks;
pub mod issue_reconcile;
pub mod hub;
pub mod hybrid_search;
pub(crate) mod internal_llm;
pub mod knowledge;
pub mod logging;
pub mod maintenance_jobs;
pub mod memory_migration;
pub mod migration;
pub mod notifications;
pub mod orchestration;
pub mod otel;
pub(crate) mod ops;
pub mod qa_evidence;
pub mod opencode_preflight;
pub(crate) mod prompt_revalidation;
pub(crate) mod task_assignment;
pub mod qa_pass;
pub mod qa_registry;
pub mod qa_impact;
pub mod qa_seed;
pub mod qa_post_release;
pub mod qa_traffic;
/// cas-e8d1 (GH #1233): open work the record says is done.
pub(crate) mod backlog_hygiene;
pub(crate) mod qa_batch;
pub(crate) mod review_body;
pub mod retrieval_eval;
pub mod retrieval_parity;
pub mod sentry;
pub(crate) mod server_signals;
mod skill_validation;
pub mod store;
pub mod sync;
pub mod telemetry;
pub mod temp_hygiene;
pub mod test_paths;
pub mod tracing;
pub mod ui;
pub mod worktree;

#[cfg(test)]
mod test_env_guard;

#[cfg(test)]
#[path = "../../crates/cas-core/src/test_child.rs"]
pub(crate) mod test_child;

/// Shared test-only utilities. Kept in one place so cross-module statics
/// (like the HOME env-var mutex used by known_repos + discovery tests)
/// refer to a single instance; otherwise each test module's own static
/// would race against the other's.
#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) use crate::test_env_guard::TestEnvGuard;
    #[cfg(unix)]
    pub(crate) use crate::test_env_guard::fake_cas_on_path;
    pub(crate) use crate::test_paths::private_hub_tempdir;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Pin real styled bytes independently of crossterm's memoized NO_COLOR.
    /// Declare after `TestEnvGuard` and keep both alive through rendering, so
    /// color-policy changes are serialized and restored before the env lock.
    #[must_use]
    pub(crate) struct StyledColorGuard(bool);

    impl StyledColorGuard {
        pub(crate) fn new(_env: &TestEnvGuard) -> Self {
            use crossterm::style::{Colored, force_color_output};
            let enabled = !Colored::ansi_color_disabled_memoized();
            force_color_output(true);
            Self(enabled)
        }
    }

    impl Drop for StyledColorGuard {
        fn drop(&mut self) {
            crossterm::style::force_color_output(self.0);
        }
    }

    #[test]
    fn styled_color_guard_restores_cached_policy() {
        use crossterm::style::{Color, SetForegroundColor, force_color_output};
        let env = TestEnvGuard::new();
        let _restore = StyledColorGuard::new(&env);
        for enabled in [false, true] {
            force_color_output(enabled);
            for unwind in [false, true] {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _colors = StyledColorGuard::new(&env);
                    assert_eq!(
                        SetForegroundColor(Color::Green).to_string(),
                        "\x1b[38;5;10m"
                    );
                    if unwind {
                        panic!("exercise styled-color guard unwinding");
                    }
                }));
                assert_eq!(result.is_err(), unwind);
                assert_eq!(
                    SetForegroundColor(Color::Green).to_string(),
                    if enabled { "\x1b[38;5;10m" } else { "\x1b[m" },
                    "cached policy not restored (enabled={enabled}, unwind={unwind})"
                );
            }
        }
    }

    /// Cargo's in-process test harness runs otherwise isolated Tantivy
    /// fixtures concurrently. Keep the tests that intentionally exercise
    /// short-lived disk writers out of each other's lock lifecycle; nextest
    /// process isolation does not provide this protection for cargo runs.
    pub(crate) fn disk_index_test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn nested_test_env_guard_panics_instead_of_deadlocking() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let mut child = Command::new(std::env::current_exe().expect("current test binary"))
            .args([
                "--exact",
                "test_support::nested_test_env_guard_panics_with_clear_message",
                "--ignored",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn nested TestEnvGuard regression test");
        let deadline = Instant::now() + Duration::from_secs(5);

        let status = loop {
            if let Some(status) = child.try_wait().expect("poll nested guard test") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("nested TestEnvGuard hung instead of failing loudly");
            }
            std::thread::sleep(Duration::from_millis(10));
        };

        assert!(
            status.success(),
            "nested TestEnvGuard did not panic with the expected diagnostic"
        );
        let output = child.wait_with_output().expect("capture nested guard child");
        crate::test_child::assert_passed(
            &String::from_utf8_lossy(&output.stdout),
            "test_support::nested_test_env_guard_panics_with_clear_message",
        );
    }

    #[test]
    #[ignore = "subprocess helper for nested_test_env_guard_panics_instead_of_deadlocking"]
    #[should_panic(
        expected = "nested TestEnvGuard on the same thread; reuse the existing guard instead"
    )]
    fn nested_test_env_guard_panics_with_clear_message() {
        let _outer = TestEnvGuard::new();
        let _inner = TestEnvGuard::new();
    }

    #[test]
    fn temp_home_uses_canonical_path_namespace() {
        let machine = tempfile::tempdir().unwrap();
        let xdg_keys = ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"];
        let _ambient =
            xdg_keys.map(|key| crate::test_env_guard::AmbientEnvRestore::set(key, machine.path()));
        let poisoned_config = machine.path().join("code-mode-mcp/config.toml");
        std::fs::create_dir_all(poisoned_config.parent().unwrap()).unwrap();
        let poison = "[servers.machine]\ntransport = \"stdio\"\ncommand = \"must-not-run\"\n";
        std::fs::write(&poisoned_config, poison).unwrap();

        let assert_private_home = |home: &Path| {
            assert_eq!(home, home.canonicalize().unwrap());
            assert_eq!(std::env::var_os("HOME").as_deref(), Some(home.as_os_str()));
            #[cfg(feature = "mcp-proxy")]
            {
                let project = crate::cli::integrate::proxy::proxy_config_path();
                let config = cmcp_core::config::Config::load_merged(project.as_deref()).unwrap();
                assert!(
                    config.servers.is_empty(),
                    "machine proxy config leaked into temp HOME"
                );
                let private_config = home.join(".config/code-mode-mcp/config.toml");
                std::fs::create_dir_all(private_config.parent().unwrap()).unwrap();
                std::fs::write(
                    &private_config,
                    "[servers.private]\ntransport = \"stdio\"\ncommand = \"fixture-only\"\n",
                )
                .unwrap();
                let config = cmcp_core::config::Config::load_merged(project.as_deref()).unwrap();
                assert_eq!(
                    config
                        .servers
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    ["private"]
                );
            }
            for (key, directory) in xdg_keys
                .into_iter()
                .zip([".config", ".local/share", ".cache"])
            {
                assert_eq!(
                    std::env::var_os(key).as_deref(),
                    Some(home.join(directory).as_os_str()),
                    "{key}"
                );
            }
        };
        let guard = TestEnvGuard::temp_home();
        assert_private_home(guard.home());
        drop(guard);
        TestEnvGuard::run_with_temp_home(assert_private_home);
        for key in xdg_keys {
            assert_eq!(
                std::env::var_os(key).as_deref(),
                Some(machine.path().as_os_str()),
                "restore {key}"
            );
        }
        assert_eq!(std::fs::read_to_string(poisoned_config).unwrap(), poison);
    }

    #[test]
    fn test_env_guard_scrubs_ambient_cas_and_factory_environment() {
        let ambient_build_guard =
            crate::test_env_guard::AmbientEnvRestore::set("CAS_FACTORY_BUILD_GUARD", "inherit");
        let provider_keys = [
            "VIKTOR_API_KEY",
            "NEON_API_KEY",
            "VERCEL_TOKEN",
            "OPENAI_API_KEY",
            "VOYAGE_API_KEY",
            "EXA_API_KEY",
            "GITHUB_TOKEN",
            "VIOLET_SLACK_TOKEN_CASSY_PROXY",
        ];
        let _ambient_credentials = provider_keys
            .map(|key| crate::test_env_guard::AmbientEnvRestore::set(key, "fixture-credential"));
        {
            let mut guard = TestEnvGuard::new();
            for key in provider_keys {
                assert!(
                    std::env::var_os(key).is_none(),
                    "ambient provider credential {key} leaked"
                );
            }
            guard.set("VIKTOR_API_KEY", "explicit-fixture");
            assert_eq!(
                std::env::var("VIKTOR_API_KEY").as_deref(),
                Ok("explicit-fixture")
            );
        }
        let guard = TestEnvGuard::temp_home();
        for key in provider_keys {
            assert!(
                std::env::var_os(key).is_none(),
                "ambient provider credential {key} leaked"
            );
        }
        assert_eq!(
            std::env::var("CAS_FACTORY_BUILD_GUARD").as_deref(),
            Ok("off"),
            "TestEnvGuard must replace, not inherit, the ambient build-guard value"
        );
        assert!(
            std::env::vars_os().all(|(key, _)| {
                let Some(key) = key.to_str() else {
                    return true;
                };
                // CAS_ROOT is the one CAS_* the guard sets itself, to pin the
                // project root inside the temp HOME (cas-4ccc). It is checked
                // below rather than exempted silently.
                //
                // The same policy preserves the init watchdog budget and the
                // protected-host tripwires. Neither redirects a test store.
                // Keep the exemption list in test_env_guard rather than
                // silently maintaining a second list here.
                (!crate::test_env_guard::is_scrubbed_ambient_env_key(key)
                    || key == "CAS_ROOT"
                    || key == "CAS_FACTORY_BUILD_GUARD"
                    || key == "CAS_GH_BIN")
                    && !matches!(
                        key,
                        "CLAUDE_CONFIG_DIR"
                            | "CLAUDE_SECURESTORAGE_CONFIG_DIR"
                            | "CODEX_HOME"
                            | "GROK_HOME"
                    )
            }),
            "TestEnvGuard must not inherit ambient CAS/factory environment"
        );

        // The stronger half of the same property: whatever CAS_ROOT holds must
        // be the guard's own temp path, never the host's. A leaked ambient
        // value would point outside the temp HOME and fail here.
        let pinned = std::env::var_os("CAS_ROOT").expect("temp_home pins CAS_ROOT");
        assert_eq!(
            std::path::Path::new(&pinned),
            guard.home().join(".cas"),
            "CAS_ROOT must be the hermetic root inside the temp HOME"
        );
        let gh = std::env::var_os("CAS_GH_BIN").expect("guard must pin gh away from the host");
        assert!(
            std::process::Command::new(gh)
                .arg("--version")
                .output()
                .is_err(),
            "the default gh fixture must fail before any process or network call"
        );
        drop(guard);
        for key in provider_keys {
            assert_eq!(
                std::env::var(key).as_deref(),
                Ok("fixture-credential"),
                "restore {key}"
            );
        }
        assert_eq!(
            std::env::var("CAS_FACTORY_BUILD_GUARD").as_deref(),
            Ok("inherit"),
            "ambient build-guard value must be restored after the fixture"
        );
        drop(ambient_build_guard);
    }

    #[test]
    fn lib_test_process_env_mutation_is_isolated() {
        fn visit(dir: &Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read source directory") {
                let path = entry.expect("source entry").path();
                if path.is_dir() {
                    visit(&path, hits);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(&path).expect("read Rust source");
                    for (line_index, line) in source.lines().enumerate() {
                        let mutates_sensitive_var =
                            ["HOME", "XDG_CONFIG_HOME", "PATH"].iter().any(|key| {
                                line.contains(&format!("std::env::set_var(\"{key}\""))
                                    || line.contains(&format!("std::env::remove_var(\"{key}\""))
                            });
                        let mutates_path_through_guard =
                            line.contains(".set(\"PATH\"") || line.contains(".remove(\"PATH\"");
                        if mutates_sensitive_var || mutates_path_through_guard {
                            hits.push(format!("{}:{}", path.display(), line_index + 1));
                        }
                    }
                }
            }
        }

        let mut hits = Vec::new();
        visit(
            crate::test_paths::crate_root().join("src").as_path(),
            &mut hits,
        );
        assert!(
            hits.is_empty(),
            "HOME/XDG_CONFIG_HOME mutations must use TestEnvGuard, and lib tests must not mutate \
             process-global PATH; use per-Command environment instead: {hits:?}"
        );
    }
}

// Re-export cas-types as types for backward compatibility
pub use cas_types as types;

// MCP server (always available; factory agents depend on `cas serve`)
pub mod mcp;

// Re-exports for convenience
pub use error::{CasError, Result};
pub use types::{
    Agent, ChangeType, CommitLink, Entry, EntryType, Event, FileChange, Prompt, Rule, RuleStatus,
    Session, SessionOutcome, Skill, Spec, Task, TaskStatus, Verification, Worktree, WorktreeStatus,
};
