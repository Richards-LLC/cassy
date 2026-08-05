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
//! The write is deliberately conservative — corrupting this file would stop
//! Codex from starting *anywhere*, which is strictly worse than the wedge being
//! fixed:
//! - it only ever **appends** a `[projects."<abs path>"]` block; it never
//!   rewrites, reformats, or reorders the operator's config;
//! - presence is decided by parsing the TOML, not by matching lines, so an
//!   existing entry in any spelling (`[projects."x"] # comment`, an inline
//!   `[projects]` entry, a dotted `projects."x".trust_level = …`) is left
//!   completely alone, whatever its `trust_level` — an operator who explicitly
//!   untrusted a directory keeps that decision, and no duplicate table is ever
//!   created;
//! - a config that does not parse, a result that would not parse, and a path
//!   that cannot be a TOML key (control characters, non-UTF-8) all abort the
//!   write instead of guessing;
//! - the read-modify-write window is guarded by a bounded lock file so a
//!   concurrent factory process (or a human answering Codex's own prompt) does
//!   not have its edit reverted;
//! - the file is replaced via write-temp-then-rename, through any symlink and
//!   preserving the original mode, so a concurrently reading Codex process never
//!   observes a half-written config and a `0600` config stays `0600`.

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

/// The literal block appended for a newly trusted path.
pub fn trust_entry_block(path: &str) -> String {
    format!(
        "\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        escape_toml_basic(path)
    )
}

/// Does `config` already declare a project entry for `path`?
///
/// Parsed with a real TOML parser rather than matched line-by-line, because
/// `[projects."x"]`, `[projects."x"] # comment`, `[ projects."x" ]`,
/// `projects."x".trust_level = ...` and an inline `[projects]` table entry are
/// all the same key. Missing any of those spellings and appending a second
/// `[projects."x"]` table would make the whole file invalid TOML — TOML forbids
/// redeclaring a table — which would stop Codex from starting **anywhere**, a
/// far worse failure than the bug being fixed.
///
/// `Err` means the config does not parse at all; the caller must then refuse to
/// touch it.
///
/// Only the key's presence is checked, never `trust_level`'s value: an existing
/// entry means the operator (or Codex itself) already decided about this
/// directory.
pub fn config_has_project(config: &str, path: &str) -> Result<bool, toml::de::Error> {
    let parsed: toml::Value = toml::from_str(config)?;
    Ok(parsed
        .get("projects")
        .and_then(|projects| projects.get(path))
        .is_some())
}

