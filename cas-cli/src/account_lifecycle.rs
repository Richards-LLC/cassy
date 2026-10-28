//! Account mutations and the transaction around a newly authenticated profile.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::accounts::{self, Provider};
use anyhow::{Context, Result, bail};

#[derive(Debug)]
pub(crate) struct Target {
    pub provider: Provider,
    pub name: String,
    pub folder: PathBuf,
    pub email: Option<String>,
}

/// Accept a unique alias/email, or `claude:<alias>` / `codex:<alias>`.
pub(crate) fn resolve(home: &Path, key: &str) -> Result<Target> {
    let (provider, key) = match key.split_once(':') {
        Some(("claude", key)) => (Some(Provider::Claude), key),
        Some(("codex", key)) => (Some(Provider::Codex), key),
        Some(_) => bail!("Unknown provider; use claude:<account> or codex:<account>"),
        None => (None, key),
    };
    let mut aliases = Vec::new();
    let mut emails = Vec::new();
    for p in Provider::ALL
        .into_iter()
        .filter(|p| provider.is_none_or(|v| v == *p))
    {
        for dir in accounts::discover_aliases(p, home)? {
            let name = dir.label.strip_prefix(p.prefix()).unwrap_or("main");
            if name == key || dir.email.as_deref() == Some(key) {
                let is_alias = name == key;
                let target = Target {
                    provider: p,
                    name: name.into(),
                    folder: dir.path,
                    email: dir.email,
                };
                if is_alias {
                    aliases.push(target);
                } else {
                    emails.push(target);
                }
            }
        }
    }
    let mut matches = if aliases.is_empty() { emails } else { aliases };
    if matches.len() > 1 {
        bail!("Account {key} is ambiguous; use provider:<folder-name>");
    }
    matches
        .pop()
        .ok_or_else(|| anyhow::anyhow!("Unknown account: {key}; run `cas accounts list`"))
}

pub(crate) fn ensure_idle(target: &Target, home: &Path) -> Result<()> {
    if target.name == "main" {
        bail!("The main account is protected");
    }
    if in_use(&target.folder, home)? {
        bail!(
            "Account {} is in use; stop its sessions before changing it",
            target.name
        );
    }
    Ok(())
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}

fn env_uses(bytes: &[u8], folder: &Path) -> bool {
    bytes.split(|b| *b == 0).any(|entry| {
        [
            b"CLAUDE_CONFIG_DIR=".as_slice(),
            b"CODEX_HOME=".as_slice(),
            b"CLAUDE_SECURESTORAGE_CONFIG_DIR=".as_slice(),
        ]
        .iter()
        .any(|key| {
            entry
                .strip_prefix(*key)
                .and_then(|v| std::str::from_utf8(v).ok())
                .is_some_and(|v| same_path(Path::new(v), folder))
        })
    })
}

