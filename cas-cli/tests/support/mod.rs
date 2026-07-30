//! Hermetic support for integration tests that spawn the `cas` binary.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::Connection;
use tempfile::TempDir;

/// A temporary CAS project whose subprocesses cannot inherit a live CAS store.
///
/// Use [`CasSandbox::command`] for every `cas` subprocess. It removes all
/// inherited `CAS_*` variables (including variables added in the future) and
/// pins every project/store resolver to this sandbox.
pub struct CasSandbox {
    temp_dir: TempDir,
    cas_root: PathBuf,
}

impl CasSandbox {
    /// Create and initialize an isolated CAS project.
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("create CAS sandbox");
        let cas_root = temp_dir.path().join(".cas");
        let sandbox = Self { temp_dir, cas_root };

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cas"));
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
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cas"));
        self.configure_command(&mut cmd);
        cmd
    }

    /// Make an existing command hermetic and anchor it to this sandbox.
    ///
    /// Removing keys by prefix avoids the recurring failure mode where a new
    /// CAS environment variable is added but a hand-maintained test scrub list
    /// is not updated. `CLAUDE_PROJECT_DIR` is also pinned because `cas serve`
    /// intentionally resolves it before `CAS_ROOT`.
    pub fn configure_command<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        let cas_keys: Vec<OsString> = std::env::vars_os()
            .map(|(key, _)| key)
            .chain(cmd.get_envs().map(|(key, _)| key.to_os_string()))
            .filter(|key| key.to_string_lossy().starts_with("CAS_"))
            .collect();
        for key in cas_keys {
            cmd.env_remove(key);
        }

        cmd.current_dir(self.path())
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
}
