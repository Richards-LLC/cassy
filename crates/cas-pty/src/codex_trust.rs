//! Pre-trust a working directory for the Codex CLI (cas-28a49, GH #97).
//!
//! # Why this exists
//!
//! Codex CLI refuses to start in a directory that is absent from the
//! `[projects]` trust table in `$CODEX_HOME/config.toml` (default
//! `~/.codex/config.toml`). It parks on its interactive "do you trust the files
//! in this folder?" onboarding screen **before** it renders a TUI, before it
//! writes a session file, and before it starts any MCP server — so a factory
//! worker launched there never registers with CAS and the spawn dies at
//! `stage=register` with a generic 60s timeout.
//!
//! Verified against codex-cli 0.146.0 on Linux by launching `codex --yolo
//! --no-alt-screen` under a real PTY in three configurations:
//!
//! | workdir state | bytes of TUI output in 12s |
//! |---|---|
//! | listed in `[projects]` as `trusted` | ~27 000 (normal startup) |
//! | absent from `[projects]` | 115 (terminal capability queries only — wedged) |
//! | absent, launched with `-c 'projects."<dir>".trust_level="trusted"'` | 115 (wedged) |
//!
//! The third row is the important one: the CLI `-c` override does **not**
//! satisfy the trust check, so the entry has to exist on disk before launch.
//! That is what [`ensure_project_trusted`] does.
//!
//! The write is deliberately conservative:
//! - it only ever **appends** a `[projects."<abs path>"]` block; it never
//!   rewrites, reformats, or reorders the operator's config;
//! - if a `[projects."<abs path>"]` header already exists it is left completely
//!   alone, whatever its `trust_level` — an operator who explicitly untrusted a
//!   directory keeps that decision;
//! - the file is replaced via write-temp-then-rename so a concurrently reading
//!   Codex process never observes a half-written config.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// What [`ensure_project_trusted`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexTrustOutcome {
    /// Every candidate path already had a `[projects."..."]` entry.
    AlreadyPresent,
    /// One or more trust entries were appended to the config file.
    Added(Vec<String>),
    /// Nothing was attempted; the reason is operator-facing.
    Skipped(&'static str),
}

/// Resolve the directory Codex reads its `config.toml` from.
///
/// `$CODEX_HOME` wins when set (that is Codex's own override), otherwise
/// `~/.codex`.
pub fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".codex")),
    }
}