/// ps prints space-separated environment entries. Match a whole selector value,
/// so a session on email-2 never prevents removing the idle email account.
#[cfg(any(target_os = "macos", test))]
fn ps_uses(text: &str, folder: &Path) -> bool {
    [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ]
    .iter()
    .any(|key| {
        let selector = format!("{key}={}", folder.display());
        text.match_indices(&selector).any(|(offset, _)| {
            text[..offset]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace)
                && text[offset + selector.len()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
    })
}

fn repo_inspection_error(error: impl std::fmt::Display) -> anyhow::Error {
    tracing::debug!(%error, "Cannot inspect known repositories");
    // The CLI prints anyhow chains on the same line. Keep the suggested
    // command separate and copyable instead of appending SQLite's SQL to it.
    anyhow::anyhow!("Cannot inspect known repositories; account kept\nRun: cas update")
}

fn in_use(folder: &Path, home: &Path) -> Result<bool> {
    for key in [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ] {
        if std::env::var_os(key).is_some_and(|v| same_path(Path::new(&v), folder)) {
            return Ok(true);
        }
    }
    #[cfg(target_os = "linux")]
    for entry in std::fs::read_dir("/proc")?.flatten() {
        if entry.file_name().to_string_lossy().parse::<u32>().is_ok() {
            if let Ok(bytes) = std::fs::read(entry.path().join("environ")) {
                if env_uses(&bytes, folder) {
                    return Ok(true);
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // ps exposes other same-user process environments on macOS, where /proc is absent.
        let out = Command::new("ps")
            .args(["eww", "-axo", "command="])
            .output()?;
        anyhow::ensure!(out.status.success(), "Cannot inspect live process accounts");
        let text = String::from_utf8_lossy(&out.stdout);
        if ps_uses(&text, folder) {
            return Ok(true);
        }
    }
    // Registered agents can outlive their launching process. Consult their
    // configuration selectors as well, including tilde paths from worker specs.
    use crate::store::KnownRepoStore;
    let host = home.join(".cas");
    let mut roots = vec![host.clone()];
    if host
        .join("cas.db")
        .try_exists()
        .map_err(repo_inspection_error)?
    {
        // Older hosts can have an agent database predating the repo registry.
        // Only an absent table means "no registered repos"; schema/open/list
        // failures still refuse the mutation, without changing the host schema.
        let conn = cas_store::shared_db::shared_connection(&host.join("cas.db"))
            .map_err(repo_inspection_error)?;
        let has_repos: bool = conn
            .lock()
            .map_err(repo_inspection_error)?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'known_repos')",
                [],
                |row| row.get(0),
            )
            .map_err(repo_inspection_error)?;
        if has_repos {
            let store =
                crate::store::SqliteKnownRepoStore::open(&host).map_err(repo_inspection_error)?;
            let repos = store.list().map_err(repo_inspection_error)?;
            roots.extend(repos.into_iter().map(|r| r.path.join(".cas")));
        }
    }
    if let Ok(root) = crate::store::find_cas_root() {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    for root in roots.into_iter().filter(|r| r.join("cas.db").is_file()) {
        if registered_account_in_use(&root, folder, home).map_err(|error| {
            tracing::debug!(%error, "Cannot inspect registered agents");
            anyhow::anyhow!("Cannot inspect registered agents; account kept\nRun: cas update")
        })? {
            return Ok(true);
        }
    }
    Ok(false)
}

// Account guards only need status and metadata, which predate the host repo
// registry. AgentStore::list also selects newer liveness/session columns and
// cannot read those older schemas. Do not migrate a host DB just to remove an
// idle account; inspect the relevant projection without mutating its schema.
fn registered_account_in_use(root: &Path, folder: &Path, home: &Path) -> Result<bool> {
    let shared = cas_store::shared_db::shared_connection(&root.join("cas.db"))?;
    let conn = shared
        .lock()
        .map_err(|_| anyhow::anyhow!("Agent database lock poisoned"))?;
    let mut schema = conn.prepare("PRAGMA table_info(agents)")?;
    let columns = schema
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if !["status", "metadata"].iter().all(|required| {
        columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(required))
    }) {
        // No agent table/selectors on this host. Live env/process checks have
        // already run; a missing optional schema is not a database failure.
        return Ok(false);
    }
    let mut statement = conn.prepare("SELECT metadata FROM agents WHERE status <> 'shutdown'")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let metadata: std::collections::HashMap<String, String> = serde_json::from_str(&row?)?;
        for key in [
            "worker_account_dir",
            "config_dir",
            "claude_config_dir",
            "codex_home",
        ] {
            if let Some(value) = metadata.get(key) {
                let path = value
                    .strip_prefix("~/")
                    .map(|v| home.join(v))
                    .unwrap_or_else(|| PathBuf::from(value));
                if same_path(&path, folder) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Config writes preserve unrelated keys, including newer selection policy.
/// The path is resolved from the caller's HOME/XDG_CONFIG_HOME, never a project.
fn policy_path(home: &Path) -> PathBuf {
    if dirs::home_dir().as_deref() == Some(home) {
        if let Some(dir) = crate::config::global_cas_dir() {
            return dir.join("config.toml");
        }
    }
    home.join(".config/cas/config.toml")
}

fn read_policy(home: &Path) -> Result<toml::Value> {
    let path = policy_path(home);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            toml::from_str(&text).with_context(|| format!("Cannot read {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(toml::Value::Table(Default::default()))
        }
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn disabled(home: &Path, folder: &Path) -> bool {
    read_policy(home)
        .ok()
        .and_then(|v| v.get("accounts")?.get("disabled")?.as_array().cloned())
        .is_some_and(|values| {
            values
                .iter()
                .filter_map(|v| v.as_str())
                .any(|v| same_path(Path::new(v), folder))
        })
}

pub(crate) fn set_disabled(home: &Path, folder: &Path, disabled: bool) -> Result<()> {
    // Store the physical account; legacy alias entries still match on read and
    // are removed together when either spelling is enabled.
    let folder = folder
        .canonicalize()
        .unwrap_or_else(|_| folder.to_path_buf());
    let mut policy = read_policy(home)?;
    let accounts = policy
        .get("accounts")
        .map(|value| value.as_table().context("accounts must be a TOML table"))
        .transpose()?;
    let list = accounts
        .and_then(|table| table.get("disabled"))
        .map(|value| {
            value
                .as_array()
                .context("accounts.disabled must be an array")
        })
        .transpose()?;
    let current = list.is_some_and(|list| {
        list.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| same_path(Path::new(value), &folder))
        })
    });
    if !current && !disabled {
        return Ok(());
    }
    let table = policy
        .as_table_mut()
        .context("Config must be a TOML table")?;
    let accounts = table
        .entry("accounts")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .context("accounts must be a TOML table")?;
    let list = accounts
        .entry("disabled")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .context("accounts.disabled must be an array")?;
    list.retain(|v| v.as_str().is_none_or(|v| !same_path(Path::new(v), &folder)));
    if disabled {
        list.push(toml::Value::String(folder.to_string_lossy().into_owned()));
    }
    let path = policy_path(home);
    std::fs::create_dir_all(path.parent().unwrap())?;
    use std::io::Write;
    let mut replacement = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    replacement.write_all(toml::to_string_pretty(&policy)?.as_bytes())?;
    replacement.as_file().sync_all()?;
    replacement.persist(&path)?;
    Ok(())
}

pub(crate) fn remove(target: &Target, home: &Path) -> Result<()> {
    remove_with(target, home, cfg!(target_os = "macos"), &mut security)
}

/// Incoming profile links are part of a physical account's removal plan.
/// Removing a link itself only unlinks that alias.
pub(crate) fn removal_aliases(target: &Target, home: &Path) -> Result<Vec<PathBuf>> {
    if target.folder.is_symlink() {
        return Ok(Vec::new());
    }
    let mut aliases = Vec::new();
    for dir in accounts::discover_aliases(target.provider, home)? {
        if dir.path.is_symlink() && same_path(&dir.path, &target.folder) {
            anyhow::ensure!(
                dir.path != home.join(format!(".{}", target.provider.name())),
                "The main account is protected"
            );
            aliases.push(dir.path);
        }
    }
    aliases.sort();
    Ok(aliases)
}

fn remove_with(
    target: &Target,
    home: &Path,
    macos: bool,
    run: &mut impl FnMut(&[String], Option<&str>) -> Result<Output>,
) -> Result<()> {
    ensure_idle(target, home)?;
    read_policy(home)?;
    let aliases = removal_aliases(target, home)?;
    let is_alias = std::fs::symlink_metadata(&target.folder)?
        .file_type()
        .is_symlink();
    // Resolve identities while every path still exists (HOME itself may be a
    // symlink). After deletion, canonicalize cannot recover these policy keys.
    let canonical_paths: Vec<_> = std::iter::once(&target.folder)
        .chain(&aliases)
        .map(|path| path.canonicalize())
        .collect::<std::io::Result<_>>()?;
    // Normalize legacy alias policy entries before their spelling disappears.
    if disabled(home, &target.folder) {
        set_disabled(home, &target.folder, true)?;
    }
    // remove_dir_all unlinks symlinks within the tree, never traverses them.
    if is_alias {
        std::fs::remove_file(&target.folder)?;
    } else {
        std::fs::remove_dir_all(&target.folder)?;
        for alias in &aliases {
            std::fs::remove_file(alias)?;
        }
    }
    // Once the directory is gone, a failed Keychain deletion must still leave
    // policy truthful. Report both failures rather than losing the remedy.
    let credentials = if macos && target.provider == Provider::Claude {
        let mut failure = None;
        for folder in std::iter::once(&target.folder).chain(&aliases) {
            if let Err(error) = keychain_delete(folder, run) {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    } else {
        Ok(())
    };
    let policy = (|| -> Result<()> {
        set_disabled(home, &target.folder, false)?;
        for alias in &aliases {
            set_disabled(home, alias, false)?;
        }
        // Unlinking an alias must retain the physical login's disabled state.
        if !is_alias {
            for path in &canonical_paths {
                set_disabled(home, path, false)?;
            }
        }
        Ok(())
    })();
    match (credentials, policy) {
        (Err(error), Err(policy)) => {
            Err(error.context(format!("Cannot update account policy: {policy:#}")))
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

pub(crate) fn rename(target: &Target, home: &Path, name: &str) -> Result<()> {
    ensure_idle(target, home)?;
    read_policy(home)?;
    anyhow::ensure!(
        name == name.trim(),
        "Account name cannot contain whitespace"
    );
    crate::cli::account_picker::validate_new_profile_name(name, "main")
        .map_err(anyhow::Error::msg)?;
    let to = home.join(format!("{}{name}", target.provider.prefix()));
    anyhow::ensure!(
        std::fs::symlink_metadata(&to).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "Account folder already exists: {}",
        to.display()
    );
    let old_canonical = target.folder.canonicalize()?;
    let was_disabled = disabled(home, &target.folder);
    if was_disabled {
        set_disabled(home, &target.folder, true)?;
    }
    std::fs::rename(&target.folder, &to)?;
    if cfg!(target_os = "macos") && target.provider == Provider::Claude {
        if let Err(error) = keychain_rekey(&target.folder, &to, &mut security) {
            std::fs::rename(&to, &target.folder)
                .context("Keychain migration failed and folder rollback failed")?;
            return Err(error);
        }
    }
    if was_disabled && old_canonical != to.canonicalize()? {
        set_disabled(home, &to, true)?;
        set_disabled(home, &target.folder, false)?;
        set_disabled(home, &old_canonical, false)?;
    }
    Ok(())
}

fn security(args: &[String], input: Option<&str>) -> Result<Output> {
    use std::io::Write;
    use std::process::Stdio;
    let mut command = Command::new("security");
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .context("Cannot run macOS Keychain security tool")?;
    if let Some(input) = input {
        let write = child
            .stdin
            .take()
            .context("Missing security stdin")?
            .write_all(input.as_bytes());
        if let Err(error) = write {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
    }
    Ok(child.wait_with_output()?)
}
fn service(folder: &Path) -> String {
    crate::claude_credentials::keychain_service(folder.to_str())
}
fn keychain_delete(
    folder: &Path,
    run: &mut impl FnMut(&[String], Option<&str>) -> Result<Output>,
) -> Result<()> {
    let service = service(folder);
    let remedy = format!(
        "Cannot delete account Keychain item\nRun: security delete-generic-password -s '{service}'"
    );
    let out = run(
        &["delete-generic-password".into(), "-s".into(), service],
        None,
    )
    .map_err(|error| {
        tracing::debug!(%error, "Cannot delete account Keychain item");
        anyhow::anyhow!("{remedy}")
    })?;
    anyhow::ensure!(
        out.status.success() || out.status.code() == Some(44),
        "{remedy}"
    );
    Ok(())
}
fn keychain_account() -> String {
    // Claude Code 2.1.311 uses USER (or the OS username), restricted to this alphabet.
    let user = std::env::var("USER")
        .ok()
        .filter(|user| !user.is_empty())
        .or_else(|| {
            let output = Command::new("/usr/bin/id").arg("-un").output().ok()?;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        })
        .unwrap_or_else(|| "claude-code-user".into());
    if !user.is_empty()
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    {
        user
    } else {
        "claude-code-user".into()
    }
}
fn keychain_rekey(
    from: &Path,
    to: &Path,
    run: &mut impl FnMut(&[String], Option<&str>) -> Result<Output>,
) -> Result<()> {
    let out = run(
        &[
            "find-generic-password".into(),
            "-s".into(),
            service(from),
            "-w".into(),
        ],
        None,
    )?;
    if out.status.code() == Some(44) {
        return Ok(());
    }
    anyhow::ensure!(out.status.success(), "Cannot read account Keychain item");
    let secret = String::from_utf8(out.stdout).context("Keychain item is not UTF-8")?;
    // Match Claude's stdin protocol so the secret is never visible in argv.
    let input = format!(
        "add-generic-password -U -a \"{}\" -s \"{}\" -X \"{}\"\n",
        keychain_account(),
        service(to),
        hex::encode(secret.trim_end_matches('\n').as_bytes())
    );
    let out = run(&["-i".into()], Some(&input))?;
    anyhow::ensure!(
        out.status.success(),
        "Cannot write renamed account Keychain item"
    );
    keychain_delete(from, run)
}

#[cfg(unix)]
static LOGIN_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
#[cfg(unix)]
static LOGIN_SIGNAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
extern "C" fn record_login_signal(signal: libc::c_int) {
    // Only an atomic store in signal context. Filesystem and child cleanup run
    // in ordinary control flow, with interception held until the tempdir drops.
    let _ = LOGIN_SIGNAL.compare_exchange(
        0,
        signal,
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
    );
}

struct LoginSignals {
    #[cfg(unix)]
    previous: Vec<(libc::c_int, libc::sigaction)>,
    #[cfg(unix)]
    _exclusive: std::sync::MutexGuard<'static, ()>,
}

impl LoginSignals {
    fn enter() -> Result<Self> {
        #[cfg(unix)]
        {
            let mut guard = Self {
                previous: Vec::new(),
                _exclusive: LOGIN_SIGNAL_LOCK.lock().unwrap_or_else(|p| p.into_inner()),
            };
            LOGIN_SIGNAL.store(0, std::sync::atomic::Ordering::Relaxed);
            for signal in [libc::SIGINT, libc::SIGTERM] {
                let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
                if unsafe { libc::sigaction(signal, std::ptr::null(), &mut previous) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                // Preserve a caller's deliberate ignored disposition.
                if previous.sa_sigaction == libc::SIG_IGN {
                    continue;
                }
                let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                action.sa_sigaction = record_login_signal as *const () as libc::sighandler_t;
                unsafe { libc::sigemptyset(&mut action.sa_mask) };
                if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                guard.previous.push((signal, previous));
            }
            Ok(guard)
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    fn check(&self) -> Result<()> {
        #[cfg(unix)]
        anyhow::ensure!(
            LOGIN_SIGNAL.load(std::sync::atomic::Ordering::Relaxed) == 0,
            "Login interrupted; account not added"
        );
        Ok(())
    }

    fn answer(&self) -> Result<String> {
        // Use the shared stdin buffer, including bytes prefetched by an earlier
        // prompt. read_line retries EINTR, so read on a separate thread while
        // the login owner remains able to notice cancellation and clean up.
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("login-answer".into())
            .spawn(move || {
                let mut answer = String::new();
                let result = std::io::stdin().read_line(&mut answer).map(|_| answer);
                let _ = send.send(result);
            })?;
        loop {
            self.check()?;
            match receive.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(answer) => {
                    self.check()?;
                    return Ok(answer?);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => return Err(error.into()),
            }
        }
        // On cancellation the reader may remain blocked; LoginSignals::drop
        // restores and re-raises the signal after NewLogin's folder is cleaned.
    }
}

impl Drop for LoginSignals {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            for (signal, previous) in self.previous.iter().rev() {
                unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
            }
            let signal = LOGIN_SIGNAL.load(std::sync::atomic::Ordering::Relaxed);
            if signal != 0 {
                // TempDir and its lease have already dropped. Preserve the
                // process's death-by-signal contract after completing cleanup.
                unsafe {
                    libc::raise(signal);
                    libc::_exit(128 + signal);
                }
            }
        }
    }
}

#[cfg(unix)]
fn open_login_dir(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

/// Sweep abandoned transactions only. A directory flock keeps a still-running
/// login safe even when the user leaves its auth prompt open for over an hour.
pub(crate) fn sweep_stale_logins(home: &Path) {
    let warn = |folder: &Path, error: anyhow::Error| {
        eprintln!(
            "Warning: stale login cleanup failed; continuing login\nFolder: {}\nCause: {error:#}",
            folder.display()
        );
    };
    let entries = match std::fs::read_dir(home) {
        Ok(entries) => entries,
        Err(error) => {
            warn(home, error.into());
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn(home, error.into());
                continue;
            }
        };
        if !Provider::ALL.iter().any(|p| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{}login.tmp.", p.prefix()))
        }) {
            continue;
        }
        if let Err(error) = sweep_stale_login(&entry.path()) {
            warn(&entry.path(), error);
        }
    }
}

fn sweep_stale_login(folder: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(folder) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !metadata.is_dir()
        || !metadata
            .modified()?
            .elapsed()
            .is_ok_and(|age| age > std::time::Duration::from_secs(3600))
    {
        return Ok(());
    }
    #[cfg(unix)]
    let _lease = {
        let file = match open_login_dir(folder) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        file
    };
    match std::fs::remove_dir_all(folder) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    }
    if cfg!(target_os = "macos")
        && folder.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with(Provider::Claude.prefix())
        })
    {
        keychain_delete(folder, &mut security)?;
    }
    Ok(())
}

/// RAII cleanup for every early return, provider failure and cancelled login.
/// The sibling's .tmp segment is hidden by NON_ACCOUNT_SUFFIXES.
pub(crate) struct NewLogin {
    home: PathBuf,
    provider: Provider,
    dir: tempfile::TempDir,
    #[cfg(unix)]
    _lease: std::fs::File,
    // Fields drop in declaration order: restore signals only after dir cleanup.
    signals: LoginSignals,
}
impl NewLogin {
    pub(crate) fn begin(home: &Path, provider: Provider, typed: &str) -> Result<Self> {
        anyhow::ensure!(
            typed == typed.trim(),
            "Account name cannot contain whitespace"
        );
        crate::cli::account_picker::validate_new_profile_name(typed, "main")
            .map_err(anyhow::Error::msg)?;
        let signals = LoginSignals::enter()?;
        let dir = tempfile::Builder::new()
            .prefix(&format!("{}login.tmp.", provider.prefix()))
            .tempdir_in(home)?;
        #[cfg(unix)]
        let lease = {
            let file = open_login_dir(dir.path())?;
            fs2::FileExt::lock_exclusive(&file)?;
            file
        };
        Ok(Self {
            home: home.into(),
            provider,
            dir,
            #[cfg(unix)]
            _lease: lease,
            signals,
        })
    }
    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }
    pub(crate) fn run(&self, command: &mut Command) -> Result<std::process::ExitStatus> {
        self.signals.check()?;
        let mut child = command.spawn()?;
        let result = (|| {
            loop {
                self.signals.check()?;
                if let Some(status) = child.try_wait()? {
                    self.signals.check()?;
                    return Ok(status);
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        })();
        if result.is_err() {
            // Only our auth child: no host-wide process-name or group kills.
            let _ = child.kill();
            let _ = child.wait();
        }
        result
    }
    pub(crate) fn finish(self, typed: &str) -> Result<String> {
        self.signals.check()?;
        let email = self.provider.login_email(&self.home, self.path());
        let name = email.as_deref().unwrap_or(typed);
        crate::cli::account_picker::validate_new_profile_name(name, "main")
            .map_err(anyhow::Error::msg)?;
        let pending =
            crate::account_registry::account(self.provider, &self.home, self.path().to_path_buf());
        let existing =
            crate::account_registry::discover_with(self.provider, &self.home, false, |_, _| {
                crate::account_registry::LoginState::Unknown
            })?
            .into_iter()
            .find(|account| {
                let strong = |a: &crate::account_registry::Account| {
                    a.identity
                        .as_ref()
                        .is_some_and(|id| id.account_id.is_some())
                };
                if strong(account) && strong(&pending) {
                    account.identity == pending.identity
                } else {
                    account
                        .email
                        .as_deref()
                        .zip(email.as_deref())
                        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
                }
            });
        if let Some(existing) = existing {
            eprintln!(
                "{} account {} already exists at {}",
                self.provider.name(),
                name,
                existing.folder.display()
            );
            use std::io::Write;
            eprint!("Keep the existing account? [y/N] ");
            std::io::stderr().flush()?;
            let answer = self.signals.answer()?;
            if answer.trim().eq_ignore_ascii_case("y") || answer.trim().eq_ignore_ascii_case("yes")
            {
                return Ok(existing.name);
            }
            bail!("Login cancelled; existing account kept");
        }
        let mut final_name = name.to_owned();
        let mut to = self
            .home
            .join(format!("{}{final_name}", self.provider.prefix()));
        let mut ordinal = 2;
        loop {
            match std::fs::symlink_metadata(&to) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(e.into()),
                Ok(_) if email.is_some() => {
                    final_name = format!("{name}-{ordinal}");
                    ordinal += 1;
                    to = self
                        .home
                        .join(format!("{}{final_name}", self.provider.prefix()));
                }
                Ok(_) => bail!("Account folder already exists: {}", to.display()),
            }
        }
        self.signals.check()?;
        std::fs::rename(self.path(), &to)?;
        if cfg!(target_os = "macos") && self.provider == Provider::Claude {
            if let Err(error) = keychain_rekey(self.path(), &to, &mut security) {
                std::fs::rename(&to, self.path())
                    .context("Keychain migration failed and login rollback failed")?;
                return Err(error);
            }
        }
        eprintln!("[OK] Added {} account {}", self.provider.name(), final_name);
        Ok(final_name)
    }
}
impl Drop for NewLogin {
    fn drop(&mut self) {
        if cfg!(target_os = "macos") && self.provider == Provider::Claude {
            let _ = keychain_delete(self.path(), &mut security);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn remove_keeps_credentials_until_folder_deletion_and_reports_keychain_remedy() {
        use std::os::unix::process::ExitStatusExt;
        let env = crate::test_support::TestEnvGuard::temp_home();
        let target = Target {
            provider: Provider::Claude,
            name: "idle".into(),
            folder: env.home().join(".claude-idle"),
            email: None,
        };
        // A regular file makes remove_dir_all fail deterministically, even as root.
        std::fs::write(&target.folder, "cannot remove as directory").unwrap();
        let mut calls = 0;
        assert!(
            remove_with(&target, env.home(), true, &mut |_, _| {
                calls += 1;
                Ok(Output {
                    status: std::process::ExitStatus::from_raw(0),
                    stdout: vec![],
                    stderr: vec![],
                })
            })
            .is_err()
        );
        assert_eq!(
            calls, 0,
            "folder failure must leave the Keychain item intact"
        );
        std::fs::remove_file(&target.folder).unwrap();
        std::fs::create_dir(&target.folder).unwrap();
        let error = remove_with(&target, env.home(), true, &mut |args, _| {
            assert!(
                !target.folder.exists(),
                "delete the folder before touching credentials"
            );
            assert_eq!(
                args,
                &["delete-generic-password", "-s", &service(&target.folder)]
            );
            Ok(Output {
                status: std::process::ExitStatus::from_raw(1 << 8),
                stdout: vec![],
                stderr: vec![],
            })
        })
        .unwrap_err();
        assert!(
            format!("{error:#}").contains(&format!(
                "security delete-generic-password -s '{}'",
                service(&target.folder)
            )),
            "{error:#}"
        );
    }

    #[test]
    fn duplicate_answer_reads_input_buffered_by_an_earlier_prompt() {
        use std::io::{BufRead, Write};
        const CHILD: &str = "CAS_TEST_BUFFERED_DUPLICATE_ANSWER";
        if std::env::var_os(CHILD).is_some() {
            let mut first = String::new();
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            input.read_line(&mut first).unwrap();
            assert_eq!(first, "earlier\n");
            assert_eq!(input.fill_buf().unwrap(), b"y\n", "answer must be buffered");
            drop(input);
            let signals = LoginSignals::enter().unwrap();
            assert_eq!(signals.answer().unwrap().trim(), "y");
            return;
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "account_lifecycle::tests::duplicate_answer_reads_input_buffered_by_an_earlier_prompt", "--nocapture"])
            .env(CHILD, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"earlier\ny\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Historical layouts from independent QA's qa_legacy_split.py. These
    // intentionally predate the modern SqliteAgentStore::init baseline.
    fn legacy_agent_db(root: &Path, with_pid_starttime: bool) -> rusqlite::Connection {
        std::fs::create_dir_all(root).unwrap();
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        let mut columns = "id TEXT PRIMARY KEY, name TEXT NOT NULL, agent_type TEXT NOT NULL DEFAULT 'primary', \
            role TEXT NOT NULL DEFAULT 'standard', status TEXT NOT NULL DEFAULT 'active', pid INTEGER, ppid INTEGER, \
            cc_session_id TEXT, parent_id TEXT, machine_id TEXT, registered_at TEXT NOT NULL, last_heartbeat TEXT NOT NULL, \
            active_tasks INTEGER NOT NULL DEFAULT 0, metadata TEXT NOT NULL DEFAULT '{}', startup_confirmed INTEGER NOT NULL DEFAULT 0".to_owned();
        if with_pid_starttime {
            columns.push_str(", pid_starttime INTEGER");
        }
        conn.execute_batch(&format!("CREATE TABLE agents ({columns})"))
            .unwrap();
        conn
    }

    #[test]
    fn remove_refuses_unreadable_known_repos() {
        use crate::store::{AgentStore, SqliteAgentStore};
        for schema in [
            "missing",
            "pre-m199",
            "pre-m204",
            "no-agents",
            "corrupt",
            "malformed",
        ] {
            let env = crate::test_support::TestEnvGuard::temp_home();
            let root = env.home().join(".cas");
            if schema == "corrupt" {
                std::fs::write(root.join("cas.db"), "not a sqlite database").unwrap();
            } else if schema.starts_with("pre-") {
                legacy_agent_db(&root, schema == "pre-m204");
            } else if schema == "no-agents" {
                rusqlite::Connection::open(root.join("cas.db")).unwrap();
            } else {
                SqliteAgentStore::open(&root).unwrap().init().unwrap();
                if schema == "malformed" {
                    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
                    conn.execute_batch("CREATE TABLE known_repos (unexpected TEXT)")
                        .unwrap();
                }
            }
            let target = Target {
                provider: Provider::Codex,
                name: "idle".into(),
                folder: env.home().join(".codex-idle"),
                email: None,
            };
            std::fs::create_dir(&target.folder).unwrap();
            let result = remove(&target, env.home());
            if matches!(schema, "missing" | "pre-m199" | "pre-m204" | "no-agents") {
                result.unwrap_or_else(|e| {
                    panic!("{schema}: legacy idle removal must succeed: {e:#}")
                });
                assert!(!target.folder.exists());
                continue;
            }
            assert!(
                result.is_err(),
                "{schema}: unreadable repo list must refuse removal"
            );
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.contains("known repositories"));
            // pin: the final line must remain a copyable diagnostic command.
            assert!(error.ends_with("\nRun: cas update"));
            assert!(target.folder.is_dir());
        }
    }

    #[test]
    fn remove_enabled_account_preserves_policy_bytes_and_absence() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let target = Target {
            provider: Provider::Codex,
            name: "idle".into(),
            folder: env.home().join(".codex-idle"),
            email: None,
        };
        let path = policy_path(env.home());
        std::fs::create_dir(&target.folder).unwrap();
        remove(&target, env.home()).unwrap();
        assert!(!path.exists(), "a no-op must not create config.toml");
        for text in [
            "# preserve this comment\n[accounts]\nselection = 'manual'\n",
            "# empty disabled list\n[accounts]\ndisabled = [ ]\n",
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            std::fs::create_dir(&target.folder).unwrap();
            remove(&target, env.home()).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
        }
    }

    #[cfg(unix)]
    #[test]
    fn keychain_migration_refuses_before_reading_any_secret() {
        let result = keychain_rekey(Path::new("/fixture/.claude-old"),
            Path::new("/fixture/.claude-new"), &mut |_, _| {
                panic!("Cassy must never read or rewrite Claude Keychain secrets")
            });
        assert!(result.is_err());
    }

    #[test]
    fn process_selectors_distinguish_subscription_suffixes() {
        let folder = Path::new("/fixture home/.claude-email@example.com");
        for (text, expected) in [
            (
                "claude CLAUDE_CONFIG_DIR=/fixture home/.claude-email@example.com OTHER=x",
                true,
            ),
            (
                "claude CLAUDE_CONFIG_DIR=/fixture home/.claude-email@example.com-2 OTHER=x",
                false,
            ),
            (
                "claude NOT_CLAUDE_CONFIG_DIR=/fixture home/.claude-email@example.com",
                false,
            ),
        ] {
            assert_eq!(ps_uses(text, folder), expected, "{text}");
        }
    }

    #[test]
    fn legacy_registered_account_stays_protected_without_modern_columns() {
        for with_pid_starttime in [false, true] {
            let env = crate::test_support::TestEnvGuard::temp_home();
            let conn = legacy_agent_db(&env.home().join(".cas"), with_pid_starttime);
            let folder = env.home().join(".codex-busy");
            std::fs::create_dir(&folder).unwrap();
            conn.execute(
                "INSERT INTO agents (id,name,registered_at,last_heartbeat,metadata) VALUES ('holder','holder','2026-01-01','2026-01-01',?1)",
                [r#"{"worker_account_dir":"~/.codex-busy"}"#],
            ).unwrap();
            let target = Target {
                provider: Provider::Codex,
                name: "busy".into(),
                folder: folder.clone(),
                email: None,
            };
            let error = ensure_idle(&target, env.home()).unwrap_err().to_string();
            assert!(error.contains("is in use"), "{error}");
            assert!(folder.is_dir());
            conn.execute("UPDATE agents SET status='shutdown'", [])
                .unwrap();
            remove(&target, env.home()).unwrap();
            assert!(!folder.exists());
        }
    }

    #[test]
    fn registered_factory_account_is_busy_without_a_live_process() {
        use crate::store::{AgentStore, SqliteAgentStore};
        let env = crate::test_support::TestEnvGuard::temp_home();
        let root = env.home().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let store = SqliteAgentStore::open(&root).unwrap();
        store.init().unwrap();
        use crate::store::KnownRepoStore;
        crate::store::SqliteKnownRepoStore::open(&root)
            .unwrap()
            .init()
            .unwrap();
        let mut agent = cas_types::Agent::new("account-holder".into(), "account-holder".into());
        let folder = env.home().join(".codex-busy");
        std::fs::create_dir(&folder).unwrap();
        agent
            .metadata
            .insert("worker_account_dir".into(), "~/.codex-busy".into());
        store.register(&agent).unwrap();
        assert!(in_use(&folder, env.home()).unwrap());
        agent.status = cas_types::AgentStatus::Shutdown;
        store.update(&agent).unwrap();
        assert!(!in_use(&folder, env.home()).unwrap());
    }
}
