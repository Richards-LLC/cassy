//! One shared base folder per provider for every Claude and Codex account
//! (cas-e1f4).
//!
//! Each named account directory (`~/.claude-<email>`, `~/.codex-<email>`)
//! keeps only its login and runtime state; the shared configuration surface
//! (commands, skills, agents, settings, project memory, ...) is a symlink into
//! one base folder, `~/.cas/claude-base` or `~/.cas/codex-base`. The default
//! directories (`~/.claude`, `~/.claude-alt`, `~/.codex`) stop being accounts:
//! a login that duplicates a named account is retired into the backup, a
//! unique one is renamed to its named directory.
//!
//! The work is a [`Plan`] (read-only, what `cas accounts link --dry-run`
//! prints) and [`apply`]. Apply never discards: every item it replaces is
//! moved into `~/.cas/accounts-backup/<ts>/` first, files that differ from the
//! base are kept side by side as `<name>.from-<account>`, and a `restore.sh`
//! undoes the move. A second plan after an apply is empty.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

/// The providers whose account directories share a base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Claude, Provider::Codex];

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// `~/.cas/claude-base` or `~/.cas/codex-base`.
    pub fn base_dir(self, home: &Path) -> PathBuf {
        home.join(".cas").join(format!("{}-base", self.name()))
    }

    /// Named account directories are `<prefix><name>`.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Claude => ".claude-",
            Self::Codex => ".codex-",
        }
    }

    /// The default directories that stop being accounts.
    pub fn retiring(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[".claude", ".claude-alt"],
            Self::Codex => &[".codex"],
        }
    }

    /// Items every account shares through the base.
    pub fn shared(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &CLAUDE_SHARED,
            Self::Codex => &CODEX_SHARED,
        }
    }

    /// The login item: an account directory without it has no login.
    pub fn credentials(self) -> &'static str {
        match self {
            Self::Claude => ".credentials.json",
            Self::Codex => "auth.json",
        }
    }

    /// The login email of the account at `dir`, if it can be read.
    ///
    /// Claude: `oauthAccount.emailAddress` in `<dir>/.claude.json`; for the
    /// default `~/.claude`, Claude Code keeps that file at `~/.claude.json`,
    /// which wins. Codex does not expose a proven non-secret email source;
    /// its email stays unknown rather than decoding a credential's id token.
    pub fn login_email(self, home: &Path, dir: &Path) -> Option<String> {
        match self {
            Self::Claude => {
                let default = dir == home.join(".claude");
                let candidates = if default {
                    vec![home.join(".claude.json"), dir.join(".claude.json")]
                } else {
                    vec![dir.join(".claude.json")]
                };
                candidates.iter().find_map(|path| claude_email(path))
            }
            Self::Codex => None,
        }
    }
}

/// Claude: the operator's list (cas-e1f4) plus the surfaces earlier
/// profiles already shared (hooks, output-styles).
pub const CLAUDE_SHARED: [&str; 11] = [
    "agents",
    "commands",
    "skills",
    "plugins",
    "hooks",
    "workflows",
    "output-styles",
    "settings.json",
    "settings.local.json",
    "CLAUDE.md",
    "projects",
];

/// Codex: instructions, agents, hooks, config, plugins, skills and memories.
/// `config.toml` is shared: the CAS MCP server is injected per spawn, so it
/// carries no per-account key.
pub const CODEX_SHARED: [&str; 7] = [
    "AGENTS.md",
    "agents",
    "hooks.json",
    "config.toml",
    "plugins",
    "skills",
    "memories",
];

fn claude_email(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value["oauthAccount"]["emailAddress"]
        .as_str()
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(str::to_ascii_lowercase)
}

/// The login email of the Claude account a process runs as (cas-e1f4):
/// `<config_dir>/.claude.json` when `CLAUDE_CONFIG_DIR` is set, else Claude
/// Code's default `~/.claude.json`.
pub fn active_claude_email(home: &Path, config_dir: Option<&Path>) -> Option<String> {
    match config_dir.filter(|dir| !dir.as_os_str().is_empty()) {
        Some(dir) => claude_email(&dir.join(".claude.json")),
        None => claude_email(&home.join(".claude.json")),
    }
}

/// Codex does not publish a proven non-secret login email source. Keep this
/// optional interface stable without opening the active home's auth.json.
pub fn active_codex_email(_home: &Path, _codex_home: Option<&Path>) -> Option<String> {
    None
}