/// Escape a path for use inside a TOML basic string key.
fn escape_toml_basic(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

/// Reverse of [`escape_toml_basic`] for the two escapes we emit.
fn unescape_toml_basic(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// The literal block appended for a newly trusted path.
pub fn trust_entry_block(path: &str) -> String {
    format!(
        "\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        escape_toml_basic(path)
    )
}

/// Does `config` already declare a `[projects."<path>"]` table for `path`?
///
/// Matches the header line only — the body (`trust_level = ...`) is
/// intentionally not inspected, because an existing entry means the operator
/// (or Codex itself) has already decided about this directory.
pub fn config_has_project(config: &str, path: &str) -> bool {
    config.lines().any(|line| {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("[projects.") else {
            return false;
        };
        let Some(inner) = rest.strip_suffix(']') else {
            return false;
        };
        let inner = inner.trim();
        let key = if let Some(k) = inner.strip_prefix('"').and_then(|k| k.strip_suffix('"')) {
            unescape_toml_basic(k)
        } else if let Some(k) = inner.strip_prefix('\'').and_then(|k| k.strip_suffix('\'')) {
            k.to_string()
        } else {
            return false;
        };
        key == path
    })
}

/// Path keys to trust for `workdir`: the path as spawned, plus its canonical
/// form when symlink resolution changes it (Codex matches on the cwd string it
/// resolves, and a factory worktree can be reached through either).
fn candidate_keys(workdir: &Path) -> Vec<String> {
    let mut keys = vec![workdir.to_string_lossy().to_string()];
    if let Ok(canonical) = std::fs::canonicalize(workdir) {
        let canonical = canonical.to_string_lossy().to_string();
        if !keys.contains(&canonical) {
            keys.push(canonical);
        }
    }
    keys
}

/// Serializes read-modify-write of the config within this process. Cross-process
/// safety comes from the atomic rename below.
fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Ensure `workdir` is trusted in the Codex config at `config_path`.
///
/// Creates the file (and its parent directory) when missing.
pub fn ensure_project_trusted_in(
    config_path: &Path,
    workdir: &Path,
) -> io::Result<CodexTrustOutcome> {
    if !workdir.is_absolute() {
        return Ok(CodexTrustOutcome::Skipped(
            "worker cwd is not absolute; cannot key a Codex trust entry",
        ));
    }
    let _guard = write_lock().lock().unwrap_or_else(|e| e.into_inner());

    let existing = match std::fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    let missing: Vec<String> = candidate_keys(workdir)
        .into_iter()
        .filter(|key| !config_has_project(&existing, key))
        .collect();
    if missing.is_empty() {
        return Ok(CodexTrustOutcome::AlreadyPresent);
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    for key in &missing {
        updated.push_str(&trust_entry_block(key));
    }

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Write-then-rename so a Codex process reading config.toml concurrently
    // never sees a truncated file.
    let tmp = config_path.with_extension(format!("toml.cas-{}", std::process::id()));
    std::fs::write(&tmp, &updated)?;
    if let Err(e) = std::fs::rename(&tmp, config_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(CodexTrustOutcome::Added(missing))
}

/// Ensure `workdir` is trusted in the resolved Codex config
/// (`$CODEX_HOME/config.toml`, else `~/.codex/config.toml`).
///
/// Never fails a spawn: I/O errors are logged and reported, and the caller
/// launches Codex anyway (worst case is the pre-existing behaviour).
pub fn ensure_project_trusted(workdir: &Path) -> CodexTrustOutcome {
    let Some(home) = codex_home() else {
        return CodexTrustOutcome::Skipped(
            "could not resolve CODEX_HOME or a home directory for ~/.codex",
        );
    };
    let config_path = home.join("config.toml");
    match ensure_project_trusted_in(&config_path, workdir) {
        Ok(CodexTrustOutcome::Added(keys)) => {
            tracing::info!(
                config = %config_path.display(),
                paths = ?keys,
                "cas-28a49: pre-trusted workdir for Codex; without this the CLI parks on its \
                 interactive trust prompt and the worker never registers"
            );
            CodexTrustOutcome::Added(keys)
        }
        Ok(other) => other,
        Err(e) => {
            tracing::warn!(
                config = %config_path.display(),
                workdir = %workdir.display(),
                error = %e,
                "cas-28a49: could not pre-trust workdir for Codex; if the worker never registers, \
                 add [projects.\"<workdir>\"] trust_level = \"trusted\" to the Codex config"
            );
            CodexTrustOutcome::Skipped("failed to update the Codex config file")
        }
    }
}

/// Operator-facing hint appended to a Codex worker's registration-timeout
/// diagnostic (cas-28a49, GH #97). Named separately so the daemon and its tests
/// share one string.
pub const CODEX_TRUST_TIMEOUT_HINT: &str = "This worker runs cli=codex: Codex refuses to start in a directory that is not listed in \
     [projects] in $CODEX_HOME/config.toml (default ~/.codex/config.toml), parking on its \
     interactive trust prompt before it can register. CAS pre-trusts the worker's cwd at launch, \
     so if that file is read-only or the entry says trust_level is not \"trusted\", fix it and \
     respawn.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_existing_double_quoted_entry() {
        let config = "[projects.\"/home/a/proj\"]\ntrust_level = \"trusted\"\n";
        assert!(config_has_project(config, "/home/a/proj"));
        assert!(!config_has_project(config, "/home/a/other"));
    }

    #[test]
    fn detects_existing_single_quoted_entry() {
        let config = "[projects.'/home/a/proj']\ntrust_level = \"trusted\"\n";
        assert!(config_has_project(config, "/home/a/proj"));
    }

    #[test]
    fn does_not_match_prefixes_or_other_tables() {
        let config = "[projects.\"/home/a/proj-two\"]\n[mcp_servers.cs]\n";
        assert!(!config_has_project(config, "/home/a/proj"));
        assert!(!config_has_project(config, "cs"));
    }

    #[test]
    fn escaped_keys_round_trip() {
        let path = "/home/a/we\"ird\\dir";
        let block = trust_entry_block(path);
        assert!(config_has_project(&block, path), "block was: {block}");
    }

    #[test]
    fn appends_entry_when_missing_and_creates_file() {
        let dir = tempdir();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(
            matches!(outcome, CodexTrustOutcome::Added(_)),
            "first call must append an entry, got {outcome:?}"
        );
        let written = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            config_has_project(&written, &workdir.to_string_lossy()),
            "config must contain the trust entry: {written}"
        );
        assert!(written.contains("trust_level = \"trusted\""));
    }

    #[test]
    fn second_call_is_a_noop_and_preserves_operator_content() {
        let dir = tempdir();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(&config_path, "# operator comment\nmodel = \"gpt-5\"\n").unwrap();

        ensure_project_trusted_in(&config_path, &workdir).unwrap();
        let after_first = std::fs::read_to_string(&config_path).unwrap();
        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert_eq!(outcome, CodexTrustOutcome::AlreadyPresent);
        let after_second = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(after_first, after_second, "second call must not rewrite");
        assert!(after_second.contains("# operator comment"));
        assert!(after_second.contains("model = \"gpt-5\""));
    }

    #[test]
    fn respects_an_explicit_untrusted_decision() {
        let dir = tempdir();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let original = format!(
            "[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            workdir.to_string_lossy()
        );
        std::fs::write(&config_path, &original).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert_eq!(outcome, CodexTrustOutcome::AlreadyPresent);
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn relative_workdir_is_skipped() {
        let dir = tempdir();
        let config_path = dir.join("config.toml");
        let outcome = ensure_project_trusted_in(&config_path, Path::new("relative/dir")).unwrap();
        assert!(matches!(outcome, CodexTrustOutcome::Skipped(_)));
        assert!(!config_path.exists());
    }

    #[test]
    fn codex_home_prefers_env_override() {
        // Not parallel-safe with other env mutation; keep it self-contained.
        let previous = std::env::var_os("CODEX_HOME");
        unsafe { std::env::set_var("CODEX_HOME", "/tmp/cas-codex-home-test") };
        assert_eq!(
            codex_home(),
            Some(PathBuf::from("/tmp/cas-codex-home-test"))
        );
        unsafe {
            match previous {
                Some(v) => std::env::set_var("CODEX_HOME", v),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
    }

    /// Minimal unique temp dir (cas-pty has no tempfile dev-dependency).
    fn tempdir() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "cas-codex-trust-{}-{}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
