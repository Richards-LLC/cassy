//! Shared process-state isolation for unit and integration tests.
//!
//! This file is compiled once into each test process. Process-global
//! environment and cwd state therefore share one lock within the only scope
//! where they can race, without exposing test machinery from the shipping
//! library API.

#![allow(dead_code)] // each integration-test binary uses only part of the shared API

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

thread_local! {
    static TEST_ENV_GUARD_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

struct TestEnvGuardNesting;

impl TestEnvGuardNesting {
    fn enter() -> Self {
        TEST_ENV_GUARD_ACTIVE.with(|active| {
            assert!(
                !active.replace(true),
                "nested TestEnvGuard on the same thread; reuse the existing guard instead"
            );
        });
        Self
    }
}

impl Drop for TestEnvGuardNesting {
    fn drop(&mut self) {
        TEST_ENV_GUARD_ACTIVE.with(|active| active.set(false));
    }
}

/// The one ambient `CAS_*` variable a guarded test keeps: the wall-clock budget
/// for the `cas init` watchdog.
///
/// It carries no store, root, or account identity — only how long a child `cas
/// init` may run before it aborts itself. A batch runner raises it to say "this
/// host is saturated"; the v3.15.1 release gate lost a run because a test's
/// child `cas init` hit the 300 s default while three isolation re-runs and six
/// idle daemons competed for the box (cas-c0411). Scrubbing it would put those
/// children back on the default the runner just overrode.
pub(crate) const AMBIENT_INIT_TIMEOUT_SECS: &str = "CAS_INIT_TIMEOUT_SECS";

/// Whether a guarded test process must drop this ambient variable.
///
/// Pure so the exemption is stated once and tested without mutating
/// process-global environment.
pub(crate) fn is_scrubbed_ambient_env_key(key: &str) -> bool {
    if key == AMBIENT_INIT_TIMEOUT_SECS {
        return false;
    }
    key.starts_with("CAS_")
        || matches!(
            key,
            "CLAUDE_CONFIG_DIR" | "CLAUDE_SECURESTORAGE_CONFIG_DIR" | "CODEX_HOME" | "GROK_HOME"
        )
}

pub(crate) fn test_env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Canonical process-wide environment fixture for tests.
///
/// The guard owns the one shared lock, captures every variable before its
/// first mutation, restores all values on drop (including during unwind), and
/// retains any temporary HOME for the full mutation lifetime. Tests should
/// never call `set_var`/`remove_var` directly.
pub(crate) struct TestEnvGuard {
    _lock: MutexGuard<'static, ()>,
    _nesting: TestEnvGuardNesting,
    saved: Vec<(OsString, Option<OsString>)>,
    temp_home: Option<TempDir>,
    temp_home_path: Option<std::path::PathBuf>,
    saved_cwd: Option<std::path::PathBuf>,
}

impl TestEnvGuard {
    pub(crate) fn new() -> Self {
        // Mark this thread before taking the process-wide lock. A nested
        // constructor can then fail immediately instead of blocking forever
        // on the non-reentrant mutex already held by this thread.
        let nesting = TestEnvGuardNesting::enter();
        let mut guard = Self {
            _lock: test_env_lock(),
            _nesting: nesting,
            saved: Vec::new(),
            temp_home: None,
            temp_home_path: None,
            saved_cwd: None,
        };
        guard.scrub_ambient_cas_environment();
        guard
    }

    pub(crate) fn temp_home() -> Self {
        let mut guard = Self::new();
        let temp = TempDir::new().expect("temp HOME");
        // TempDir may expose a symlinked platform spelling (notably macOS
        // `/var` -> `/private/var`). Keep test HOME and every path derived
        // from it in the same canonical namespace production validation uses.
        let path = temp.path().canonicalize().expect("canonical temp HOME");
        guard.temp_home = Some(temp);
        guard.temp_home_path = Some(path.clone());
        guard.set("HOME", &path);
        guard.pin_cas_root_under_home();
        guard
    }

    /// Pin the project root inside the temp HOME so **ancestor** project
    /// config cannot leak into a test that believes it is hermetic.
    ///
    /// HOME and `XDG_CONFIG_HOME` only redirect *user-level* lookups. Project
    /// config resolves through `find_cas_root`, which walks up from the current
    /// directory and maps a git worktree onto its main repository's `.cas`.
    /// Every factory worktree lives under `<repo>/.cas/worktrees/<name>`, so
    /// without this the host checkout's `.cas/proxy.toml` was reachable from
    /// tests that set only HOME: on 2026-09-03 three proxy tests began
    /// panicking inside reqwest because the loader handed them a real http
    /// server (cas-4ccc). `CAS_ROOT` is the loader's own documented override
    /// and wins ahead of both the worktree mapping and the walk.
    ///
    /// The directory is created because `find_cas_root` only honours an
    /// override that exists; an empty one is exactly the hermetic world these
    /// tests assume — a project store with nothing configured in it.
    fn pin_cas_root_under_home(&mut self) {
        let root = self.home().join(".cas");
        std::fs::create_dir_all(&root).expect("temp CAS_ROOT");
        self.set("CAS_ROOT", &root);
    }

    pub(crate) fn with_vars(vars: &[(&str, &str)]) -> Self {
        let mut guard = Self::new();
        for (key, value) in vars {
            guard.set(*key, *value);
        }
        guard
    }

    pub(crate) fn with_optional_vars(vars: &[(&str, Option<&str>)]) -> Self {
        let mut guard = Self::new();
        for (key, value) in vars {
            match value {
                Some(value) => guard.set(*key, *value),
                None => guard.remove(*key),
            }
        }
        guard
    }

    pub(crate) fn run_with_temp_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let guard = Self::temp_home();
        f(guard.home())
    }

    pub(crate) fn home(&self) -> &Path {
        self.temp_home_path
            .as_ref()
            .expect("TestEnvGuard has no temp HOME")
            .as_path()
    }

    pub(crate) fn set(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
        let key = key.as_ref();
        self.capture(key);
        // SAFETY: the guard holds the process-wide test environment lock until
        // after Drop restores every captured variable.
        unsafe { std::env::set_var(key, value) };
    }

    pub(crate) fn remove(&mut self, key: impl AsRef<OsStr>) {
        let key = key.as_ref();
        self.capture(key);
        // SAFETY: see `set`.
        unsafe { std::env::remove_var(key) };
    }

    pub(crate) fn set_current_dir(&mut self, path: impl AsRef<Path>) {
        if self.saved_cwd.is_none() {
            self.saved_cwd = Some(std::env::current_dir().expect("current test directory"));
        }
        std::env::set_current_dir(path).expect("set test current directory");
    }

    fn capture(&mut self, key: &OsStr) {
        if !self.saved.iter().any(|(saved, _)| saved == key) {
            self.saved.push((key.to_os_string(), std::env::var_os(key)));
        }
    }

    /// Keep test fixtures independent from the factory process that launched
    /// the test binary. Every `CAS_*` variable is ambient Cassy state, and the
    /// account-home variables are the non-CAS part of the factory contract.
    /// Tests that need one of these values set it explicitly after constructing
    /// the guard; Drop restores the caller's environment.
    fn scrub_ambient_cas_environment(&mut self) {
        let keys = std::env::vars_os()
            .map(|(key, _)| key)
            .filter(|key| key.to_str().is_some_and(is_scrubbed_ambient_env_key))
            .collect::<Vec<_>>();
        for key in keys {
            self.remove(key);
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

#[cfg(test)]
mod ambient_scrub_tests {
    use super::*;

    #[test]
    fn ambient_cas_and_factory_state_is_scrubbed() {
        for key in [
            "CAS_ROOT",
            "CAS_DIR",
            "CAS_CLOUD_TOKEN",
            "CAS_SOME_FUTURE_VARIABLE",
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
            "CODEX_HOME",
            "GROK_HOME",
        ] {
            assert!(
                is_scrubbed_ambient_env_key(key),
                "{key} is ambient Cassy/factory state and must not reach a fixture"
            );
        }
    }

    #[test]
    fn the_init_watchdog_budget_survives_because_it_is_only_a_clock() {
        // cas-c0411: scrubbing this put a test's child `cas init` back on the
        // 300s default that a saturated release gate had just raised, and the
        // gate failed on wall clock rather than on anything about the tree.
        assert!(!is_scrubbed_ambient_env_key(AMBIENT_INIT_TIMEOUT_SECS));
    }

    #[test]
    fn the_exemption_does_not_extend_to_neighbouring_names() {
        // A prefix or suffix match would quietly widen the carve-out to
        // variables that do carry identity.
        for key in [
            "CAS_INIT_TIMEOUT",
            "CAS_INIT_TIMEOUT_SECS_EXTRA",
            "CAS_INIT_NO_TIMEOUT",
            "CAS_INIT_ROOT",
        ] {
            assert!(
                is_scrubbed_ambient_env_key(key),
                "{key} must still be scrubbed; only the exact budget variable is exempt"
            );
        }
    }

    #[test]
    fn unrelated_host_environment_is_left_alone() {
        for key in ["PATH", "HOME", "TMPDIR", "CARGO_TARGET_DIR", "CASSETTE"] {
            assert!(
                !is_scrubbed_ambient_env_key(key),
                "{key} is not Cassy state and must survive"
            );
        }
    }
}