/// Paths that cannot be written as a TOML basic-string key without corrupting
/// (or, with an embedded newline, *injecting into*) the operator's config.
fn path_is_unsafe_key(path: &str) -> bool {
    path.contains('\u{FFFD}') || path.chars().any(|c| c.is_control())
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

/// Serializes read-modify-write of the config within this process. Across
/// processes, [`ConfigLock`] guards the same window.
fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Best-effort cross-process lock over the read-modify-write window.
///
/// Atomic rename only prevents a *torn read*; it does not prevent a lost
/// update. Two factory processes (or a human answering Codex's own trust prompt
/// in the window) would otherwise have the later rename revert the whole file to
/// the earlier snapshot. Acquisition is bounded: a stale lock from a crashed
/// process must never wedge a spawn, so after the timeout we proceed unlocked.
struct ConfigLock {
    path: PathBuf,
    held: bool,
}

impl ConfigLock {
    fn acquire(config_path: &Path) -> Self {
        let path = config_path.with_extension("toml.cas-lock");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Self { path, held: true },
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if std::time::Instant::now() >= deadline {
                        tracing::warn!(
                            lock = %path.display(),
                            "cas-28a49: Codex config lock still held after 2s; proceeding \
                             unlocked (remove the file if it is stale)"
                        );
                        return Self { path, held: false };
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                // Unwritable directory, missing parent, etc. — the write itself
                // will report the real error.
                Err(_) => return Self { path, held: false },
            }
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        if self.held {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Ensure `workdir` is trusted in the Codex config at `config_path`.
///
/// Creates the file (and its parent directory) when missing. Refuses to write
/// when the existing config does not parse, when the result would not parse, or
/// when the path cannot be represented as a TOML key — corrupting
/// `config.toml` would break every Codex launch on the machine, which is
/// strictly worse than the wedge this function prevents.
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

    // `read_to_string` follows a symlink but `rename` would replace the *link*.
    // Operators who keep ~/.codex/config.toml symlinked into a dotfiles repo
    // must keep that link, so write through to the real file.
    let config_path = std::fs::canonicalize(config_path)
        .ok()
        .unwrap_or_else(|| config_path.to_path_buf());
    let _lock = ConfigLock::acquire(&config_path);

    let existing = match std::fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    let keys = candidate_keys(workdir);
    if keys.iter().any(|key| path_is_unsafe_key(key)) {
        return Ok(CodexTrustOutcome::Skipped(
            "worker cwd contains control characters or is not valid UTF-8; refusing to write a \
             Codex trust entry for it",
        ));
    }

    let mut missing = Vec::new();
    for key in keys {
        match config_has_project(&existing, &key) {
            Ok(false) => missing.push(key),
            Ok(true) => {}
            Err(e) => {
                tracing::warn!(
                    config = %config_path.display(),
                    error = %e,
                    "cas-28a49: Codex config does not parse as TOML; leaving it untouched"
                );
                return Ok(CodexTrustOutcome::Skipped(
                    "the Codex config file does not parse as TOML; refusing to modify it",
                ));
            }
        }
    }
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
    if let Err(e) = toml::from_str::<toml::Value>(&updated) {
        tracing::error!(
            config = %config_path.display(),
            error = %e,
            "cas-28a49: appending the Codex trust entry would produce invalid TOML; \
             leaving the config untouched"
        );
        return Ok(CodexTrustOutcome::Skipped(
            "appending the trust entry would produce invalid TOML; config left untouched",
        ));
    }

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Write-then-rename so a Codex process reading config.toml concurrently
    // never sees a truncated file.
    let tmp = config_path.with_extension(format!("toml.cas-{}", std::process::id()));
    std::fs::write(&tmp, &updated)?;
    // The rename replaces the original's metadata wholesale, so carry the
    // original mode across: this file can hold API keys and MCP env secrets, and
    // a 0600 config must not silently widen to 0644.
    if let Ok(meta) = std::fs::metadata(&config_path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if let Err(e) = std::fs::rename(&tmp, &config_path) {
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
     so check that file for a [projects.\"<worker cwd>\"] entry with trust_level = \"trusted\"; if \
     it is missing, the daemon log records why the write was skipped (unparseable config, or the \
     entry already existed with another trust_level).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_existing_entry_in_every_toml_spelling() {
        // cas-28a49 review follow-up: a matcher that misses any of these would
        // append a duplicate [projects."x"] table, and TOML forbids redeclaring
        // a table — the config would stop parsing and Codex would fail to start
        // for EVERY directory, not just this one.
        for config in [
            "[projects.\"/home/a/proj\"]\ntrust_level = \"trusted\"\n",
            "[projects.'/home/a/proj']\ntrust_level = \"trusted\"\n",
            "[projects.\"/home/a/proj\"] # main repo\ntrust_level = \"trusted\"\n",
            "[ projects.\"/home/a/proj\" ]\ntrust_level = \"trusted\"\n",
            "[projects]\n\"/home/a/proj\" = { trust_level = \"trusted\" }\n",
            "projects.\"/home/a/proj\".trust_level = \"trusted\"\n",
        ] {
            assert!(
                config_has_project(config, "/home/a/proj").unwrap(),
                "must detect the existing key in: {config}"
            );
            assert!(
                !config_has_project(config, "/home/a/other").unwrap(),
                "must not report an unrelated key present in: {config}"
            );
        }
    }

    #[test]
    fn does_not_match_prefixes_or_other_tables() {
        let config = "[projects.\"/home/a/proj-two\"]\n[mcp_servers.cs]\n";
        assert!(!config_has_project(config, "/home/a/proj").unwrap());
        assert!(!config_has_project(config, "cs").unwrap());
    }

    #[test]
    fn unparseable_config_is_reported_not_guessed() {
        assert!(config_has_project("this is [not( toml", "/home/a/proj").is_err());
    }

    #[test]
    fn escaped_keys_round_trip() {
        let path = "/home/a/we\"ird\\dir";
        let block = trust_entry_block(path);
        assert!(
            config_has_project(&block, path).unwrap(),
            "block was: {block}"
        );
    }

    #[test]
    fn appends_entry_when_missing_and_creates_file() {
        let dir = TempDir::new();
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
            config_has_project(&written, &workdir.to_string_lossy()).unwrap(),
            "config must contain the trust entry: {written}"
        );
        assert!(written.contains("trust_level = \"trusted\""));
    }

    #[test]
    fn second_call_is_a_noop_and_preserves_operator_content() {
        let dir = TempDir::new();
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
    fn inline_projects_table_entry_is_not_duplicated() {
        // The shape Codex's own config can take; appending here would produce a
        // duplicate-table parse error for the whole file.
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let original = format!(
            "[projects]\n\"{}\" = {{ trust_level = \"trusted\" }}\n",
            workdir.to_string_lossy()
        );
        std::fs::write(&config_path, &original).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert_eq!(outcome, CodexTrustOutcome::AlreadyPresent);
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn unparseable_operator_config_is_left_untouched() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let original = "this is [not( toml\n";
        std::fs::write(&config_path, original).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(
            matches!(outcome, CodexTrustOutcome::Skipped(_)),
            "must refuse to modify an unparseable config, got {outcome:?}"
        );
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn respects_an_explicit_untrusted_decision() {
        let dir = TempDir::new();
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
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let outcome = ensure_project_trusted_in(&config_path, Path::new("relative/dir")).unwrap();
        assert!(matches!(outcome, CodexTrustOutcome::Skipped(_)));
        assert!(!config_path.exists());
    }

    #[test]
    fn control_character_paths_are_refused() {
        // A newline in a path is legal on Linux and would otherwise inject a
        // whole table (e.g. [mcp_servers.evil]) into the operator's config.
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj\n[mcp_servers.evil]\ncommand = \"sh\"\n");

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(
            matches!(outcome, CodexTrustOutcome::Skipped(_)),
            "must refuse a control-character path, got {outcome:?}"
        );
        assert!(!config_path.exists());
        assert!(path_is_unsafe_key("/a/b\nc"));
        assert!(!path_is_unsafe_key("/a/b c"));
    }

    #[cfg(unix)]
    #[test]
    fn existing_file_mode_is_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(&config_path, "model = \"gpt-5\"\n").unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        ensure_project_trusted_in(&config_path, &workdir).unwrap();
        let mode = std::fs::metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "a 0600 config (it can hold API keys) must not widen on rewrite"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_keeps_its_link() {
        let dir = TempDir::new();
        let real = dir.join("dotfiles-config.toml");
        let link = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(&real, "model = \"gpt-5\"\n").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        ensure_project_trusted_in(&link, &workdir).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "a symlinked config.toml must stay a symlink, not be replaced by a regular file"
        );
        assert!(
            config_has_project(
                &std::fs::read_to_string(&real).unwrap(),
                &workdir.to_string_lossy()
            )
            .unwrap(),
            "the entry must land in the symlink target"
        );
    }

    #[test]
    fn codex_home_prefers_env_override() {
        // Env mutation is process-global: this is the only test in the crate
        // that touches CODEX_HOME, and it restores the previous value.
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

    /// Minimal self-cleaning temp dir (cas-pty has no tempfile dev-dependency).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("cas-codex-trust-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
