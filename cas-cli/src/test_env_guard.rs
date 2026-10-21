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

/// An ambient `CAS_*` variable a guarded test keeps: the wall-clock budget
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
    // These are safety controls, not store selectors. Dropping the tripwires
    // silently lets a fixture reach the operator's host registry (cas-88a0).
    if matches!(
        key,
        AMBIENT_INIT_TIMEOUT_SECS | "CAS_TEST_PROTECTED_DBS" | "CAS_TEST_PROTECTED_HOME"
    ) {
        return false;
    }
    // cas-c7c7: real-cloud run controls for the distributed tests. Only test
    // code reads them, and scrubbing them made guarded tests fall back to the
    // in-process fake silently.
    if key.starts_with("CAS_TEST_CLOUD_") {
        return false;
    }
    key.starts_with("CAS_")
        // Provider credentials are ambient operator state too (cas-6f45).
        // Include account-suffixed token names used by Violet and its legacy
        // aliases, without keeping a separate list of provider brands.
        || key.ends_with("_API_KEY")
        || key.ends_with("_TOKEN")
        || key.contains("_TOKEN_")
        || key.ends_with("_VERCEL_BYPASS")
        || matches!(
            key,
            "CLAUDE_CONFIG_DIR"
                | "CLAUDE_SECURESTORAGE_CONFIG_DIR"
                | "CODEX_HOME"
                | "GROK_HOME"
                // Repository-local variables exported by Git hooks (cas-ea62).
                // The regression compares this list with --local-env-vars.
                | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
                | "GIT_CONFIG"
                | "GIT_CONFIG_PARAMETERS"
                | "GIT_CONFIG_COUNT"
                | "GIT_OBJECT_DIRECTORY"
                | "GIT_DIR"
                | "GIT_WORK_TREE"
                | "GIT_IMPLICIT_WORK_TREE"
                | "GIT_GRAFT_FILE"
                | "GIT_INDEX_FILE"
                | "GIT_NO_REPLACE_OBJECTS"
                | "GIT_REPLACE_REF_BASE"
                | "GIT_PREFIX"
                | "GIT_SHALLOW_FILE"
                | "GIT_COMMON_DIR"
        )
}

pub(crate) fn test_env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Install an ambient value before constructing a `TestEnvGuard`, restoring it
/// after the fixture drops. This keeps invariant tests from mutating process
/// environment directly while proving that guard-owned overrides replace,
/// rather than inherit, ambient values.
pub(crate) struct AmbientEnvRestore {
    saved: Vec<(OsString, Option<OsString>)>,
}

impl AmbientEnvRestore {
    pub(crate) fn set(key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        let key = key.as_ref();
        let _lock = test_env_lock();
        let saved = vec![(key.to_os_string(), std::env::var_os(key))];
        // SAFETY: the mutation is serialized by the shared test environment
        // lock and the original value is restored on Drop.
        unsafe { std::env::set_var(key, value) };
        drop(_lock);
        Self { saved }
    }
}