/// One account directory found under the home directory.
#[derive(Debug, Clone, Serialize)]
pub struct AccountDir {
    /// Directory name, e.g. `.claude-pippenz@gmail.com`.
    pub label: String,
    pub path: PathBuf,
    /// A default directory being retired.
    pub retiring: bool,
    pub email: Option<String>,
    pub has_login: bool,
}

/// What happens to a directory as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DirChange {
    /// A default directory whose login duplicates a named account, or a stray
    /// named directory: moved whole into the backup.
    Retire { label: String, reason: String },
    /// A default directory with a unique login, renamed to its named form.
    Rename { label: String, to: String },
}

/// How a file reaches the base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeKind {
    /// The base has no such file yet.
    Copy,
    /// It differs and needs a person: kept beside the base copy as
    /// `<name>.from-<account>`.
    SideBySide,
    /// A regenerable cache (plugin marketplaces, synced skills): the newest
    /// copy wins; every original stays in the backup.
    Newest,
    /// A project memory index (`MEMORY.md`): the union of its lines, so
    /// every account's memories stay indexed.
    UnionLines,
    /// A plugin registry: the union of its JSON objects, base first.
    JsonUnion,
}

/// One file copied into the base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Merge {
    /// Account directory the file comes from.
    pub account: String,
    /// Path relative to that account directory (and to the base).
    pub rel: String,
    /// Where it lands relative to the base: `rel`, or a side-by-side name for
    /// a conflict.
    pub to: String,
    /// Only [`MergeKind::SideBySide`] is a conflict a person reviews.
    pub conflict: bool,
    pub kind: MergeKind,
    /// For a conflict: the account whose copy the base keeps at `rel`, which
    /// every account reads from now on; `None` when the base already had it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_from: Option<String>,
}

/// How a differing copy of `rel` is resolved (cas-e1f4, measured on the
/// operator's accounts: 815 raw conflicts, 731 of them plugin marketplace
/// caches and 31 project memory indexes).
fn resolution(rel: &str) -> MergeKind {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if rel.starts_with("projects/") && rel.ends_with("/memory/MEMORY.md") {
        return MergeKind::UnionLines;
    }
    if matches!(rel, "plugins/installed_plugins.json" | "plugins/known_marketplaces.json") {
        return MergeKind::JsonUnion;
    }
    let regenerable = [
        "plugins/marketplaces/",
        "plugins/cache/",
        "plugins/synced/",
        "plugins/repos/",
        "skills/synced/",
    ]
    .iter()
    .any(|prefix| rel.starts_with(prefix))
        || rel.starts_with("plugins/plugin-directory-cache")
        || (rel.starts_with("plugins/") && name.starts_with('.'));
    if regenerable {
        MergeKind::Newest
    } else {
        MergeKind::SideBySide
    }
}

fn modified(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

/// One shared item replaced by a link to the base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    /// Account directory label after any rename.
    pub account: String,
    pub item: String,
    /// What is there now, for the dry run: `missing`, `file`, `dir` or
    /// `link -> <target>`.
    pub was: String,
}

/// The read-only plan for one provider.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub provider: Provider,
    pub base: PathBuf,
    pub create_base: bool,
    pub accounts: Vec<AccountDir>,
    pub dir_changes: Vec<DirChange>,
    pub merges: Vec<Merge>,
    /// Shared folders that exist only empty in some account: created empty in
    /// the base so they are linked in the first run (cas-e1f4 QA F01).
    pub empty_dirs: Vec<String>,
    pub links: Vec<Link>,
    /// Reported only, never acted on (e.g. a directory named for one email
    /// holding another email's login).
    pub notes: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        !self.create_base
            && self.dir_changes.is_empty()
            && self.merges.is_empty()
            && self.empty_dirs.is_empty()
            && self.links.is_empty()
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &Merge> {
        self.merges.iter().filter(|merge| merge.conflict)
    }
}

/// Whether `name` (after the prefix) looks like a login email.
pub(crate) fn email_shaped(name: &str) -> bool {
    name.split_once('@')
        .is_some_and(|(user, domain)| !user.is_empty() && domain.contains('.'))
}

/// Find the provider's account directories under `home`: the retiring
/// defaults first, then the named ones in name order. Directory symlinks are
/// followed; unreadable entries and non-account siblings (`.lock`, `.bak`, ...)
/// are skipped. Each canonical directory target is returned once, even when
/// several aliases reach it. Write callers treat the target, inside or outside
/// `home`, as the account; the registry retains every alias separately.
pub fn discover(provider: Provider, home: &Path) -> io::Result<Vec<AccountDir>> {
    let mut targets = BTreeSet::new();
    Ok(discover_aliases(provider, home)?
        .into_iter()
        .filter(|account| account.path.canonicalize().is_ok_and(|target| targets.insert(target)))
        .collect())
}

