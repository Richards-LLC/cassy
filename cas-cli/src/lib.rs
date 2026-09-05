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
pub mod agent_id;
pub(crate) mod ai_enrichment;
pub(crate) mod ambient_recall;
pub mod async_runtime;
mod bounded_process;
pub mod bridge;
pub mod builtins;
pub mod cli;
pub mod cloud;
pub mod capability;
pub mod config;
pub mod consolidation;
pub mod daemon;
pub mod duplicate_check;
pub mod error;
pub mod extraction;
pub mod factory_auth_health;
pub mod factory_context_reset;
pub mod factory_isolation;
pub mod factory_preflight;
pub mod factory_supervisor_overlap;
pub mod factory_target_cache;
pub mod fs_space;
pub mod gh_graphql;
pub mod git_log;
pub mod harness_policy;
pub mod history;
pub mod hooks;
pub mod hub;
pub mod hybrid_search;
pub(crate) mod internal_llm;
pub mod knowledge;
pub mod logging;
pub mod memory_migration;
pub mod migration;
pub mod notifications;
pub mod orchestration;
pub mod otel;
pub mod opencode_preflight;
pub(crate) mod prompt_revalidation;
pub mod retrieval_eval;
pub mod retrieval_parity;
pub mod sentry;
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

/// Shared test-only utilities. Kept in one place so cross-module statics
/// (like the HOME env-var mutex used by known_repos + discovery tests)
/// refer to a single instance; otherwise each test module's own static
/// would race against the other's.
#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) use crate::test_env_guard::TestEnvGuard;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, OnceLock};

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
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
        let guard = TestEnvGuard::temp_home();
        assert_eq!(guard.home(), guard.home().canonicalize().unwrap());
        assert_eq!(
            std::env::var_os("HOME").as_deref(),
            Some(guard.home().as_os_str())
        );
    }

    #[test]
    fn test_env_guard_scrubs_ambient_cas_and_factory_environment() {
        let guard = TestEnvGuard::temp_home();
        assert!(
            std::env::vars_os().all(|(key, _)| {
                let Some(key) = key.to_str() else {
                    return true;
                };
                // CAS_ROOT is the one CAS_* the guard sets itself, to pin the
                // project root inside the temp HOME (cas-4ccc). It is checked
                // below rather than exempted silently.
                //
                // CAS_INIT_TIMEOUT_SECS is the one ambient CAS_* the guard
                // deliberately keeps: a pure wall-clock budget for the `cas
                // init` watchdog, which a saturated batch host raises for its
                // whole process tree (cas-c0411). The exemption is stated once,
                // in test_env_guard::is_scrubbed_ambient_env_key, and covered by
                // its own tests there.
                (!key.starts_with("CAS_")
                    || key == "CAS_ROOT"
                    || key == crate::test_env_guard::AMBIENT_INIT_TIMEOUT_SECS)
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
