//! Shared-link health of one account folder (cas-05cee).
//!
//! `cas accounts link` points every account's shared items (skills, agents,
//! hooks, settings, ...) at one base per provider. A session started on a
//! folder that lost those links runs without them, and a provider recreates
//! a retired default folder (`~/.codex`) the moment anything launches there.
//! Doctor and every launch path use this module: creating a link in an empty
//! slot is always safe and automatic; replacing a real file, folder or other
//! link goes through `cas accounts link`'s backup and restore script.
//! Credentials and other private entries are never in a shared slot, so they
//! are never read for linking or touched.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::accounts::{self, Provider};

/// What a folder's shared slots hold relative to its provider's base.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LinkStatus {
    /// Empty slots whose base item exists: linking them is safe.
    pub missing: Vec<String>,
    /// Slots holding a real file, folder or a link elsewhere: replacing one
    /// needs the backed-up `cas accounts link` path.
    pub conflicts: Vec<String>,
    /// This folder is a retired account folder that came back.
    pub retired: Option<Retired>,
}

impl LinkStatus {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.conflicts.is_empty() && self.retired.is_none()
    }
}

/// Why a folder `cas accounts link` would retire exists again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Retired {
    pub reason: String,
    /// The named account with the same login, when there is one.
    pub canonical: Option<PathBuf>,
}

/// One launch or repair check: what was linked and what still needs a person.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Ensured {
    pub linked: Vec<String>,
    pub status: LinkStatus,
}

/// Read-only. Nothing to report until the provider has a base, so an operator
/// who never ran `cas accounts link` sees nothing.
pub fn inspect(provider: Provider, home: &Path, folder: &Path) -> LinkStatus {
    let base = provider.base_dir(home);
    let mut status = LinkStatus::default();
    if !base.is_dir() {
        return status;
    }
    status.retired = retired(provider, home, folder);
    for item in provider.shared() {
        let target = base.join(item);
        if !target.exists() {
            continue;
        }
        let path = folder.join(item);
        match std::fs::symlink_metadata(&path) {
            Err(_) => status.missing.push((*item).to_string()),
            Ok(meta) if meta.file_type().is_symlink() && std::fs::read_link(&path).is_ok_and(|to| to == target) => {}
            Ok(_) => status.conflicts.push((*item).to_string()),
        }
    }
    status
}

/// Link every empty slot, unless the folder is a retired one: linking it
/// would make a duplicate look like a working account. Real files stay.
pub fn ensure(provider: Provider, home: &Path, folder: &Path) -> Result<Ensured> {
    let mut status = inspect(provider, home, folder);
    if status.retired.is_some() || status.missing.is_empty() {
        return Ok(Ensured { linked: Vec::new(), status });
    }
    let linked = accounts::link_new_account(provider, home, folder)?;
    status.missing.retain(|item| !linked.contains(item));
    Ok(Ensured { linked, status })
}

/// The destructive repair: `cas accounts link` for the whole provider, with
/// its backup and restore script.
pub fn repair(provider: Provider, home: &Path) -> Result<Option<accounts::Applied>> {
    let plan = accounts::plan(provider, home)?;
    if plan.is_empty() {
        return Ok(None);
    }
    apply(&plan, home).map(Some)
}

/// `accounts::apply` with a backup folder no earlier run used: two repairs in
/// the same second (a link, then doctor or a launch) must not collide.
pub fn apply(plan: &accounts::Plan, home: &Path) -> Result<accounts::Applied> {
    let base = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let backups = home.join(".cas").join("accounts-backup");
    let taken = |stamp: &str| backups.join(format!("{stamp}-{}", plan.provider.name())).exists();
    let mut stamp = base.clone();
    let mut ordinal = 2;
    while taken(&stamp) {
        stamp = format!("{base}-{ordinal}");
        ordinal += 1;
    }
    accounts::apply(plan, home, &stamp)
}

/// Every account folder of every provider that has a base, with its status.
pub fn survey(home: &Path) -> Result<Vec<(Provider, PathBuf, LinkStatus)>> {
    let mut found = Vec::new();
    for provider in Provider::ALL {
        if !provider.base_dir(home).is_dir() {
            continue;
        }
        for account in accounts::discover(provider, home)? {
            let status = inspect(provider, home, &account.path);
            found.push((provider, account.path, status));
        }
    }
    Ok(found)
}