/// Alias-preserving lookup for selection, launch and individual lifecycle commands.
/// Bulk link/sync callers use `discover` to process each physical target once.
pub(crate) fn discover_aliases(provider: Provider, home: &Path) -> io::Result<Vec<AccountDir>> {
    let real_dir = |path: &Path| path.is_dir();
    let account = |label: String, path: PathBuf, retiring: bool| AccountDir {
        email: provider.login_email(home, &path),
        has_login: path.join(provider.credentials()).exists(),
        label,
        path,
        retiring,
    };
    let mut found = Vec::new();
    for label in provider.retiring() {
        let path = home.join(label);
        if real_dir(&path) {
            found.push(account((*label).to_string(), path, true));
        }
    }
    let mut named = Vec::new();
    if let Ok(entries) = std::fs::read_dir(home) {
        for entry in entries.flatten() {
            let Some(label) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Some(name) = label.strip_prefix(provider.prefix()) else {
                continue;
            };
            if provider.retiring().contains(&label.as_str())
                || !crate::cli::account_picker::is_account_profile_name(name)
                || !real_dir(&entry.path())
            {
                continue;
            }
            named.push(label);
        }
    }
    named.sort();
    for label in named {
        let path = home.join(&label);
        found.push(account(label, path, false));
    }
    Ok(found)
}

/// Resolve existing registered Codex homes, or the explicitly selected home.
///
/// An explicit selection is authoritative even if it does not exist. Otherwise
/// use the account registry (legacy default first, then named accounts), without
/// inventing a retired `~/.codex`. This read-only resolver serves capability
/// probes and machine configuration writers alike; it never chooses an account.
pub fn codex_homes(home: &Path, selected: Option<&Path>) -> io::Result<Vec<PathBuf>> {
    if let Some(selected) = selected.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(vec![selected.to_path_buf()]);
    }
    Ok(discover(Provider::Codex, home)?
        .into_iter()
        .map(|account| account.path)
        .collect())
}

