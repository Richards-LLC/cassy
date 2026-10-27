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

/// The launch-time repair of one folder: back up its own real shared items,
/// copy files the base lacks into the base, and link those slots. It never
/// renames, retires or moves a folder and never touches another account, so
/// it is offered only when no folder change or cross-account merge is needed.
pub enum FolderRepair {
    /// Nothing to replace.
    None,
    /// Needs `cas doctor --fix`: a retired or default folder, or an item
    /// whose files differ from the base's (a merge across accounts).
    NeedsDoctor,
    /// This folder's own slots only.
    Offer(accounts::Plan),
}

pub fn plan_folder_repair(provider: Provider, home: &Path, folder: &Path) -> Result<FolderRepair> {
    let status = inspect(provider, home, folder);
    if status.conflicts.is_empty() {
        return Ok(FolderRepair::None);
    }
    let label = match folder.file_name().and_then(|name| name.to_str()) {
        Some(label) if folder.parent() == Some(home) => label.to_string(),
        _ => return Ok(FolderRepair::NeedsDoctor),
    };
    // A default folder is renamed or retired by `cas accounts link`.
    if status.retired.is_some() || provider.retiring().contains(&label.as_str()) {
        return Ok(FolderRepair::NeedsDoctor);
    }
    let base = provider.base_dir(home);
    let mut merges = Vec::new();
    let mut links = Vec::new();
    for item in &status.conflicts {
        let path = folder.join(item);
        let meta = std::fs::symlink_metadata(&path)?;
        let was = if meta.file_type().is_symlink() {
            format!("link -> {}", std::fs::read_link(&path)?.display())
        } else if meta.is_dir() {
            "dir".into()
        } else {
            "file".into()
        };
        // A link elsewhere keeps its target's files in place; only a real item
        // carries files into the base.
        if !meta.file_type().is_symlink() {
            for (rel, file) in accounts::files_under(&path, item)? {
                let shared = base.join(&rel);
                if std::fs::symlink_metadata(&shared).is_err() {
                    merges.push(accounts::Merge {
                        account: label.clone(),
                        to: rel.clone(),
                        rel,
                        conflict: false,
                        kind: accounts::MergeKind::Copy,
                        base_from: None,
                    });
                } else if !shared.is_file() || accounts::digest_file(&shared)? != accounts::digest_file(&file)? {
                    return Ok(FolderRepair::NeedsDoctor);
                }
            }
        }
        links.push(accounts::Link {
            account: label.clone(),
            item: item.clone(),
            was,
        });
    }
    let account = accounts::AccountDir {
        label: label.clone(),
        path: folder.to_path_buf(),
        retiring: false,
        email: provider.login_email(home, folder),
        has_login: folder.join(provider.credentials()).exists(),
    };
    Ok(FolderRepair::Offer(accounts::Plan {
        provider,
        base,
        create_base: false,
        accounts: vec![account],
        dir_changes: Vec::new(),
        merges,
        empty_dirs: Vec::new(),
        links,
        notes: Vec::new(),
    }))
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
/// Never blocks the launch. An interactive launch may repair the folder's
/// own real items now. Returns the folder to launch, re-read after a repair.
pub fn prelaunch(provider: Provider, home: &Path, folder: &Path, interactive: bool) -> PathBuf {
    let mut ask = |question: &str| {
        use std::io::Write as _;
        eprint!("{question} [y/N] ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        let read = std::io::stdin().read_line(&mut answer);
        // An empty read (EOF) echoes no newline; keep later output off the prompt line.
        if !answer.ends_with('\n') {
            eprintln!();
        }
        read.is_ok() && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    };
    prelaunch_with(provider, home, folder, interactive.then_some(&mut ask as &mut dyn FnMut(&str) -> bool))
}

fn prelaunch_with(
    provider: Provider,
    home: &Path,
    folder: &Path,
    confirm: Option<&mut dyn FnMut(&str) -> bool>,
) -> PathBuf {
    let here = short(home, folder);
    let ensured = match ensure(provider, home, folder) {
        Ok(ensured) => ensured,
        Err(error) => {
            eprintln!("Warning: could not link shared items in {here}: {error:#}");
            return folder.to_path_buf();
        }
    };
    for line in notices(provider, home, folder, &ensured) {
        eprintln!("{line}");
    }
    let Some(confirm) = confirm else {
        return folder.to_path_buf();
    };
    let plan = match plan_folder_repair(provider, home, folder) {
        Ok(FolderRepair::Offer(plan)) => plan,
        Ok(FolderRepair::None) => return folder.to_path_buf(),
        Ok(FolderRepair::NeedsDoctor) => {
            eprintln!("This needs folder or cross-account changes; run `cas doctor --fix` to preview them.");
            return folder.to_path_buf();
        }
        Err(error) => {
            eprintln!("Warning: could not plan a repair for {here}: {error:#}");
            return folder.to_path_buf();
        }
    };
    let items: Vec<&str> = plan.links.iter().map(|link| link.item.as_str()).collect();
    let question = format!(
        "Back up and link {} in {here} only (no other folder changes)?",
        items.join(", ")
    );
    if !confirm(&question) {
        return folder.to_path_buf();
    }
    match apply(&plan, home) {
        Ok(applied) => {
            let base = short(home, &plan.base);
            let mut line = format!("[OK] Linked {} in {here} to {base}", items.join(", "));
            if applied.merged > 0 {
                line.push_str(&format!("; copied {} the base lacked into it", plural(applied.merged, "file")));
            }
            line.push_str(&format!("; replaced items backed up in {}", short(home, &applied.backup)));
            eprintln!("{line}");
            eprintln!("Undo: bash {}", applied.restore.display());
        }
        Err(error) => eprintln!("Warning: repair failed; launching anyway: {error:#}"),
    }
    // Re-read the launch folder: the repair must leave it in place.
    let status = inspect(provider, home, folder);
    if !folder.is_dir() || !status.conflicts.is_empty() {
        eprintln!("Warning: {here} still has unlinked shared items; run `cas doctor --fix`.");
    }
    folder.to_path_buf()
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