/// `~/x` for a path under `home`.
pub fn short(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// The one-line notices a launch prints (and spawn/Commander receipts carry).
pub fn notices(provider: Provider, home: &Path, folder: &Path, ensured: &Ensured) -> Vec<String> {
    let mut lines = Vec::new();
    let base = short(home, &provider.base_dir(home));
    let here = short(home, folder);
    if !ensured.linked.is_empty() {
        lines.push(format!(
            "Linked {} in {here} from {base}",
            plural(ensured.linked.len(), "shared item")
        ));
    }
    if let Some(retired) = &ensured.status.retired {
        let next = match &retired.canonical {
            Some(canonical) => {
                let name = canonical
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_prefix(provider.prefix()))
                    .unwrap_or_default();
                format!("use {}: cas {} {name}", short(home, canonical), provider.name())
            }
            None => "pick a named account: cas accounts list".to_string(),
        };
        lines.push(format!(
            "Warning: {here} is a retired account folder that came back ({}); {next}",
            retired.reason
        ));
    } else if !ensured.status.conflicts.is_empty() {
        lines.push(format!(
            "Warning: {} in {here} not linked to {base}: {}. Fix: cas doctor --fix --yes",
            plural(ensured.status.conflicts.len(), "shared item"),
            ensured.status.conflicts.join(", ")
        ));
    }
    lines
}

/// Check, auto-link and report before `cas claude|codex` launches `folder`.
/// Never blocks the launch. Interactive launches may repair conflicts now.
pub fn prelaunch(provider: Provider, home: &Path, folder: &Path, interactive: bool) {
    let ensured = match ensure(provider, home, folder) {
        Ok(ensured) => ensured,
        Err(error) => {
            eprintln!(
                "Warning: could not link shared items in {}: {error:#}",
                short(home, folder)
            );
            return;
        }
    };
    for line in notices(provider, home, folder, &ensured) {
        eprintln!("{line}");
    }
    if !interactive || ensured.status.conflicts.is_empty() || ensured.status.retired.is_some() {
        return;
    }
    use std::io::Write as _;
    eprint!("Repair now? Real items are backed up first [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err()
        || !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    {
        return;
    }
    match repair(provider, home) {
        Ok(Some(applied)) => {
            eprintln!(
                "[OK] {}: {} linked; backup {}",
                provider.name(),
                plural(applied.linked, "item"),
                short(home, &applied.backup)
            );
            eprintln!("Undo: bash {}", applied.restore.display());
        }
        Ok(None) => {}
        Err(error) => eprintln!("Warning: repair failed; launching anyway: {error:#}"),
    }
}

fn retired(provider: Provider, home: &Path, folder: &Path) -> Option<Retired> {
    if folder.parent() != Some(home) {
        return None;
    }
    let label = folder.file_name()?.to_str()?;
    let has_login = folder.join(provider.credentials()).exists();
    let duplicate = |email: &str| {
        let canonical = home.join(format!("{}{email}", provider.prefix()));
        (canonical != folder && canonical.is_dir()).then(|| Retired {
            reason: format!("its login {email} duplicates {}", short(home, &canonical)),
            canonical: Some(canonical),
        })
    };
    let no_login = || Retired {
        reason: "it has no login".into(),
        canonical: None,
    };
    // The same rules `accounts::plan` retires folders by.
    if provider.retiring().contains(&label) {
        return match provider.login_email(home, folder) {
            Some(email) => duplicate(&email),
            None if !has_login => Some(no_login()),
            None => None,
        };
    }
    let name = label.strip_prefix(provider.prefix())?;
    if !accounts::email_shaped(name) {
        return None;
    }
    // A readable login email also counts: macOS Claude keeps the credential
    // in the Keychain, not in the folder.
    match provider.login_email(home, folder) {
        None if !has_login => Some(no_login()),
        Some(email) if email != name.to_ascii_lowercase() => duplicate(&email),
        _ => None,
    }
}

#[cfg(test)]
#[path = "account_links_tests.rs"]
mod tests;