/// Build the plan for `provider` under `home`. Reads only.
pub fn plan(provider: Provider, home: &Path) -> Result<Plan> {
    let base = provider.base_dir(home);
    let accounts = discover(provider, home).context("could not list account directories")?;
    let named: BTreeSet<String> = accounts
        .iter()
        .filter(|account| !account.retiring)
        .map(|account| account.label.clone())
        .collect();
    let named_for = |email: &str| format!("{}{email}", provider.prefix());

    let mut dir_changes = Vec::new();
    let mut notes = Vec::new();
    // Final label of every account directory that keeps living.
    let mut keep: Vec<(String, &AccountDir)> = Vec::new();
    for account in &accounts {
        let name = account.label.strip_prefix(provider.prefix()).unwrap_or("");
        if account.retiring {
            match account.email.as_deref() {
                Some(email) if named.contains(&named_for(email)) => {
                    dir_changes.push(DirChange::Retire {
                        label: account.label.clone(),
                        reason: format!("login {email} duplicates {}", named_for(email)),
                    })
                }
                Some(email) => {
                    let to = named_for(email);
                    dir_changes.push(DirChange::Rename {
                        label: account.label.clone(),
                        to: to.clone(),
                    });
                    keep.push((to, account));
                }
                None if !account.has_login => dir_changes.push(DirChange::Retire {
                    label: account.label.clone(),
                    reason: "no login".into(),
                }),
                None => {
                    notes.push(format!(
                        "{} has a login whose email cannot be read; left in place and linked",
                        account.label
                    ));
                    keep.push((account.label.clone(), account));
                }
            }
            continue;
        }
        if email_shaped(name) && !account.has_login {
            dir_changes.push(DirChange::Retire {
                label: account.label.clone(),
                reason: "no login (stray directory)".into(),
            });
            continue;
        }
        match account.email.as_deref() {
            Some(email) if email != name.to_ascii_lowercase() => {
                if email_shaped(name) && named.contains(&named_for(email)) {
                    dir_changes.push(DirChange::Retire {
                        label: account.label.clone(),
                        reason: format!("login {email} duplicates {}", named_for(email)),
                    });
                    continue;
                }
                notes.push(format!(
                    "{} is signed in as {email}; left as is",
                    account.label
                ));
            }
            _ => {}
        }
        keep.push((account.label.clone(), account));
    }

    // Merge every account's shared items into the base, defaults first.
    let mut planned: HashMap<String, [u8; 32]> = HashMap::new();
    let mut planned_time: HashMap<String, std::time::SystemTime> = HashMap::new();
    // The account each planned base file comes from (conflict lines name it).
    let mut planned_from: HashMap<String, String> = HashMap::new();
    // Shared items some account has as a real directory, even an empty one.
    let mut dir_items: BTreeSet<String> = BTreeSet::new();
    let mut seen_sources: BTreeSet<PathBuf> = BTreeSet::new();
    let base_canonical = base.canonicalize().ok();
    let mut merges = Vec::new();
    for account in &accounts {
        for item in provider.shared() {
            let source = account.path.join(item);
            let Ok(canonical) = source.canonicalize() else {
                continue;
            };
            if base_canonical
                .as_ref()
                .is_some_and(|base| canonical.starts_with(base))
                || !seen_sources.insert(canonical.clone())
            {
                continue;
            }
            if canonical.is_dir() {
                dir_items.insert((*item).to_string());
            }
            for (rel, file) in files_under(&canonical, item)? {
                let digest = digest(&file)?;
                let existing = match planned.get(&rel) {
                    Some(planned) => Some(*planned),
                    None => {
                        let on_disk = base.join(&rel);
                        if on_disk.is_file() {
                            Some(digest_file(&on_disk)?)
                        } else {
                            None
                        }
                    }
                };
                let merge = |rel: String, to: String, kind: MergeKind, base_from: Option<String>| Merge {
                    account: account.label.clone(),
                    rel,
                    to,
                    conflict: kind == MergeKind::SideBySide,
                    kind,
                    base_from,
                };
                match existing {
                    None => {
                        planned.insert(rel.clone(), digest);
                        planned_time.insert(rel.clone(), modified(&file));
                        planned_from.insert(rel.clone(), account.label.clone());
                        merges.push(merge(rel.clone(), rel, MergeKind::Copy, None));
                    }
                    Some(existing) if existing == digest => {}
                    Some(_) => match resolution(&rel) {
                        MergeKind::Newest => {
                            let current = planned_time
                                .get(&rel)
                                .copied()
                                .unwrap_or_else(|| modified(&base.join(&rel)));
                            if modified(&file) > current {
                                planned.insert(rel.clone(), digest);
                                planned_time.insert(rel.clone(), modified(&file));
                                merges.push(merge(rel.clone(), rel, MergeKind::Newest, None));
                            }
                        }
                        kind @ (MergeKind::UnionLines | MergeKind::JsonUnion) => {
                            planned.insert(rel.clone(), digest);
                            merges.push(merge(rel.clone(), rel, kind, None));
                        }
                        _ => {
                            let to = side_by_side(&rel, &account.label);
                            if planned.contains_key(&to) || base.join(&to).exists() {
                                continue;
                            }
                            planned.insert(to.clone(), digest);
                            let base_from = planned_from.get(&rel).cloned();
                            merges.push(merge(rel, to, MergeKind::SideBySide, base_from));
                        }
                    },
                }
            }
        }
    }

    // Link every shared item the base has (or will have) in every kept
    // account directory.
    let base_has = |item: &str| {
        base.join(item).exists()
            || dir_items.contains(item)
            || planned
                .keys()
                .any(|rel| rel == item || rel.starts_with(&format!("{item}/")))
    };
    let empty_dirs: Vec<String> = dir_items
        .iter()
        .filter(|item| {
            !base.join(item.as_str()).exists()
                && !planned.keys().any(|rel| rel.starts_with(&format!("{item}/")))
        })
        .cloned()
        .collect();
    let mut links = Vec::new();
    for (label, account) in &keep {
        for item in provider.shared() {
            if !base_has(item) {
                continue;
            }
            let path = account.path.join(item);
            let target = base.join(item);
            let was = match std::fs::symlink_metadata(&path) {
                Err(_) => "missing".to_string(),
                Ok(meta) if meta.file_type().is_symlink() => {
                    let pointed = std::fs::read_link(&path).unwrap_or_default();
                    if pointed == target {
                        continue;
                    }
                    format!("link -> {}", pointed.display())
                }
                Ok(meta) if meta.is_dir() => "dir".to_string(),
                Ok(_) => "file".to_string(),
            };
            links.push(Link {
                account: label.clone(),
                item: (*item).to_string(),
                was,
            });
        }
    }

    Ok(Plan {
        provider,
        create_base: !base.is_dir(),
        base,
        accounts,
        dir_changes,
        merges,
        empty_dirs,
        links,
        notes,
    })
}

/// `commands/finish.md` from `.claude-daniel@x` → `commands/finish.md.from-claude-daniel@x`.
fn side_by_side(rel: &str, account: &str) -> String {
    format!("{rel}.from-{}", account.trim_start_matches('.'))
}

