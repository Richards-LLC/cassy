//! CAS - Coding Agent System
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
pub mod async_runtime;
pub mod bridge;
pub mod builtins;
pub mod cli;
pub mod cloud;
pub mod config;
pub mod consolidation;
pub mod daemon;
pub mod duplicate_check;
pub mod error;
pub mod extraction;
pub mod harness_policy;
pub mod hooks;
pub mod hybrid_search;
pub mod logging;
pub mod migration;
pub mod notifications;
pub mod orchestration;
pub mod otel;
pub mod rules;
pub mod sentry;
pub mod store;
pub mod sync;
pub mod telemetry;
pub mod tracing;
pub mod ui;
pub mod worktree;

/// Shared test-only utilities. Kept in one place so cross-module statics
/// (like the HOME env-var mutex used by known_repos + discovery tests)
/// refer to a single instance; otherwise each test module's own static
/// would race against the other's.
#[cfg(test)]
pub(crate) mod test_support {
    use std::ffi::{OsStr, OsString};
    use std::path::Path;
    use tempfile::TempDir;

    /// Canonical process-wide environment fixture for lib tests.
    ///
    /// The guard owns the one shared lock, captures every variable before its
    /// first mutation, restores all values on drop (including during unwind),
    /// and retains any temporary HOME for the full mutation lifetime. Tests
    /// should never call `set_var`/`remove_var` directly.
    pub struct TestEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(OsString, Option<OsString>)>,
        temp_home: Option<TempDir>,
        saved_cwd: Option<std::path::PathBuf>,
    }

    impl TestEnvGuard {
        pub fn new() -> Self {
            Self {
                _lock: crate::hooks::test_env_lock(),
                saved: Vec::new(),
                temp_home: None,
                saved_cwd: None,
            }
        }

        pub fn temp_home() -> Self {
            let mut guard = Self::new();
            let temp = TempDir::new().expect("temp HOME");
            let path = temp.path().to_path_buf();
            guard.temp_home = Some(temp);
            guard.set("HOME", path);
            guard
        }

        pub fn with_vars(vars: &[(&str, &str)]) -> Self {
            let mut guard = Self::new();
            for (key, value) in vars {
                guard.set(*key, *value);
            }
            guard
        }

        pub fn with_optional_vars(vars: &[(&str, Option<&str>)]) -> Self {
            let mut guard = Self::new();
            for (key, value) in vars {
                match value {
                    Some(value) => guard.set(*key, *value),
                    None => guard.remove(*key),
                }
            }
            guard
        }

        pub fn run_with_temp_home<T>(f: impl FnOnce(&Path) -> T) -> T {
            let guard = Self::temp_home();
            f(guard.home())
        }

        pub fn home(&self) -> &Path {
            self.temp_home
                .as_ref()
                .expect("TestEnvGuard has no temp HOME")
                .path()
        }

        pub fn set(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
            let key = key.as_ref();
            self.capture(key);
            // SAFETY: the guard holds the process-wide test environment lock
            // until after Drop restores every captured variable.
            unsafe { std::env::set_var(key, value) };
        }

        pub fn remove(&mut self, key: impl AsRef<OsStr>) {
            let key = key.as_ref();
            self.capture(key);
            // SAFETY: see `set`.
            unsafe { std::env::remove_var(key) };
        }

        pub fn set_current_dir(&mut self, path: impl AsRef<Path>) {
            if self.saved_cwd.is_none() {
                self.saved_cwd = Some(std::env::current_dir().expect("current test directory"));
            }
            std::env::set_current_dir(path).expect("set test current directory");
        }

        fn capture(&mut self, key: &OsStr) {
            if !self.saved.iter().any(|(saved, _)| saved == key) {
                self.saved
                    .push((key.to_os_string(), std::env::var_os(key)));
            }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            // Restore in reverse mutation order while `_lock` is still held.
            for (key, value) in self.saved.iter().rev() {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
            if let Some(cwd) = &self.saved_cwd {
                let _ = std::env::set_current_dir(cwd);
            }
        }
    }

    #[test]
    fn home_path_env_mutation_is_centralized_in_test_env_guard() {
        fn visit(dir: &Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read source directory") {
                let path = entry.expect("source entry").path();
                if path.is_dir() {
                    visit(&path, hits);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let source = std::fs::read_to_string(&path).expect("read Rust source");
                    for (line_index, line) in source.lines().enumerate() {
                        let mutates_sensitive_var = ["HOME", "XDG_CONFIG_HOME", "PATH"]
                            .iter()
                            .any(|key| {
                                line.contains(&format!("std::env::set_var(\"{key}\""))
                                    || line.contains(&format!("std::env::remove_var(\"{key}\""))
                            });
                        if mutates_sensitive_var {
                            hits.push(format!("{}:{}", path.display(), line_index + 1));
                        }
                    }
                }
            }
        }

        let mut hits = Vec::new();
        visit(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").as_path(), &mut hits);
        assert!(
            hits.is_empty(),
            "HOME/XDG_CONFIG_HOME/PATH mutations must go through TestEnvGuard: {hits:?}"
        );
    }
}

// Re-export cas-types as types for backward compatibility
pub use cas_types as types;

// MCP server (behind feature flag)
#[cfg(feature = "mcp-server")]
pub mod mcp;

// Re-exports for convenience
pub use error::{CasError, Result};
pub use types::{
    Agent, ChangeType, CommitLink, Entry, EntryType, Event, FileChange, Prompt, Rule, RuleStatus,
    Session, SessionOutcome, Skill, Spec, Task, TaskStatus, Verification, Worktree, WorktreeStatus,
};