impl Drop for AmbientEnvRestore {
    fn drop(&mut self) {
        let _lock = test_env_lock();
        for (key, value) in self.saved.iter().rev() {
            // SAFETY: the lock prevents concurrent test environment changes.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
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
        guard.scrub_ambient_environment();
        // Factory spawn tests must not inherit the runner's live load. The
        // production probe remains enabled unless an explicit test fixture
        // opts into this override; guard unit tests use evaluate() with
        // injected snapshots and therefore still exercise refusal behavior.
        guard.set("CAS_FACTORY_BUILD_GUARD", "off");
        // A path beneath this executable cannot resolve to another program.
        // Fail before spawning host gh, even with inherited credentials.
        // GitHub fixtures must explicitly install their own CAS_GH_BIN.
        guard.set(
            "CAS_GH_BIN",
            std::env::current_exe()
                .expect("test executable")
                .join("disabled-gh"),
        );
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
        // XDG lookups take precedence over HOME. Redirect all three scopes so
        // config, credential stores and caches cannot escape this fixture.
        for (key, directory) in [
            ("XDG_CONFIG_HOME", ".config"),
            ("XDG_DATA_HOME", ".local/share"),
            ("XDG_CACHE_HOME", ".cache"),
        ] {
            guard.set(key, path.join(directory));
        }
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
    /// the test binary. `CAS_*` variables other than the watchdog budget and
    /// protected-store tripwires are ambient Cassy state; account-home
    /// variables and provider keys/tokens are the non-CAS part of the factory
    /// contract. Repository-local Git variables would redirect fixture writes
    /// into the caller's checkout.
    /// Tests that need one of these values set it explicitly after constructing
    /// the guard; Drop restores the caller's environment.
    fn scrub_ambient_environment(&mut self) {
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
    fn repository_local_git_environment_is_scrubbed_cas_ea62() {
        // Keep the in-process guard in sync with the shell suite boundary's
        // `git rev-parse --local-env-vars`, without spawning Git per guard.
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--local-env-vars"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let names = String::from_utf8(output.stdout).unwrap();
        assert!(names.lines().any(|key| key == "GIT_DIR"));
        for key in names.lines() {
            assert!(
                is_scrubbed_ambient_env_key(key),
                "{key} must not redirect a fixture"
            );
        }
        // User/tool configuration is not repository-local hook state.
        for key in ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM", "GIT_AUTHOR_NAME"] {
            assert!(!is_scrubbed_ambient_env_key(key), "{key}");
        }
    }

    #[test]
    fn bare_fixture_under_hook_environment_preserves_parent_config_cas_ea62() {
        let _guard = TestEnvGuard::new();
        let parent = TempDir::new().unwrap();
        let initialized = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(parent.path())
            .output()
            .unwrap();
        assert!(initialized.status.success());
        let config = parent.path().join(".git/config");
        let before = std::fs::read(&config).unwrap();
        let fixture = TempDir::new().unwrap();
        // Inject hook state into a separate test process, avoiding global
        // environment mutations in this parallel test runner.
        // This shared file is also compiled below integration-test modules.
        // libtest names omit the crate segment but retain those module names.
        let module = module_path!()
            .split_once("::")
            .map_or(module_path!(), |(_, module)| module);
        let child_test = format!("{module}::hook_environment_bare_fixture_child_cas_ea62");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", &child_test])
            .env("GIT_DIR", parent.path().join(".git"))
            .env("GIT_COMMON_DIR", parent.path().join(".git"))
            .env("GIT_WORK_TREE", parent.path())
            .env("GIT_INDEX_FILE", parent.path().join(".git/index"))
            .current_dir(fixture.path())
            .output()
            .unwrap();
        assert_eq!(
            std::fs::read(config).unwrap(),
            before,
            "parent config changed"
        );
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(
            String::from_utf8_lossy(&child.stdout).contains("1 passed; 0 failed"),
            "child filter must run its test: {}",
            String::from_utf8_lossy(&child.stdout)
        );
        assert!(fixture.path().join("HEAD").is_file());
    }

    #[test]
    #[ignore = "subprocess helper for the hook environment regression"]
    fn hook_environment_bare_fixture_child_cas_ea62() {
        let guard = TestEnvGuard::new();
        for key in [
            "GIT_DIR",
            "GIT_COMMON_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
        ] {
            assert!(std::env::var_os(key).is_none(), "leaked {key}");
        }
        let initialized = std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .output()
            .unwrap();
        assert!(
            initialized.status.success(),
            "{}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        drop(guard);
        assert!(
            std::env::var_os("GIT_DIR").is_some(),
            "guard must restore hook state"
        );
    }

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
    fn real_cloud_run_controls_survive_because_only_tests_read_them() {
        // cas-c7c7: a guarded distributed test must still see that this run
        // targets the real cloud, or it silently falls back to the fake.
        for key in [
            "CAS_TEST_CLOUD_TOKEN_FILE",
            "CAS_TEST_CLOUD_ENDPOINT",
            "CAS_TEST_CLOUD_TRANSCRIPT_DIR",
        ] {
            assert!(!is_scrubbed_ambient_env_key(key), "{key}");
        }
        // The product's own cloud selectors stay scrubbed.
        for key in [
            "CAS_CLOUD_ENDPOINT",
            "CAS_CLOUD_TOKEN",
            "CAS_CLOUD_TOKEN_FILE",
        ] {
            assert!(is_scrubbed_ambient_env_key(key), "{key}");
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
    fn protected_host_tripwires_survive_the_ambient_scrub() {
        for key in ["CAS_TEST_PROTECTED_DBS", "CAS_TEST_PROTECTED_HOME"] {
            assert!(!is_scrubbed_ambient_env_key(key), "{key} must remain armed");
        }
        for key in [
            "CAS_TEST_PROTECTED_DBS_EXTRA",
            "CAS_TEST_PROTECTED_HOME_ROOT",
        ] {
            assert!(is_scrubbed_ambient_env_key(key), "{key} is not a tripwire");
        }
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

/// Private no-op `cas` for preflight/hook fixtures. Keep it alive with the
/// child and pass `path()` through Command.env. Before calling Pty::spawn,
/// re-execute the selected test in that child: the PTY preflight reads ambient
/// PATH, not PtyConfig.env. No process environment is changed; tests of Cassy
/// behavior use the built binary.
#[cfg(unix)]
pub(crate) struct FakeCasOnPath {
    bin: TempDir,
    path: OsString,
}

#[cfg(unix)]
impl FakeCasOnPath {
    pub(crate) fn path(&self) -> &OsStr {
        &self.path
    }
    pub(crate) fn bin(&self) -> &Path {
        self.bin.path()
    }
}

#[cfg(unix)]
pub(crate) fn fake_cas_on_path() -> FakeCasOnPath {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().expect("private cas stand-in directory");
    let cas = bin.path().join("cas");
    std::fs::write(&cas, "#!/bin/sh\nexit 0\n").expect("write private cas stand-in");
    std::fs::set_permissions(&cas, std::fs::Permissions::from_mode(0o700))
        .expect("make private cas stand-in executable");
    let previous = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(bin.path().to_path_buf()).chain(std::env::split_paths(&previous)),
    )
    .expect("join fixture PATH");
    FakeCasOnPath { bin, path }
}