/// Every regular file under `root` (or `root` itself), as paths relative to
/// the account directory: `<item>/<...>`. Symlinks inside an item are
/// followed only when they point at a regular file.
pub(crate) fn files_under(root: &Path, item: &str) -> Result<Vec<(String, PathBuf)>> {
    if root.is_file() {
        return Ok(vec![(item.to_string(), root.to_path_buf())]);
    }
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let entry = entry.with_context(|| format!("could not read {}", root.display()))?;
        let path = entry.path();
        let is_file = entry.file_type().is_file()
            || (entry.file_type().is_symlink() && path.is_file());
        if !is_file {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(path);
        files.push((
            format!("{item}/{}", relative.to_string_lossy()),
            path.to_path_buf(),
        ));
    }
    Ok(files)
}

fn digest(path: &Path) -> Result<[u8; 32]> {
    digest_file(path)
}

pub(crate) fn digest_file(path: &Path) -> Result<[u8; 32]> {
    use sha2::{Digest as _, Sha256};
    use std::io::Read as _;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("could not read {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

/// What an apply did, and how to undo it.
#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    pub backup: PathBuf,
    pub restore: PathBuf,
    pub merged: usize,
    pub conflicts: usize,
    pub linked: usize,
    pub dir_changes: usize,
}

/// Apply `plan` under `home`, backing everything up under
/// `~/.cas/accounts-backup/<stamp>/` first.
///
/// Order: (1) every shared item about to be replaced, and every directory
/// being retired, is moved into the backup; (2) the files of the plan are
/// copied into the base from their backed-up location; (3) renames run;
/// (4) links are created. `restore.sh` reverses (1), (3) and (4); the base is
/// left in place.
pub fn apply(plan: &Plan, home: &Path, stamp: &str) -> Result<Applied> {
    let backup = home
        .join(".cas")
        .join("accounts-backup")
        .join(format!("{stamp}-{}", plan.provider.name()));
    std::fs::create_dir_all(&backup)
        .with_context(|| format!("could not create {}", backup.display()))?;
    restrict(&backup)?;
    let mut restore = vec![
        "#!/usr/bin/env bash".to_string(),
        format!(
            "# Undo `cas accounts link` for {} (backup {}).",
            plan.provider.name(),
            backup.display()
        ),
        "# Run with no Claude or Codex session open on these accounts.".to_string(),
        "set -euo pipefail".to_string(),
    ];
    let mut undo: Vec<String> = Vec::new();
    // Original path of a moved file or directory → its backup location.
    let mut moved: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();

    // (1) Move directories being retired, and items being replaced.
    let account_path = |label: &str| home.join(label);
    for change in &plan.dir_changes {
        if let DirChange::Retire { label, .. } = change {
            let from = account_path(label);
            let to = backup.join("dirs").join(label);
            move_path(&from, &to)?;
            undo.push(format!("mv {} {}", quote(&to), quote(&from)));
            moved.insert(from, to);
        }
    }
    let renamed: HashMap<&str, &str> = plan
        .dir_changes
        .iter()
        .filter_map(|change| match change {
            DirChange::Rename { label, to } => Some((to.as_str(), label.as_str())),
            DirChange::Retire { .. } => None,
        })
        .collect();
    for link in &plan.links {
        let original_label = renamed.get(link.account.as_str()).copied().unwrap_or(&link.account);
        let from = account_path(original_label).join(&link.item);
        if std::fs::symlink_metadata(&from).is_err() {
            continue;
        }
        let to = backup.join("items").join(original_label).join(&link.item);
        move_path(&from, &to)?;
        // Undo runs after any rename is reversed, so it names the original
        // directory.
        undo.push(format!(
            "rm -f {link_path} && mv {} {link_path}",
            quote(&to),
            link_path = quote(&from)
        ));
        moved.insert(from, to);
    }

    // (2) Fill the base.
    std::fs::create_dir_all(&plan.base)
        .with_context(|| format!("could not create {}", plan.base.display()))?;
    let mut merged = 0;
    let mut conflicts = 0;
    for merge in &plan.merges {
        let original = account_path(&merge.account).join(&merge.rel);
        let source = locate(&original, &moved, home)
            .with_context(|| format!("could not find {} to merge", original.display()))?;
        let target = plan.base.join(&merge.to);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match merge.kind {
            MergeKind::Copy | MergeKind::SideBySide if target.exists() => continue,
            MergeKind::Copy | MergeKind::SideBySide | MergeKind::Newest => {
                std::fs::copy(&source, &target).with_context(|| {
                    format!("could not copy {} into the base", source.display())
                })?;
            }
            MergeKind::UnionLines => union_lines(&source, &target)?,
            MergeKind::JsonUnion => {
                if json_union(&source, &target).is_err() {
                    // Unreadable JSON is a person's call, side by side.
                    let aside = plan.base.join(side_by_side(&merge.rel, &merge.account));
                    std::fs::copy(&source, &aside)?;
                    conflicts += 1;
                }
            }
        }
        merged += 1;
        conflicts += usize::from(merge.conflict);
    }
    for item in &plan.empty_dirs {
        std::fs::create_dir_all(plan.base.join(item))?;
    }
    for item in plan.provider.shared() {
        // An empty shared directory still becomes a linkable base entry.
        let original = plan
            .accounts
            .iter()
            .map(|account| account.path.join(item))
            .find_map(|path| locate(&path, &moved, home));
        if let Some(source) = original
            && source.is_dir()
            && !plan.base.join(item).exists()
        {
            std::fs::create_dir_all(plan.base.join(item))?;
        }
    }

    // (3) Renames.
    for change in &plan.dir_changes {
        if let DirChange::Rename { label, to } = change {
            let from = account_path(label);
            let target = account_path(to);
            anyhow::ensure!(
                !target.exists(),
                "{} already exists; not renaming {}",
                target.display(),
                from.display()
            );
            std::fs::rename(&from, &target).with_context(|| {
                format!("could not rename {} to {}", from.display(), target.display())
            })?;
            undo.push(format!("mv {} {}", quote(&target), quote(&from)));
            // The default Claude login lives beside the directory. Undo (it
            // runs before the rename is reversed) puts the directory's own
            // login file back byte for byte (cas-e1f4 QA F02).
            if plan.provider == Provider::Claude && label == ".claude" {
                let login = home.join(".claude.json");
                if login.is_file() {
                    let inside = target.join(".claude.json");
                    let kept = target.join(".claude.json.pre-link");
                    if inside.exists() {
                        std::fs::rename(&inside, &kept)?;
                        undo.push(format!("mv -f {} {}", quote(&kept), quote(&inside)));
                    } else {
                        undo.push(format!("rm -f {}", quote(&inside)));
                    }
                    std::fs::copy(&login, &inside)?;
                }
            }
        }
    }

    // (4) Links.
    let mut linked = 0;
    for link in &plan.links {
        let path = account_path(&link.account).join(&link.item);
        let target = plan.base.join(&link.item);
        if !target.exists() {
            continue;
        }
        if std::fs::symlink_metadata(&path).is_ok() {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        symlink(&target, &path)
            .with_context(|| format!("could not link {}", path.display()))?;
        let original_label = renamed.get(link.account.as_str()).copied().unwrap_or(&link.account);
        if !moved.contains_key(&account_path(original_label).join(&link.item)) {
            // Nothing was there before: undo removes the link.
            undo.push(format!("rm -f {}", quote(&path)));
        }
        linked += 1;
    }

    // Undo runs last-first: links and moved items, then renames, then dirs.
    undo.reverse();
    restore.extend(undo);
    restore.push(format!(
        "echo 'Restored. The base {} was left in place.'",
        plan.base.display()
    ));
    let restore_path = backup.join("restore.sh");
    std::fs::write(&restore_path, restore.join("\n") + "\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&restore_path, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::write(
        backup.join("plan.json"),
        serde_json::to_string_pretty(plan)?,
    )?;
    Ok(Applied {
        backup,
        restore: restore_path,
        merged,
        conflicts,
        linked,
        dir_changes: plan.dir_changes.len(),
    })
}

/// Append the lines of `source` that `target` lacks, keeping `target`'s
/// order first (a `MEMORY.md` index from each account).
fn union_lines(source: &Path, target: &Path) -> Result<()> {
    let base = std::fs::read_to_string(target).unwrap_or_default();
    let incoming = std::fs::read_to_string(source)
        .with_context(|| format!("could not read {}", source.display()))?;
    let known: BTreeSet<&str> = base.lines().collect();
    let mut merged = base.clone();
    let mut added = false;
    for line in incoming.lines() {
        if line.trim().is_empty() || known.contains(line) {
            continue;
        }
        if !added && !merged.is_empty() && !merged.ends_with('\n') {
            merged.push('\n');
        }
        merged.push_str(line);
        merged.push('\n');
        added = true;
    }
    if added {
        std::fs::write(target, merged)?;
    }
    Ok(())
}

/// Merge the JSON object at `source` into `target`: objects merge
/// recursively, `target` wins on a scalar, arrays gain missing items.
fn json_union(source: &Path, target: &Path) -> Result<()> {
    fn merge(into: &mut serde_json::Value, from: serde_json::Value) {
        match (into, from) {
            (serde_json::Value::Object(into), serde_json::Value::Object(from)) => {
                for (key, value) in from {
                    match into.get_mut(&key) {
                        Some(existing) => merge(existing, value),
                        None => {
                            into.insert(key, value);
                        }
                    }
                }
            }
            (serde_json::Value::Array(into), serde_json::Value::Array(from)) => {
                for value in from {
                    if !into.contains(&value) {
                        into.push(value);
                    }
                }
            }
            _ => {}
        }
    }
    let mut base: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(target)?)?;
    let incoming: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(source)?)?;
    anyhow::ensure!(base.is_object() && incoming.is_object(), "not a JSON object");
    merge(&mut base, incoming);
    std::fs::write(target, serde_json::to_string_pretty(&base)? + "\n")?;
    Ok(())
}

