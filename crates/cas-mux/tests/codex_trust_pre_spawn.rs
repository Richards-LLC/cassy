//! cas-28a49 (GH #97): a Codex pane must pre-trust its working directory
//! *before* the process is launched.
//!
//! Codex CLI parks on its interactive "do you trust the files in this folder?"
//! screen when the directory is absent from `[projects]` in
//! `$CODEX_HOME/config.toml` — before rendering, before writing a session file,
//! and before starting `cas serve`. The worker then never registers and the
//! spawn dies at `stage=register` with a generic timeout.
//!
//! This test drives the real `Pane::worker` / `Pane::supervisor` entry points
//! with `codex` removed from `PATH`, so the PTY launch fails immediately. The
//! trust write happens before the launch attempt, so the config file must
//! already carry the entry even though the spawn failed. That ordering is the
//! whole fix: a later write would lose the race with Codex's own startup read.
//!
//! Lives in its own integration-test binary because it mutates process-global
//! environment (`CODEX_HOME`, `PATH`).

use cas_mux::{Pane, SupervisorCli};
use std::path::{Path, PathBuf};

/// Self-cleaning scratch dir — these tests would otherwise leave a handful of
/// `/tmp` directories behind on every run.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "cas-28a49-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn trust_entry_present(config: &Path, workdir: &Path) -> bool {
    let contents = std::fs::read_to_string(config).unwrap_or_default();
    let key = format!("[projects.\"{}\"]", workdir.to_string_lossy());
    contents.contains(&key) && contents.contains("trust_level = \"trusted\"")
}

/// Point Codex at a private config dir and guarantee the launch fails fast.
///
/// The real `~/.codex/config.toml` is never touched: `CODEX_HOME` is resolved
/// in-process at call time and is set before every phase. `PATH` is restored by
/// [`EnvGuard`] so nothing added to this binary later inherits an empty `PATH`.
fn isolate_codex_env(tag: &str) -> (Scratch, Scratch, PathBuf) {
    let home = Scratch::new(&format!("{tag}-home"));
    let empty_path = Scratch::new(&format!("{tag}-bin"));
    unsafe {
        std::env::set_var("CODEX_HOME", home.path());
        std::env::set_var("PATH", empty_path.path());
    }
    let config = home.path().join("config.toml");
    (home, empty_path, config)
}

/// Restores `PATH` / `CODEX_HOME` when the test ends.
struct EnvGuard {
    path: Option<std::ffi::OsString>,
    codex_home: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn capture() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            codex_home: std::env::var_os("CODEX_HOME"),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.path {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match &self.codex_home {
                Some(v) => std::env::set_var("CODEX_HOME", v),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
    }
}

/// All three phases run in one test: they mutate process-global env
/// (`CODEX_HOME`, `PATH`), which cannot be done safely from parallel threads.
#[tokio::test]
async fn codex_panes_pre_trust_workdir_and_other_harnesses_do_not() {
    let _env = EnvGuard::capture();
    codex_worker_pane_pre_trusts_workdir_before_launch().await;
    codex_supervisor_pane_pre_trusts_workdir_before_launch().await;
    claude_worker_pane_does_not_touch_codex_config().await;
}

async fn codex_worker_pane_pre_trusts_workdir_before_launch() {
    let (_home, _bin, config) = isolate_codex_env("worker");
    let workdir_dir = Scratch::new("worker-cwd");
    let workdir = workdir_dir.path().to_path_buf();

    // The launch itself is expected to fail (no `codex` on PATH); the pre-trust
    // step runs first and is what we assert on.
    let _ = Pane::worker(
        "w1",
        workdir.clone(),
        None,
        "supervisor",
        SupervisorCli::Codex,
        SupervisorCli::Codex,
        None,
        None,
        None,
        None,
        24,
        80,
        None,
        None,
        // active_workers: this test drives the real spawn path only to assert
        // the pre-trust side effect, so the fleet size is irrelevant here.
        None,
    );

    assert!(
        trust_entry_present(&config, &workdir),
        "spawning a codex worker must append a trusted [projects.\"<cwd>\"] entry to {} \
         before launching the CLI; contents were: {:?}",
        config.display(),
        std::fs::read_to_string(&config).ok()
    );
}

async fn codex_supervisor_pane_pre_trusts_workdir_before_launch() {
    let (_home, _bin, config) = isolate_codex_env("supervisor");
    let workdir_dir = Scratch::new("supervisor-cwd");
    let workdir = workdir_dir.path().to_path_buf();

    let _ = Pane::supervisor(
        "s1",
        workdir.clone(),
        None,
        24,
        80,
        SupervisorCli::Codex,
        SupervisorCli::Codex,
        &[],
        None,
        None,
        None,
        None,
    );

    assert!(
        trust_entry_present(&config, &workdir),
        "spawning a codex supervisor must append a trusted [projects.\"<cwd>\"] entry to {}",
        config.display()
    );
}

async fn claude_worker_pane_does_not_touch_codex_config() {
    let (_home, _bin, config) = isolate_codex_env("claude");
    let workdir_dir = Scratch::new("claude-cwd");
    let workdir = workdir_dir.path().to_path_buf();

    let _ = Pane::worker(
        "w1",
        workdir.clone(),
        None,
        "supervisor",
        SupervisorCli::Claude,
        SupervisorCli::Claude,
        None,
        None,
        None,
        None,
        24,
        80,
        None,
        None,
        // active_workers: this test drives the real spawn path only to assert
        // the pre-trust side effect, so the fleet size is irrelevant here.
        None,
    );

    assert!(
        !config.exists(),
        "a non-Codex worker must never create or modify the Codex config"
    );
}
