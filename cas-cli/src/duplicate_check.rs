//! Startup self-check for multiple `cas` binaries on PATH.
//!
//! When a developer has multiple `cas` binaries on PATH (e.g., a stale
//! `/usr/local/bin/cas` alongside a fresh `~/.local/bin/cas`), shell PATH order
//! or absolute-path invocations can silently resolve to the stale copy. This
//! module scans PATH on startup and emits a single-line stderr warning when
//! duplicates with different mtimes are found.
//!
//! Gating (see [`should_run`]):
//! * Skipped for `hook`, `serve`, `factory`, and `bridge` subcommands — they
//!   must stay silent per `feedback_hook_performance`.
//! * Skipped when no user-visible subcommand is present and factory-launch
//!   flags (`--new`, `--workers`, etc.) are — the factory alias rewrite in
//!   `main.rs` injects the `factory` token after this check runs, so the gate
//!   must spot the alias on its own.
//! * Skipped when stderr is not a TTY unless `CAS_WARN_DUPLICATES=1`.
//! * Unconditionally silenced when `CAS_SUPPRESS_DUPLICATE_WARNING=1`.
//!
//! Env var semantics: both `CAS_WARN_DUPLICATES` and `CAS_SUPPRESS_DUPLICATE_WARNING`
//! require a truthy value (non-empty and not `"0"` / `"false"`). Setting them
//! to `0` or an empty string does **not** enable the gate.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Subcommands that must stay silent (hooks, long-running servers, factory TUI,
/// Slack bridge daemon).
const QUIET_SUBCOMMANDS: &[&str] = &["hook", "serve", "factory", "bridge"];

/// Flags that imply factory launch without an explicit `factory` subcommand
/// token (see `main::maybe_rewrite_factory_args`).
const FACTORY_LAUNCH_FLAGS: &[&str] = &[
    "--new",
    "--workers",
    "-w",
    "--name",
    "-n",
    "--no-worktrees",
    "--worktree-root",
    "--tabbed",
    "--record",
    "--legacy",
];

/// Read a boolean-ish env var. Returns `true` when the var is set to a value
/// that is neither empty nor `"0"` / `"false"`. Plain `is_some()` would treat
/// `FOO=0` as "set" and force the feature on, contradicting the docs.
fn env_flag(name: &str) -> bool {
    let Some(raw) = std::env::var_os(name) else {
        return false;
    };
    let Some(s) = raw.to_str() else {
        // Non-UTF-8 content. Treat any non-empty exotic value as truthy.
        return !raw.is_empty();
    };
    match s.trim() {
        "" | "0" | "false" | "FALSE" | "False" => false,
        _ => true,
    }
}

/// Decide whether the duplicate check should run for this invocation.
///
/// `args` is the full argv (including argv[0]).
pub fn should_run(args: &[String]) -> bool {
    if env_flag("CAS_SUPPRESS_DUPLICATE_WARNING") {
        return false;
    }

    // Skip for subcommands that must be silent or long-lived.
    if let Some(sub) = first_subcommand(args) {
        if QUIET_SUBCOMMANDS.contains(&sub.as_str()) {
            return false;
        }
    } else if contains_factory_launch_flag(args) {
        // No subcommand token but factory-launch flags present: `cas --new -w4`
        // will be rewritten to `cas factory --new -w4` after this check. Treat
        // it as the factory subcommand for gating purposes.
        return false;
    }

    // Force-on override bypasses the TTY gate.
    if env_flag("CAS_WARN_DUPLICATES") {
        return true;
    }

    is_stderr_tty()
}

/// Return the first non-flag token after argv[0], if any.
fn first_subcommand(args: &[String]) -> Option<String> {
    for token in args.iter().skip(1) {
        if token.starts_with('-') {
            continue;
        }
        return Some(token.clone());
    }
    None
}

/// True when argv (past argv[0]) contains a factory-launch flag. Kept
/// deliberately narrow — mirrors `main::contains_factory_launch_flag` only for
/// the subset of flags that can appear without an explicit subcommand.
fn contains_factory_launch_flag(args: &[String]) -> bool {
    args.iter().skip(1).any(|t| {
        FACTORY_LAUNCH_FLAGS.contains(&t.as_str())
            || t.starts_with("--workers=")
            || t.starts_with("--name=")
            || t.starts_with("--worktree-root=")
            || (t.starts_with("-w") && t.len() > 2)
            || (t.starts_with("-n") && t.len() > 2)
    })
}

#[cfg(unix)]
fn is_stderr_tty() -> bool {
    // SAFETY: isatty takes an fd and has no preconditions.
    unsafe { libc::isatty(libc::STDERR_FILENO) != 0 }
}

#[cfg(not(unix))]
fn is_stderr_tty() -> bool {
    false
}