/// Where `original` lives now: in place, or under the backup a parent of it
/// (or its symlink target) was moved to.
fn locate(original: &Path, moved: &BTreeMap<PathBuf, PathBuf>, home: &Path) -> Option<PathBuf> {
    for (from, to) in moved.iter().rev() {
        if let Ok(rest) = original.strip_prefix(from) {
            let candidate = if rest.as_os_str().is_empty() {
                to.clone()
            } else {
                to.join(rest)
            };
            return resolve_moved(&candidate, moved, home);
        }
    }
    resolve_moved(original, moved, home)
}

/// Follow a symlink in a moved tree to its own (possibly moved) target.
fn resolve_moved(
    path: &Path,
    moved: &BTreeMap<PathBuf, PathBuf>,
    home: &Path,
) -> Option<PathBuf> {
    if path.exists() {
        return Some(path.to_path_buf());
    }
    // A dangling path: walk up to the first symlink ancestor and re-target.
    let mut ancestor = path.to_path_buf();
    let mut rest = PathBuf::new();
    loop {
        if std::fs::symlink_metadata(&ancestor).is_ok_and(|meta| meta.file_type().is_symlink()) {
            let target = std::fs::read_link(&ancestor).ok()?;
            let target = if target.is_absolute() {
                target
            } else {
                ancestor.parent()?.join(target)
            };
            let joined = if rest.as_os_str().is_empty() {
                target
            } else {
                target.join(&rest)
            };
            if joined.starts_with(home) && joined != path {
                return locate(&joined, moved, home);
            }
            return None;
        }
        let name = ancestor.file_name()?.to_os_string();
        rest = if rest.as_os_str().is_empty() {
            PathBuf::from(name)
        } else {
            PathBuf::from(name).join(&rest)
        };
        if !ancestor.pop() {
            return None;
        }
    }
}

