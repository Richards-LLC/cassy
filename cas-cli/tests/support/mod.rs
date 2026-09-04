//! Hermetic support for integration tests that spawn the `cas` binary.

// This shared module is compiled separately for each integration-test binary,
// so helpers used by only some consumers otherwise produce false positives.
#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::Connection;
use tempfile::TempDir;

/// The `cas init` watchdog budget in seconds. Forwarded through the sandbox's
/// `CAS_*` scrub so a batch runner can tell its whole process tree it is a slow
/// environment (cas-c0411); see [`CasSandbox::configure_command`].
pub const INIT_TIMEOUT_SECS_ENV: &str = "CAS_INIT_TIMEOUT_SECS";

/// Resolve an archive-consumable `cas` binary without trusting a producer path
/// baked into an integration-test executable.
pub fn cas_binary() -> PathBuf {
    cas::test_paths::binary("cas", option_env!("CARGO_BIN_EXE_cas").map(PathBuf::from))
}

pub fn workspace_root() -> PathBuf {
    cas::test_paths::workspace_root()
}

/// A temporary CAS project whose subprocesses cannot inherit a live CAS store.
///
/// Use [`CasSandbox::command`] for every `cas` subprocess. It removes all
/// inherited `CAS_*` variables (including variables added in the future) and
/// pins every project/store resolver to this sandbox.
pub struct CasSandbox {
    temp_dir: TempDir,
    cas_root: PathBuf,
    home_dir: PathBuf,
    xdg_config_home: PathBuf,
}

impl CasSandbox {
    /// Create and initialize an isolated CAS project.
    pub fn new() -> Self {
        Self::new_with_initializer_environment(None, None)
    }

    /// Create a sandbox while modeling hostile inherited host directories on
    /// the initializer process. `configure_command` must overwrite both.
    #[allow(dead_code)] // shared support is compiled into consumers without this regression
    pub fn new_with_host_environment(host_home: &Path, host_xdg_config_home: &Path) -> Self {
        Self::new_with_initializer_environment(Some(host_home), Some(host_xdg_config_home))
    }

    fn new_with_initializer_environment(
        host_home: Option<&Path>,
        host_xdg_config_home: Option<&Path>,
    ) -> Self {
        let temp_dir = TempDir::new().expect("create CAS sandbox");
        let cas_root = temp_dir.path().join(".cas");
        let home_dir = temp_dir.path().join("home");
        let xdg_config_home = temp_dir.path().join("xdg-config");
        std::fs::create_dir_all(&home_dir).expect("create sandbox HOME");
        std::fs::create_dir_all(&xdg_config_home).expect("create sandbox XDG_CONFIG_HOME");
        let sandbox = Self {
            temp_dir,
            cas_root,
            home_dir,
            xdg_config_home,
        };

        let mut cmd = Command::new(cas_binary());
        if let Some(host_home) = host_home {
            cmd.env("HOME", host_home);
        }
        if let Some(host_xdg_config_home) = host_xdg_config_home {
            cmd.env("XDG_CONFIG_HOME", host_xdg_config_home);
        }
        sandbox.configure_command(&mut cmd);
        let output = cmd
            .args(["init", "--yes"])
            .output()
            .expect("run cas init in sandbox");
        assert!(
            output.status.success(),
            "cas init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            sandbox.cas_root.join("cas.db").is_file(),
            "cas init did not create sandbox database"
        );

        sandbox
    }