/// Scan PATH for executables named `cas` and return the unique list, preserving
/// PATH order. Symlinks are canonicalised so that `/usr/bin/cas -> /usr/local/bin/cas`
/// does not count as two distinct binaries.
pub fn find_cas_binaries_on_path() -> Vec<PathBuf> {
    let path_var = match std::env::var_os("PATH") {
        Some(v) => v,
        None => return Vec::new(),
    };

    let mut seen_user_visible: Vec<PathBuf> = Vec::new();
    let mut seen_canonical: Vec<PathBuf> = Vec::new();
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join("cas");
        if !is_executable_file(&candidate) {
            continue;
        }
        let canonical = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if seen_canonical.iter().any(|c| c == &canonical) {
            continue;
        }
        seen_canonical.push(canonical);
        seen_user_visible.push(candidate);
    }
    seen_user_visible
}

fn is_executable_file(p: &Path) -> bool {
    let meta = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// A warning about duplicate binaries. Returned by [`build_warning`] so callers
/// (and tests) can decide how to present it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateWarning {
    /// The binary the current process is running from (argv[0] resolved).
    pub active: PathBuf,
    /// Other `cas` binaries on PATH with different mtimes.
    pub stale: Vec<PathBuf>,
}

impl DuplicateWarning {
    pub fn render(&self) -> String {
        let stale = self
            .stale
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "warning: multiple cas binaries on PATH with different mtimes — active: {}, stale: {} (set CAS_SUPPRESS_DUPLICATE_WARNING=1 to silence)",
            self.active.display(),
            stale,
        )
    }
}

/// Given a list of `cas` binaries on PATH and the active binary path, build a
/// warning if any of the others have an mtime that differs from the active one.
/// Returns `None` if only one binary exists, all mtimes match, or the active
/// binary is not present on PATH (we can't tell which copy is "correct").
pub fn build_warning(binaries: &[PathBuf], active: &Path) -> Option<DuplicateWarning> {
    if binaries.len() < 2 {
        return None;
    }
    // Don't emit a misleading "stale" list when the running binary was
    // launched from outside PATH — we have no ground truth to pick the active
    // one against.
    if !binaries.iter().any(|b| same_path(b, active)) {
        return None;
    }
    let active_mtime = mtime_of(active)?;
    let mut stale = Vec::new();
    for bin in binaries {
        if same_path(bin, active) {
            continue;
        }
        let Some(m) = mtime_of(bin) else { continue };
        if m != active_mtime {
            stale.push(bin.clone());
        }
    }
    if stale.is_empty() {
        None
    } else {
        Some(DuplicateWarning {
            active: active.to_path_buf(),
            stale,
        })
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let ac = std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let bc = std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    ac == bc
}

fn mtime_of(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// Best-effort resolution of the currently-executing binary. Falls back to
/// `argv[0]` when `current_exe` fails (e.g., on exotic filesystems).
fn resolve_active_binary(argv0: &OsStr) -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from(argv0))
}