fn move_path(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)
        .with_context(|| format!("could not move {} to {}", from.display(), to.display()))
}

fn restrict(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = path;
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, path: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(not(unix))]
fn symlink(target: &Path, path: &Path) -> io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, path)
    } else {
        std::os::windows::fs::symlink_file(target, path)
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// A shared item in an account directory that is not linked to the base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drift {
    pub provider: Provider,
    pub account: String,
    pub item: String,
}

/// Drift for `provider`: only once its base exists, so an operator who never
/// linked sees nothing.
pub fn drift(provider: Provider, home: &Path) -> Result<Vec<Drift>> {
    let base = provider.base_dir(home);
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for account in discover(provider, home)? {
        for item in provider.shared() {
            if !base.join(item).exists() {
                continue;
            }
            let path = account.path.join(item);
            let linked = std::fs::read_link(&path).is_ok_and(|target| target == base.join(item));
            if !linked || account.retiring {
                found.push(Drift {
                    provider,
                    account: account.label.clone(),
                    item: (*item).to_string(),
                });
            }
        }
    }
    Ok(found)
}

/// Link a new account directory to the base (account creation). Returns the
/// items linked; existing entries are left alone.
pub fn link_new_account(provider: Provider, home: &Path, account: &Path) -> Result<Vec<String>> {
    let base = provider.base_dir(home);
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    let mut linked = Vec::new();
    for item in provider.shared() {
        let target = base.join(item);
        let path = account.join(item);
        if target.exists() && std::fs::symlink_metadata(&path).is_err() {
            symlink(&target, &path)
                .with_context(|| format!("could not link {}", path.display()))?;
            linked.push((*item).to_string());
        }
    }
    Ok(linked)
}