    /// Build a `cas` command anchored to this sandbox.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(cas_binary());
        self.configure_command(&mut cmd);
        cmd
    }

    /// Make an existing command hermetic and anchor it to this sandbox.
    ///
    /// Removing keys by prefix avoids the recurring failure mode where a new
    /// CAS environment variable is added but a hand-maintained test scrub list
    /// is not updated. `HOME`, `XDG_CONFIG_HOME`, and `CLAUDE_PROJECT_DIR` are
    /// authoritative sandbox values because spawned production binaries use
    /// them for the host known-repo registry, user config, and serve-root
    /// resolution respectively.
    pub fn configure_command<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        // Captured before the scrub below: the protected-store tripwire is
        // itself a `CAS_*` variable, and dropping it would disarm the very
        // guard that proves this sandbox works (cas-78c8). A value already set
        // on `cmd` wins over the ambient one so a caller can arm the tripwire
        // for a single subprocess without mutating process-global environment
        // in a parallel test run.
        let protected_dbs = cmd
            .get_envs()
            .find_map(|(key, value)| {
                (key == cas_store::shared_db::PROTECTED_DBS_ENV)
                    .then_some(value)
                    .flatten()
            })
            .map(|value| value.to_os_string())
            .or_else(|| std::env::var_os(cas_store::shared_db::PROTECTED_DBS_ENV));

        // Same carve-out, different reason (cas-c0411): this one is a pure
        // wall-clock budget for the `cas init` watchdog. Every sandbox runs
        // `cas init` in its constructor, so on a saturated host — the v3.15.1
        // release gate, with three isolation re-runs and six idle `cas serve`
        // daemons in flight — that child can hit the 300 s default and abort,
        // failing a release on timing. The batch runner raises the budget by
        // exporting it; the scrub must not be what silently drops it. It cannot
        // redirect any store or root, so forwarding it costs no hermeticity.
        let init_timeout_secs = cmd
            .get_envs()
            .find_map(|(key, value)| (key == INIT_TIMEOUT_SECS_ENV).then_some(value).flatten())
            .map(|value| value.to_os_string())
            .or_else(|| std::env::var_os(INIT_TIMEOUT_SECS_ENV));

        let cas_keys: Vec<OsString> = std::env::vars_os()
            .map(|(key, _)| key)
            .chain(cmd.get_envs().map(|(key, _)| key.to_os_string()))
            .filter(|key| key.to_string_lossy().starts_with("CAS_"))
            .collect();
        for key in cas_keys {
            cmd.env_remove(key);
        }

        if let Some(protected_dbs) = protected_dbs {
            cmd.env(cas_store::shared_db::PROTECTED_DBS_ENV, protected_dbs);
        }
        if let Some(init_timeout_secs) = init_timeout_secs {
            cmd.env(INIT_TIMEOUT_SECS_ENV, init_timeout_secs);
        }

        cmd.current_dir(self.path())
            .env("HOME", &self.home_dir)
            .env("XDG_CONFIG_HOME", &self.xdg_config_home)
            .env_remove("CLAUDE_PROJECT_DIR")
            .env("CAS_ROOT", &self.cas_root)
            .env("CAS_DIR", &self.cas_root)
            .env("CLAUDE_PROJECT_DIR", self.path())
            .env("CAS_SKIP_FACTORY_TOOLING", "1")
    }

    pub fn path(&self) -> &Path {
        self.temp_dir.path()
    }

    pub fn cas_root(&self) -> &Path {
        &self.cas_root
    }

    pub fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    pub fn xdg_config_home(&self) -> &Path {
        &self.xdg_config_home
    }

    pub fn task_count(&self) -> i64 {
        let conn = Connection::open(self.cas_root.join("cas.db"))
            .expect("open sandbox database for task count");
        conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .expect("count sandbox tasks")
    }
}

/// Assert that a command's effective environment is pinned to `sandbox`.
///
/// This is intentionally based on `Command::get_envs`, so tests can seed a
/// command with hostile/live-store values and prove the sandbox overwrites
/// them without mutating process-global environment in a parallel test run.
pub fn assert_command_is_sandboxed(cmd: &Command, sandbox: &CasSandbox) {
    fn env_value<'a>(cmd: &'a Command, name: &str) -> Option<&'a OsStr> {
        cmd.get_envs()
            .find_map(|(key, value)| (key == name).then_some(value).flatten())
    }

    assert_eq!(
        env_value(cmd, "CAS_ROOT"),
        Some(sandbox.cas_root().as_os_str())
    );
    assert_eq!(
        env_value(cmd, "CAS_DIR"),
        Some(sandbox.cas_root().as_os_str())
    );
    assert_eq!(
        env_value(cmd, "CLAUDE_PROJECT_DIR"),
        Some(sandbox.path().as_os_str())
    );
    assert_eq!(env_value(cmd, "HOME"), Some(sandbox.home_dir().as_os_str()));
    assert_eq!(
        env_value(cmd, "XDG_CONFIG_HOME"),
        Some(sandbox.xdg_config_home().as_os_str())
    );
}