/// Entry point: run the check and print the warning once.
///
/// Best-effort; any filesystem error silently suppresses the warning.
pub fn check_and_warn(args: &[String]) {
    if !should_run(args) {
        return;
    }
    let Some(argv0) = args.first() else {
        return;
    };
    let active = resolve_active_binary(OsStr::new(argv0));
    let binaries = find_cas_binaries_on_path();
    if let Some(w) = build_warning(&binaries, &active) {
        eprintln!("{}", w.render());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use std::fs;
    use std::time::{Duration, SystemTime};

    #[test]
    fn should_run_skips_hook_subcommand() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_WARN_DUPLICATES", None), ("CAS_SUPPRESS_DUPLICATE_WARNING", None)]);
        g.set("CAS_WARN_DUPLICATES", "1");
        let args = vec!["cas".to_string(), "hook".to_string(), "PreToolUse".to_string()];
        assert!(!should_run(&args));
    }

    #[test]
    fn should_run_skips_serve_subcommand() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_WARN_DUPLICATES", None), ("CAS_SUPPRESS_DUPLICATE_WARNING", None)]);
        g.set("CAS_WARN_DUPLICATES", "1");
        let args = vec!["cas".to_string(), "serve".to_string()];
        assert!(!should_run(&args));
    }

    #[test]
    fn should_run_skips_factory_subcommand() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_WARN_DUPLICATES", None), ("CAS_SUPPRESS_DUPLICATE_WARNING", None)]);
        g.set("CAS_WARN_DUPLICATES", "1");
        let args = vec!["cas".to_string(), "factory".to_string(), "--new".to_string()];
        assert!(!should_run(&args));
    }

    #[test]
    fn should_run_skips_bridge_subcommand() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_WARN_DUPLICATES", None), ("CAS_SUPPRESS_DUPLICATE_WARNING", None)]);
        g.set("CAS_WARN_DUPLICATES", "1");
        let args = vec!["cas".to_string(), "bridge".to_string(), "serve".to_string()];
        assert!(!should_run(&args));
    }

    #[test]
    fn should_run_skips_implicit_factory_launch() {
        // `cas --new -w4` gets rewritten to `cas factory --new -w4` in main.rs,
        // but the check fires on raw_args before that rewrite. The gate must
        // still suppress it.
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_WARN_DUPLICATES", None), ("CAS_SUPPRESS_DUPLICATE_WARNING", None)]);
        g.set("CAS_WARN_DUPLICATES", "1");
        let args = vec!["cas".to_string(), "--new".to_string(), "-w4".to_string()];
        assert!(!should_run(&args));
    }

    #[test]
    fn should_run_force_on_with_warn_duplicates() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_WARN_DUPLICATES", None), ("CAS_SUPPRESS_DUPLICATE_WARNING", None)]);
        g.set("CAS_WARN_DUPLICATES", "1");
        let args = vec!["cas".to_string(), "memory".to_string(), "list".to_string()];
        assert!(should_run(&args));
    }

    #[test]
    fn should_run_respects_suppress_env() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_WARN_DUPLICATES", None), ("CAS_SUPPRESS_DUPLICATE_WARNING", None)]);
        g.set("CAS_SUPPRESS_DUPLICATE_WARNING", "1");
        let args = vec!["cas".to_string(), "memory".to_string()];
        assert!(!should_run(&args));
    }

    #[test]
    fn env_flag_ignores_zero_and_empty() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_WARN_DUPLICATES", None), ("CAS_SUPPRESS_DUPLICATE_WARNING", None)]);
        g.set("CAS_WARN_DUPLICATES", "0");
        assert!(!env_flag("CAS_WARN_DUPLICATES"));
        g.set("CAS_WARN_DUPLICATES", "");
        assert!(!env_flag("CAS_WARN_DUPLICATES"));
        g.set("CAS_WARN_DUPLICATES", "false");
        assert!(!env_flag("CAS_WARN_DUPLICATES"));
        g.set("CAS_WARN_DUPLICATES", "1");
        assert!(env_flag("CAS_WARN_DUPLICATES"));
        g.remove("CAS_WARN_DUPLICATES");
        assert!(!env_flag("CAS_WARN_DUPLICATES"));
    }

    #[test]
    fn first_subcommand_ignores_flags() {
        let args = vec![
            "cas".to_string(),
            "--verbose".to_string(),
            "task".to_string(),
            "list".to_string(),
        ];
        assert_eq!(first_subcommand(&args).as_deref(), Some("task"));
    }

    #[test]
    fn first_subcommand_none_for_flags_only() {
        let args = vec!["cas".to_string(), "--version".to_string()];
        assert_eq!(first_subcommand(&args), None);
        let bare = vec!["cas".to_string()];
        assert_eq!(first_subcommand(&bare), None);
    }

    #[test]
    fn contains_factory_launch_flag_matches_forms() {
        assert!(contains_factory_launch_flag(&[
            "cas".to_string(),
            "--new".to_string()
        ]));
        assert!(contains_factory_launch_flag(&[
            "cas".to_string(),
            "--workers=4".to_string()
        ]));
        assert!(contains_factory_launch_flag(&[
            "cas".to_string(),
            "-w4".to_string()
        ]));
        assert!(!contains_factory_launch_flag(&[
            "cas".to_string(),
            "--verbose".to_string()
        ]));
    }

    #[test]
    fn build_warning_none_for_single_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("cas");
        fs::write(&a, b"#!/bin/sh\n").unwrap();
        assert!(build_warning(std::slice::from_ref(&a), &a).is_none());
    }

    fn set_mtime(p: &Path, t: SystemTime) -> SystemTime {
        let f = fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(t).unwrap();
        // Read back — filesystem mtime resolution may round (e.g., 2s on FAT32,
        // 1s on ext3), and we want tests to compare stored values, not the
        // in-memory value we handed to the kernel.
        fs::metadata(p).unwrap().modified().unwrap()
    }

    #[test]
    fn build_warning_none_when_mtimes_match() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a_cas");
        let b = tmp.path().join("b_cas");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let t = SystemTime::now();
        let ma = set_mtime(&a, t);
        let mb = set_mtime(&b, t);
        assert_eq!(ma, mb);
        assert!(build_warning(&[a.clone(), b.clone()], &a).is_none());
    }

    #[test]
    fn build_warning_flags_differing_mtimes() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a_cas");
        let b = tmp.path().join("b_cas");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let t = SystemTime::now();
        set_mtime(&a, t);
        set_mtime(&b, t - Duration::from_secs(3600));
        let w = build_warning(&[a.clone(), b.clone()], &a).expect("expected warning");
        assert_eq!(w.active, a);
        assert_eq!(w.stale, vec![b]);
        let rendered = w.render();
        assert!(rendered.contains("active:"));
        assert!(rendered.contains("stale:"));
        assert!(rendered.contains("CAS_SUPPRESS_DUPLICATE_WARNING"));
    }

    #[test]
    fn build_warning_none_when_active_not_in_binaries() {
        // When cas is invoked via an absolute path that doesn't appear in PATH
        // (e.g., a systemd unit pointing at /opt/cas/cas), we can't pick a
        // "correct" copy, so we stay silent rather than mislabel fresher PATH
        // entries as "stale".
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a_cas");
        let b = tmp.path().join("b_cas");
        let outside = tmp.path().join("outside_cas");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        fs::write(&outside, b"o").unwrap();
        let t = SystemTime::now();
        set_mtime(&a, t);
        set_mtime(&b, t - Duration::from_secs(3600));
        set_mtime(&outside, t - Duration::from_secs(7200));
        assert!(build_warning(&[a, b], &outside).is_none());
    }
}