/// The dry run, for people: what moves, merges, conflicts and links, each
/// line wrapped to 80 columns. `--json` carries every file.
pub fn render_plan(plan: &Plan, home: &Path) -> String {
    use crate::cloud::presence::wrap_line;
    let short = |path: &Path| match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    let plural = |count: usize, word: &str| {
        format!("{count} {word}{}", if count == 1 { "" } else { "s" })
    };
    let conflicts = plan.conflicts().count();
    let mut out = wrap_line(&format!(
        "{} {}: {}, {}, {}, {}",
        if plan.is_empty() { "[OK]" } else { "[PLAN]" },
        plan.provider.name(),
        plural(plan.merges.len() - conflicts, "file"),
        plural(conflicts, "conflict"),
        plural(plan.links.len(), "link"),
        plural(plan.dir_changes.len(), "dir change"),
    ));
    out.push_str("  ");
    out.push_str(&wrap_line(&format!(
        "base {} from {}",
        short(&plan.base),
        plural(plan.accounts.len(), "account dir")
    )));
    if plan.is_empty() {
        out.push_str("  Every account already links its shared items to the base.\n");
    }
    if plan.create_base {
        out.push_str(&wrap_line(&format!("Create {}", short(&plan.base))));
    }
    if !plan.empty_dirs.is_empty() {
        out.push_str(&wrap_line(&format!(
            "Create empty shared folders in the base: {}",
            plan.empty_dirs.join(", ")
        )));
    }
    for change in &plan.dir_changes {
        out.push_str(&wrap_line(&match change {
            DirChange::Retire { label, reason } => format!("Retire ~/{label}: {reason}"),
            DirChange::Rename { label, to } if plan.provider == Provider::Claude && label == ".claude" => format!(
                "Rename ~/{label} to ~/{to}: unique login; ~/.claude.json is copied in as its login file (its own .claude.json kept as .claude.json.pre-link; undo restores it)"
            ),
            DirChange::Rename { label, to } => format!("Rename ~/{label} to ~/{to}: unique login"),
        }));
    }
    // Copies grouped per account and top-level item, so a projects/ tree
    // reads as one line.
    let mut groups: BTreeMap<(String, String), usize> = BTreeMap::new();
    for merge in plan.merges.iter().filter(|merge| merge.kind == MergeKind::Copy) {
        let item = merge.rel.split('/').next().unwrap_or(&merge.rel).to_string();
        *groups.entry((merge.account.clone(), item)).or_default() += 1;
    }
    for ((account, item), count) in groups {
        out.push_str(&wrap_line(&format!(
            "Merge {item} from ~/{account}: {}",
            plural(count, "file")
        )));
    }
    let count = |kind: MergeKind| plan.merges.iter().filter(|m| m.kind == kind).count();
    let (memory, registries, caches) = (
        count(MergeKind::UnionLines),
        count(MergeKind::JsonUnion),
        count(MergeKind::Newest),
    );
    if memory > 0 {
        out.push_str(&wrap_line(&format!(
            "Union {memory} project memory index{} (MEMORY.md): every account's memories stay listed",
            if memory == 1 { "" } else { "es" }
        )));
    }
    if registries > 0 {
        out.push_str(&wrap_line(&format!(
            "Union {registries} plugin registr{} (installed and known marketplaces)",
            if registries == 1 { "y" } else { "ies" }
        )));
    }
    if caches > 0 {
        out.push_str(&wrap_line(&format!(
            "Keep the newest of {} (plugin marketplaces, synced skills)",
            plural(caches, "regenerable cache file")
        )));
    }
    // Every conflict a person reviews, listed in full: they are few once
    // caches, memory indexes and registries merge themselves.
    for merge in plan.conflicts() {
        let kept = match &merge.base_from {
            Some(from) => format!("every account now reads ~/{from}'s copy"),
            None => "every account now reads the base's existing copy".to_string(),
        };
        out.push_str(&wrap_line(&format!(
            "Conflict {}: {kept}; ~/{}'s is kept beside it as {}",
            merge.rel, merge.account, merge.to
        )));
    }
    let mut per_account: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for link in &plan.links {
        per_account.entry(&link.account).or_default().push(&link.item);
    }
    for (account, items) in per_account {
        out.push_str(&wrap_line(&format!("Link ~/{account}: {}", items.join(", "))));
    }
    for note in &plan.notes {
        out.push_str(&wrap_line(&format!("Note: {note}")));
    }
    out
}

#[cfg(test)]
#[path = "accounts_tests.rs"]
mod tests;
