//! Codex availability probe (cas-a487 / cas-7199).
//!
//! Codex became the default worker harness in v2.27.0 (cas-fbac). Before
//! that, a machine without the `codex` binary (or without a completed
//! ChatGPT login) only hit this gap if someone opted in explicitly via
//! `harness = codex`; now it's a first-run experience. [`codex_available`]
//! gives [`crate::spec_resolver::apply_codex_fallback`] a single yes/no
//! signal to decide whether a spec requesting Codex can actually be
//! honored, so the fallback decision happens BEFORE a worker is queued for
//! spawn — not as a raw PTY spawn failure after worktree/agent-registration
//! setup has already run for the wrong harness.
//!
//! Two independent checks, both required:
//! 1. The `codex` binary resolves on `PATH` and runs (`codex --version`).
//! 2. `~/.codex/auth.json` exists — Codex's ChatGPT-login credential file.
//!
//! Per the cas-e9e9 decision, deliberately does NOT also accept
//! `OPENAI_API_KEY` as an alternate auth signal: the fallback must be
//! deterministic given the same host state, not depend on which
//! credential path the operator happened to configure. A binary that
//! resolves via an API key but has no ChatGPT login still counts as
//! "unavailable" here.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Upper bound on how long the `codex --version` probe waits for the child
/// to exit before treating it as unavailable. Short by design — this runs
/// on every spec resolution that requests Codex, including the interactive
/// `cas factory` launch path, and must never make a hung/misbehaving
/// binary block startup.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Returns `true` when `codex --version` resolves and exits (successfully
/// or not — a binary that runs and errors is still "present"; we only care
/// about resolving + executing, matching `is_codex_installed()`'s existing
/// `cas-cli/src/cli/factory/mod.rs` convention) within [`PROBE_TIMEOUT`].
///
/// Uses a spawn + bounded `try_wait()` poll rather than the simpler
/// `Command::output()` (blocking, no timeout) that `is_codex_installed()`
/// uses today, because cas-a487 explicitly calls for a short timeout here:
/// this probe can run on every worker-spec resolution, not just once at
/// `cas factory` launch, so a wedged binary must not be able to stall
/// every subsequent spawn attempt indefinitely.
pub fn codex_binary_present() -> bool {
    codex_binary_present_with(|| {
        Command::new("codex")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    })
}

/// Testable core of [`codex_binary_present`] — takes a spawn closure so
/// tests can substitute a slow/hanging fake binary without depending on a
/// real `codex` install.
fn codex_binary_present_with(
    spawn: impl FnOnce() -> std::io::Result<std::process::Child>,
) -> bool {
    let Ok(mut child) = spawn() else {
        return false;
    };
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return true,
            Ok(None) => {
                if Instant::now() >= deadline {
                    // Wedged or slow — kill it and report unavailable rather
                    // than block the caller indefinitely. Best-effort: if
                    // the kill itself fails there is nothing more useful to
                    // do than move on (conservative "unavailable" verdict
                    // already covers it).
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(PROBE_POLL_INTERVAL);
            }
            Err(_) => return false,
        }
    }
}

/// Path to Codex's ChatGPT-login credential file under the given home dir.
fn codex_auth_path_in(home: &std::path::Path) -> PathBuf {
    home.join(".codex").join("auth.json")
}

/// Returns `true` when `~/.codex/auth.json` exists.
///
/// Existence only — this probe does not parse or validate the credential
/// contents (expired/invalid tokens still fail at actual Codex startup,
/// same as today; that failure mode is unrelated to "is Codex installed at
/// all," which is what the fallback decision needs).
pub fn codex_auth_present() -> bool {
    dirs::home_dir().is_some_and(|home| codex_auth_present_in(&home))
}

/// Testable core of [`codex_auth_present`] — takes an explicit home dir so
/// tests can point it at a scratch directory instead of depending on
/// process-wide `HOME` mutation (which would need cross-test locking).
fn codex_auth_present_in(home: &std::path::Path) -> bool {
    codex_auth_path_in(home).is_file()
}

/// Combined probe: both the binary and the ChatGPT-login credential must be
/// present for a Codex worker to be considered available. Either failing
/// routes to [`crate::spec_resolver::apply_codex_fallback`].
pub fn codex_available() -> bool {
    codex_binary_present() && codex_auth_present()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `codex --version` stand-in that exits immediately and successfully.
    fn fake_present() -> std::io::Result<std::process::Child> {
        Command::new("true").spawn()
    }

    /// A stand-in for "codex is not on PATH at all."
    fn fake_absent() -> std::io::Result<std::process::Child> {
        Command::new("cas-a487-definitely-not-a-real-binary-xyz").spawn()
    }

    /// A stand-in for a wedged/hanging codex binary that never exits on its
    /// own — the probe must still return within its bounded timeout rather
    /// than hang the test (or a real caller) forever.
    fn fake_hangs() -> std::io::Result<std::process::Child> {
        Command::new("sleep").arg("30").spawn()
    }

    #[test]
    fn codex_binary_present_true_when_child_exits() {
        assert!(codex_binary_present_with(fake_present));
    }

    #[test]
    fn codex_binary_present_false_when_spawn_fails() {
        assert!(!codex_binary_present_with(fake_absent));
    }

    #[test]
    fn codex_binary_present_false_and_bounded_when_child_hangs() {
        let start = Instant::now();
        let found = codex_binary_present_with(fake_hangs);
        let elapsed = start.elapsed();
        assert!(!found, "a wedged child must read as unavailable, not present");
        assert!(
            elapsed < PROBE_TIMEOUT + Duration::from_secs(1),
            "probe must not block substantially longer than PROBE_TIMEOUT — took {elapsed:?}"
        );
    }

    /// cas-a487 Tests spec: "auth.json absent → false".
    #[test]
    fn codex_auth_present_false_when_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!codex_auth_present_in(dir.path()));
    }

    /// cas-a487 Tests spec: "both present → true" (auth half).
    #[test]
    fn codex_auth_present_true_when_file_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(codex_dir.join("auth.json"), b"{}").unwrap();
        assert!(codex_auth_present_in(dir.path()));
    }

    /// A directory literally named `auth.json` must not read as the file.
    #[test]
    fn codex_auth_present_false_when_path_is_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        std::fs::create_dir_all(codex_dir.join("auth.json")).unwrap();
        assert!(!codex_auth_present_in(dir.path()));
    }
}
