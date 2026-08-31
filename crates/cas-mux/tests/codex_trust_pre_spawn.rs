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
use std::sync::{Arc, Barrier};

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

    fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
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
    #[cfg(unix)]
    codex_does_not_launch_when_trust_read_back_cannot_verify().await;
    #[cfg(unix)]
    concurrent_codex_workers_launch_only_after_every_trust_entry_is_read_back().await;
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

/// A failed trust transaction must prevent `Pty::spawn`: Codex only reads this
/// config at process start, so executing it after an unparseable/read-back
/// failure would reintroduce the permanent interactive-prompt park.
#[cfg(unix)]
async fn codex_does_not_launch_when_trust_read_back_cannot_verify() {
    use std::os::unix::fs::PermissionsExt;

    let (_home, bin, config) = isolate_codex_env("unverified");
    let workdir = Scratch::new("unverified-cwd");
    std::fs::write(&config, "this is [not valid TOML\n").unwrap();
    let codex = bin.join("codex");
    std::fs::write(
        &codex,
        "#!/bin/sh\n: > \"$PWD/.mock-codex-should-not-launch\"\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cas = bin.join("cas");
    std::fs::write(&cas, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&cas, std::fs::Permissions::from_mode(0o755)).unwrap();

    let result = Pane::worker(
        "unverified",
        workdir.path().to_path_buf(),
        None,
        "supervisor",
        SupervisorCli::Codex,
        SupervisorCli::Codex,
        None,
        None,
        None,
        None,
        None,
        24,
        80,
        None,
        None,
        None,
    );
    assert!(result.is_err(), "unverified trust must refuse the spawn");
    assert!(
        !workdir.join(".mock-codex-should-not-launch").exists(),
        "Codex executable must not run before trust read-back verification"
    );
}

/// cas-3603 (GH #237): every concurrent Codex spawn must see its own durable
/// trust entry before its executable begins. The mock `codex` checks its config
/// at process start and drops a per-cwd receipt, so this asserts the actual
/// `Pane::worker` happens-before boundary rather than only the write helper.
#[cfg(unix)]
async fn concurrent_codex_workers_launch_only_after_every_trust_entry_is_read_back() {
    use std::os::unix::fs::PermissionsExt;

    const WORKERS: usize = 8;
    let (_home, bin, config) = isolate_codex_env("concurrent");
    let workdirs: Vec<Scratch> = (0..WORKERS)
        .map(|index| Scratch::new(&format!("concurrent-cwd-{index}")))
        .collect();

    // Keep PATH private to this test. `cas` only satisfies the PTY preflight;
    // the mock Codex never invokes it. The Codex mock itself verifies the
    // project table before emitting its launch receipt.
    let codex = bin.join("codex");
    std::fs::write(
        &codex,
        r#"#!/bin/sh
expected="[projects.\"$(pwd)\"]"
found=0
while IFS= read -r line; do
    if [ "$line" = "$expected" ]; then
        found=1
    elif [ "$found" = 1 ] && [ "$line" = 'trust_level = "trusted"' ]; then
        : > "$PWD/.mock-codex-launched-after-trust"
        exit 0
    fi
done < "$CODEX_HOME/config.toml"
: > "$PWD/.mock-codex-launched-before-trust"
exit 23
"#,
    )
    .unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cas = bin.join("cas");
    std::fs::write(&cas, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&cas, std::fs::Permissions::from_mode(0o755)).unwrap();
    let nice = bin.join("nice");
    std::fs::write(
        &nice,
        "#!/bin/sh\nwhile [ \"$1\" != \"codex\" ]; do shift; done\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&nice, std::fs::Permissions::from_mode(0o755)).unwrap();

    let start = Arc::new(Barrier::new(WORKERS));
    std::thread::scope(|scope| {
        let mut spawns = Vec::with_capacity(WORKERS);
        for (index, scratch) in workdirs.iter().enumerate() {
            let start = Arc::clone(&start);
            let cwd = scratch.path().to_path_buf();
            spawns.push(scope.spawn(move || {
                start.wait();
                let pane = Pane::worker(
                    &format!("concurrent-{index}"),
                    cwd.clone(),
                    None,
                    "supervisor",
                    SupervisorCli::Codex,
                    SupervisorCli::Codex,
                    None,
                    None,
                    None,
                    None,
                    None,
                    24,
                    80,
                    None,
                    None,
                    None,
                )
                .map_err(|error| error.to_string())?;

                let success = cwd.join(".mock-codex-launched-after-trust");
                let failure = cwd.join(".mock-codex-launched-before-trust");
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !success.exists() && !failure.exists() && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                drop(pane);
                if failure.exists() {
                    return Err(format!(
                        "mock Codex launched before trust was present for {}",
                        cwd.display()
                    ));
                }
                if !success.exists() {
                    return Err(format!("mock Codex did not launch for {}", cwd.display()));
                }
                Ok::<_, String>(())
            }));
        }
        for spawn in spawns {
            spawn.join().unwrap().unwrap();
        }
    });

    let contents = std::fs::read_to_string(&config).unwrap();
    for scratch in &workdirs {
        assert!(
            trust_entry_present(&config, scratch.path()),
            "config lost concurrent trust entry for {}; contents: {contents}",
            scratch.path().display(),
        );
    }
}
