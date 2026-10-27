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
        for dir in accounts::discover(p, home)? {
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
        if [
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
        ]
        .iter()
        .any(|key| text.contains(&format!("{key}={}", folder.display())))
        {
            return Ok(true);
        }
    }
    // Registered agents can outlive their launching process. Consult their
    // configuration selectors as well, including tilde paths from worker specs.
    use crate::store::KnownRepoStore;
    let host = home.join(".cas");
    let mut roots = vec![host.clone()];
    if host.join("cas.db").is_file() {
        if let Ok(store) = crate::store::SqliteKnownRepoStore::open(&host) {
            if let Ok(repos) = store.list() {
                roots.extend(repos.into_iter().map(|r| r.path.join(".cas")));
            }
        }
    }
    if let Ok(root) = crate::store::find_cas_root() {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    for root in roots.into_iter().filter(|r| r.join("cas.db").is_file()) {
        use crate::store::AgentStore;
        let store = crate::store::SqliteAgentStore::open(&root)?;
        for agent in store.list(None)? {
            if matches!(agent.status, cas_types::AgentStatus::Shutdown) {
                continue;
            }
            for key in [
                "worker_account_dir",
                "config_dir",
                "claude_config_dir",
                "codex_home",
            ] {
                if let Some(value) = agent.metadata.get(key) {
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
                .any(|v| Path::new(v) == folder)
        })
}

pub(crate) fn set_disabled(home: &Path, folder: &Path, disabled: bool) -> Result<()> {
    let mut policy = read_policy(home)?;
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
    list.retain(|v| v.as_str().is_none_or(|v| Path::new(v) != folder));
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
    ensure_idle(target, home)?;
    read_policy(home)?;
    if cfg!(target_os = "macos") && target.provider == Provider::Claude {
        keychain_delete(&target.folder, &mut security)?;
    }
    // remove_dir_all unlinks symlinks within the tree, never traverses them.
    if std::fs::symlink_metadata(&target.folder)?
        .file_type()
        .is_symlink()
    {
        std::fs::remove_file(&target.folder)?;
    } else {
        std::fs::remove_dir_all(&target.folder)?;
    }
    set_disabled(home, &target.folder, false)
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
    std::fs::rename(&target.folder, &to)?;
    if cfg!(target_os = "macos") && target.provider == Provider::Claude {
        if let Err(error) = keychain_rekey(&target.folder, &to, &mut security) {
            std::fs::rename(&to, &target.folder)
                .context("Keychain migration failed and folder rollback failed")?;
            return Err(error);
        }
    }
    if disabled(home, &target.folder) {
        set_disabled(home, &to, true)?;
        set_disabled(home, &target.folder, false)?;
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
    // TODO(cas-3870): use crate::claude_credentials::keychain_service when its epic reaches this base.
    crate::capability::claude_keychain_service(folder.to_str())
}
fn keychain_delete(
    folder: &Path,
    run: &mut impl FnMut(&[String], Option<&str>) -> Result<Output>,
) -> Result<()> {
    let out = run(
        &[
            "delete-generic-password".into(),
            "-s".into(),
            service(folder),
        ],
        None,
    )?;
    anyhow::ensure!(
        out.status.success() || out.status.code() == Some(44),
        "Cannot delete account Keychain item"
    );
    Ok(())
}
fn keychain_account() -> String {
    // Claude Code 2.1.311 uses USER (or the OS username), restricted to this alphabet.
    let user = std::env::var("USER").unwrap_or_else(|_| "claude-code-user".into());
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

/// RAII cleanup for every early return, provider failure and cancelled login.
/// The sibling's .tmp segment is hidden by NON_ACCOUNT_SUFFIXES.
pub(crate) struct NewLogin {
    home: PathBuf,
    provider: Provider,
    dir: tempfile::TempDir,
}
impl NewLogin {
    pub(crate) fn begin(home: &Path, provider: Provider, typed: &str) -> Result<Self> {
        anyhow::ensure!(
            typed == typed.trim(),
            "Account name cannot contain whitespace"
        );
        crate::cli::account_picker::validate_new_profile_name(typed, "main")
            .map_err(anyhow::Error::msg)?;
        let dir = tempfile::Builder::new()
            .prefix(&format!("{}login.tmp.", provider.prefix()))
            .tempdir_in(home)?;
        Ok(Self {
            home: home.into(),
            provider,
            dir,
        })
    }
    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }
    pub(crate) fn finish(self, typed: &str) -> Result<String> {
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
                // TODO(cas-5952): typed AccountIdentity uses account_id.is_none() for fallback.
                let strong = |a: &crate::account_registry::Account| {
                    a.identity
                        .as_deref()
                        .is_some_and(|id| !id.starts_with("email:"))
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
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
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
    fn keychain_lifecycle_uses_named_services_and_preserves_old_item_on_write_failure() {
        use std::os::unix::process::ExitStatusExt;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("USER", "fixture");
        let from = Path::new("/Users/fixture/.claude-old@example.com");
        let to = Path::new("/Users/fixture/.claude-new@example.com");
        let old = "Claude Code-credentials-0be5f251";
        for fail_write in [false, true] {
            let mut calls = Vec::new();
            let result = keychain_rekey(from, to, &mut |args, input| {
                calls.push((args.to_vec(), input.map(str::to_owned)));
                Ok(Output {
                    status: std::process::ExitStatus::from_raw(if fail_write && args[0] == "-i" {
                        256
                    } else {
                        0
                    }),
                    stdout: b"fixture-secret\n".to_vec(),
                    stderr: Vec::new(),
                })
            });
            assert_eq!(result.is_err(), fail_write);
            assert_eq!(
                calls[0],
                (
                    ["find-generic-password", "-s", old, "-w"]
                        .map(str::to_owned)
                        .to_vec(),
                    None
                )
            );
            assert_eq!(calls[1].0, ["-i"]);
            assert_eq!(
                calls[1].1.as_deref(),
                Some(
                    "add-generic-password -U -a \"fixture\" -s \"Claude Code-credentials-ef91b6f7\" -X \"666978747572652d736563726574\"\n"
                )
            );
            if fail_write {
                assert_eq!(calls.len(), 2, "old item must survive failed writes");
            } else {
                assert_eq!(
                    calls[2],
                    (
                        ["delete-generic-password", "-s", old]
                            .map(str::to_owned)
                            .to_vec(),
                        None
                    )
                );
            }
        }
        let mut calls = Vec::new();
        keychain_delete(from, &mut |args, input| {
            assert!(input.is_none());
            calls.push(args.to_vec());
            Ok(Output {
                status: std::process::ExitStatus::from_raw(44 << 8),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        })
        .unwrap();
        assert_eq!(calls, vec![vec!["delete-generic-password", "-s", old]]);
    }

    #[test]
    fn registered_factory_account_is_busy_without_a_live_process() {
        use crate::store::{AgentStore, SqliteAgentStore};
        let env = crate::test_support::TestEnvGuard::temp_home();
        let root = env.home().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let store = SqliteAgentStore::open(&root).unwrap();
        store.init().unwrap();
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
