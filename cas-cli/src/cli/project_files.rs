//! Ownership and Git reconciliation for files installed by `cas update`.
//! See docs/ARCHITECTURE.md, “Managed project files”. Never pushes.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use cas_mux::SupervisorCli;
use serde::Serialize;
use sha2::Digest;

use crate::builtins::{
    self, BUILTIN_GITIGNORE_BEGIN as IGNORE_BEGIN, BUILTIN_GITIGNORE_END as IGNORE_END,
};

mod git_lock;

const BEGIN: &str = "<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->";
const END: &str = "<!-- CAS:END -->";
const TRACKED: &[&str] = &[
    ".gitignore",
    "CLAUDE.md",
    "AGENTS.md",
    ".claude/settings.json",
    ".mcp.json",
    ".codex/config.toml",
    ".codex/hooks.json",
];

pub(crate) fn authoring(root: &Path) -> bool {
    root.join("Cargo.toml").is_file() && root.join("cas-cli/src/builtins.rs").is_file()
}

pub(crate) fn git_root(root: &Path) -> bool {
    git(root, &["rev-parse", "--show-toplevel"])
        .ok()
        .is_some_and(|out| {
            Path::new(String::from_utf8_lossy(&out).trim())
                .canonicalize()
                .ok()
                == root.canonicalize().ok()
        })
}

pub(crate) fn harnesses(root: &Path) -> Vec<SupervisorCli> {
    let mut out = vec![SupervisorCli::Claude];
    for (h, dir) in [
        (SupervisorCli::Codex, ".codex"),
        (SupervisorCli::Grok, ".grok"),
        (SupervisorCli::OpenCode, ".opencode"),
    ] {
        if root.join(dir).is_dir()
            || h != SupervisorCli::Codex && builtins::harness_installed_for_user(h)
        {
            out.push(h);
        }
    }
    out
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !out.status.success() {
        if let Some(finding) = git_lock::diagnose(root, &String::from_utf8_lossy(&out.stderr)) {
            bail!("{finding}");
        }
        bail!(
            "git {}: {}",
            args[0],
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn may_commit_managed_files(root: &Path, cas_root: &Path) -> Result<bool> {
    let Some(_) = git(root, &["symbolic-ref", "--quiet", "HEAD"])
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|reference| reference.trim().strip_prefix("refs/heads/").map(str::to_string))
        .filter(|branch| !branch.is_empty())
    else {
        // Detached or unreadable HEAD is not permission to create history.
        return Ok(false);
    };
    // Sync owns managed commits on named branches, including main and train
    // branches. Never complete someone else's merge/rebase/sequencer operation.
    crate::config::Config::load(cas_root)?;
    for state in ["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "rebase-merge", "rebase-apply", "sequencer"] {
        let path = git(root, &["rev-parse", "--path-format=absolute", "--git-path", state])?;
        if Path::new(text(&path)?.trim()).exists() {
            return Ok(false);
        }
    }
    Ok(true)
}

// Read the effective rules (including nested/global ignores) without touching
// the index. A project exception is a conflict, not permission to override it.
fn ignore_conflicts(root: &Path, paths: &[String]) -> Result<Vec<String>> {
    // File-backed stdin avoids a bidirectional pipe deadlock on large catalogs.
    let mut input = tempfile::tempfile()?;
    for path in paths {
        input.write_all(path.trim_start_matches('/').as_bytes())?;
        input.write_all(&[0])?;
    }
    input.rewind()?;
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "check-ignore",
            "--no-index",
            "--verbose",
            "--non-matching",
            "-z",
            "--stdin",
        ])
        .stdin(Stdio::from(input))
        .output()?;
    if !matches!(output.status.code(), Some(0 | 1)) {
        bail!(
            "cannot inspect managed ignore rules: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .collect::<Vec<_>>()
        .chunks_exact(4)
        .filter(|row| row[2].starts_with(b"!"))
        .map(|row| String::from_utf8_lossy(row[0]).to_string())
        .collect())
}

fn cassy_command(command: &str) -> bool {
    command.starts_with("cas ") || command.starts_with("CAS_HOOK_HARNESS=codex cas ")
}

fn text(bytes: &[u8]) -> Result<&str> {
    Ok(std::str::from_utf8(bytes)?)
}

fn block<'a>(s: &'a str, begin: &str, end: &str) -> Result<Option<&'a str>> {
    match (s.find(begin), s.find(end)) {
        (None, None) => Ok(None),
        (Some(a), Some(b))
            if b >= a && s.matches(begin).count() == 1 && s.matches(end).count() == 1 =>
        {
            Ok(Some(&s[a..b + end.len()]))
        }
        _ => bail!("malformed or duplicate managed block"),
    }
}

fn replace_block(base: &str, replacement: &str, begin: &str, end: &str) -> Result<String> {
    if let Some(old) = block(base, begin, end)? {
        Ok(base.replacen(old, replacement, 1))
    } else if base.is_empty() {
        Ok(format!("{replacement}\n"))
    } else {
        Ok(format!(
            "{base}{}{replacement}\n",
            if base.ends_with('\n') { "\n" } else { "\n\n" }
        ))
    }
}

fn generated_agents(s: &str) -> bool {
    s.contains("<!-- Auto-generated from CLAUDE.md by `cas sync agents-md`.")
        || block(s, BEGIN, END)
            .ok()
            .flatten()
            .is_some_and(|owned| s.replace(owned, "").trim().is_empty())
}

/// The one ignored-file classification, shared by update, init and doctor.
/// Exact catalog paths protect project-authored siblings. Store projections
/// have their reserved namespace; mixed AGENTS.md remains project-tracked.
pub(crate) fn ignored_paths(root: &Path, harnesses: &[SupervisorCli]) -> Vec<String> {
    let mut paths: BTreeSet<_> = builtins::builtin_gitignore_entries(harnesses)
        .into_iter()
        .collect();
    paths.extend(
        [
            "/.cas/",
            "/.env.worktree",
            "/.fallow/",
            "/.claude/rules/cas/",
        ]
        .map(str::to_string),
    );
    if harnesses.contains(&SupervisorCli::Claude) {
        paths.insert("/.claude/skills/cas/SKILL.md".into());
    }
    let agents = std::fs::read_to_string(root.join("AGENTS.md")).ok();
    let committed = git(root, &["show", "HEAD:AGENTS.md"])
        .ok()
        .and_then(|b| String::from_utf8(b).ok());
    if agents.as_deref().is_none_or(generated_agents)
        && committed.as_deref().is_none_or(generated_agents)
    {
        paths.insert("/AGENTS.md".into());
    }
    if root.join(".cas/cas.db").is_file() {
        if let Ok(store) = crate::store::open_skill_store(&root.join(".cas")) {
            if let Ok(skills) = store.list(None) {
                for skill in skills {
                    paths.insert(format!(
                        "/.claude/skills/{}/SKILL.md",
                        crate::sync::skills::skill_dir_name(&skill.name)
                    ));
                }
            }
        }
    }
    paths.into_iter().collect()
}

// Exact source bytes distinguish a prior legitimate sync from a manual edit.
// Reuse the writers' renderers; no second projection format lives here.
fn source_projections(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut projections = BTreeMap::new();
    let cas_dir = root.join(".cas");
    if !cas_dir.join("cas.db").is_file() {
        return Ok(projections);
    }
    let rules = crate::store::open_rule_store_local(&cas_dir)?.list()?;
    let syncer = crate::sync::Syncer::with_defaults(root);
    for rule in rules.iter().filter(|rule| syncer.should_sync(rule)) {
        projections.insert(
            format!(".claude/rules/cas/{}.md", rule.id),
            crate::sync::Syncer::render_rule(rule).into_bytes(),
        );
    }
    let skills = crate::store::open_skill_store_local(&cas_dir)?.list(None)?;
    let syncer = crate::sync::SkillSyncer::with_defaults(root);
    for skill in skills.iter().filter(|skill| syncer.is_enabled(skill)) {
        projections.insert(
            format!(
                ".claude/skills/{}/SKILL.md",
                crate::sync::skills::skill_dir_name(&skill.name)
            ),
            syncer.generate_skill_md(skill).into_bytes(),
        );
    }
    Ok(projections)
}

fn ignored(path: &str, paths: &[String]) -> bool {
    paths.iter().any(|p| {
        let p = p.trim_start_matches('/');
        if p.ends_with('/') {
            path.starts_with(p)
        } else {
            path == p
        }
    })
}

fn harness_path(path: &str) -> bool {
    [".claude/", ".codex/", ".grok/", ".opencode/"]
        .iter()
        .any(|p| path.starts_with(p))
}

fn cassy_body(path: &str, bytes: &[u8]) -> bool {
    harness_path(path)
        && text(bytes).is_ok_and(|s| {
            builtins::is_managed_by_cas(s)
                || (path.contains("/workflows/")
                    && (s.contains("managed_by: cas") || path.ends_with("/cas-code-review.js")))
        })
}

#[derive(Default, Serialize)]
pub(crate) struct Report {
    pub outcome: &'static str,
    pub untracked: usize,
    pub staged: usize,
    pub commit: Option<String>,
    /// An attempted Git commit failed; this is not a file-body conflict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_error: Option<String>,
    /// Commit policy could not be read; the repair remains safely staged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_warning: Option<String>,
    pub conflicts: Vec<String>,
}

const CONFLICT_RECORD: &str = "cache/managed-file-conflicts.json";
const WRITE_RECORD: &str = "cache/managed-project-writes.json";

fn prior_writes(root: &Path) -> BTreeMap<String, String> {
    std::fs::read(root.join(".cas").join(WRITE_RECORD)).ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn fragment_hash(fragment: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(fragment.as_bytes()))
}

// Bootstrap the statusline migration on pre-receipt installs: an owned
// field may still be HEAD's value while another already equals this writer.
fn matches_settings_projection(local: &str, baseline: &str, expected: &str) -> bool {
    let values: Result<Vec<serde_json::Value>, _> = [local, baseline, expected]
        .into_iter().map(serde_json::from_str).collect();
    let Ok(values) = values else { return false; };
    let Some(local) = values[0].as_object() else { return false; };
    let Some(baseline) = values[1].as_object() else { return false; };
    let Some(expected) = values[2].as_object() else { return false; };
    local.keys().chain(baseline.keys()).chain(expected.keys()).all(|key|
        local.get(key) == baseline.get(key) || local.get(key) == expected.get(key))
}

/// Persist the last sync's preserved conflicts so SessionStart can report them;
/// a sync without conflicts clears the record (cas-3199). Unattended syncs otherwise
/// leave the refusal only in a timer journal nobody reads.
pub(crate) fn record_conflicts(cas_root: &Path, conflicts: &[String]) -> Result<()> {
    let path = cas_root.join(CONFLICT_RECORD);
    if conflicts.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(path.parent().context("conflict record parent")?)?;
    std::fs::write(path, serde_json::to_vec(conflicts)?)?;
    Ok(())
}

/// Conflicts that blocked this project's last sync; empty when none did.
pub(crate) fn recorded_conflicts(cas_root: &Path) -> Vec<String> {
    std::fs::read(cas_root.join(CONFLICT_RECORD))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) struct Reconcile {
    root: PathBuf,
    index: BTreeMap<String, (String, Vec<u8>)>,
    before: BTreeMap<String, Option<Vec<u8>>>,
    index_signature: Vec<u8>,
    pub report: Report,
}

impl Reconcile {
    /// Preflight before any writer, including malformed documents, unmerged
    /// files, symlinks and local edits to a managed body. Conflicts are visible
    /// and preserve each conflicting path in both the worktree and index.
    pub(crate) fn begin(root: &Path, harnesses: &[SupervisorCli]) -> Result<Option<Self>> {
        if authoring(root) || !git_root(root) {
            return Ok(None);
        }
        let writes = prior_writes(root);
        let mut index = BTreeMap::new();
        let mut conflicts = Vec::new();
        let index_signature = git(root, &["ls-files", "--stage", "-z"])?;
        for row in index_signature.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let (meta, path) = text(row)?.split_once('\t').context("Git index record")?;
            let fields: Vec<_> = meta.split_whitespace().collect();
            if fields[2] != "0" {
                conflicts.push(path.to_string());
                continue;
            }
            if !(harness_path(path)
                || path.starts_with(".cas/")
                || TRACKED.contains(&path)
                || matches!(
                    path,
                    ".env.worktree.template" | "scripts/worktree-boot.sh" | ".env.worktree"
                )
                || path.starts_with(".fallow/"))
            {
                continue;
            }
            if fields[0] == "160000" {
                conflicts.push(path.into());
                continue;
            }
            index.insert(
                path.to_string(),
                (
                    fields[0].to_string(),
                    git(root, &["cat-file", "blob", fields[1]])?,
                ),
            );
        }
        let paths = ignored_paths(root, harnesses);
        conflicts.extend(ignore_conflicts(root, &paths)?);
        let committed: BTreeMap<_, _> = index
            .keys()
            .map(|path| {
                (
                    path.clone(),
                    git(root, &["show", &format!("HEAD:{path}")]).unwrap_or_default(),
                )
            })
            .collect();
        let mut candidates: BTreeSet<String> = paths
            .iter()
            .filter(|p| !p.ends_with('/'))
            .map(|p| p.trim_start_matches('/').to_string())
            .collect();
        candidates.extend(TRACKED.iter().map(|p| p.to_string()));
        candidates.extend(index.keys().filter(|p| harness_path(p)).cloned());
        candidates
            .extend([".env.worktree.template", "scripts/worktree-boot.sh"].map(str::to_string));
        let projections = source_projections(root)?;
        let mut before = BTreeMap::new();
        for path in candidates {
            let absolute = root.join(&path);
            if let Some(link) = absolute.ancestors().take_while(|p| *p != root)
                .find(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())) {
                conflicts.push(link.strip_prefix(root)?.to_string_lossy().into_owned());
                continue;
            }
            let working = std::fs::read(&absolute).ok();
            let indexed = index.get(&path).map(|(_, b)| b);
            let baseline = committed.get(&path);
            for bytes in working.iter().chain(indexed) {
                if builtins::project_managed_body(&path)
                    && !text(bytes).is_ok_and(builtins::is_managed_by_cas)
                    && !builtins::matches_shipped_project_file(&path, bytes)
                {
                    conflicts.push(path.clone());
                }
                if (cassy_body(&path, bytes) || ignored(&path, &paths))
                    && baseline.is_some_and(|b| b != bytes)
                    && !builtins::matches_shipped_project_file(&path, bytes)
                    && projections.get(&path) != Some(bytes)
                    && bytes != crate::cli::init::CAS_SKILL.as_bytes()
                    && bytes != format!("{}\n", crate::cli::init::build_agents_section()).as_bytes()
                {
                    conflicts.push(path.clone());
                }
                if path == ".gitignore" || path.ends_with(".md") && TRACKED.contains(&path.as_str())
                {
                    let (a, b) = if path == ".gitignore" {
                        (IGNORE_BEGIN, IGNORE_END)
                    } else {
                        (BEGIN, END)
                    };
                    match block(text(bytes)?, a, b) {
                        Err(_) => conflicts.push(path.clone()),
                        Ok(Some(local)) if path == ".gitignore" && indexed.is_some() => {
                            let prior = block(text(baseline.unwrap())?, a, b)?;
                            if prior != Some(local)
                                && local.lines().any(|line| {
                                    let line = line.trim();
                                    !line.is_empty()
                                        && !line.starts_with('#')
                                        && !paths.iter().any(|p| p == line)
                                        && !builtins::previously_shipped_gitignore_entry(line)
                                })
                            {
                                conflicts.push(path.clone());
                            }
                        }
                        Ok(local) if path != ".gitignore" && indexed.is_some() => {
                            let prior = block(text(baseline.unwrap())?, a, b)?;
                            let expected = if path == "CLAUDE.md" {
                                crate::cli::init::build_cas_section()
                            } else {
                                crate::cli::init::build_agents_section()
                            };
                            if local != prior && local != Some(expected.as_str()) {
                                conflicts.push(path.clone());
                            }
                        }
                        _ => {}
                    }
                }
                if path.ends_with(".json") && TRACKED.contains(&path.as_str()) {
                    match serde_json::from_slice::<serde_json::Value>(bytes) {
                        Ok(v) if v.is_object() => {
                            if path == ".claude/settings.json"
                                && v.get("statusLine").is_some_and(|s| {
                                    !s.get("command")
                                        .and_then(|s| s.as_str())
                                        .is_some_and(|s| s.starts_with("cas "))
                                })
                            {
                                conflicts.push(path.clone());
                            }
                            if path == ".mcp.json"
                                && v.pointer("/mcpServers/cas/command")
                                    .is_some_and(|v| v != "cas")
                            {
                                conflicts.push(path.clone());
                            }
                        }
                        _ => conflicts.push(path.clone()),
                    }
                }
            }
            if path == ".codex/config.toml" {
                for bytes in working.iter().chain(indexed) {
                    match toml::from_str::<toml::Value>(text(bytes)?) {
                        Ok(v) if v.is_table() => {
                            for key in ["cs", "cas"] {
                                if v.get("mcp_servers")
                                    .and_then(|v| v.get(key))
                                    .and_then(|v| v.get("command"))
                                    .is_some_and(|v| v.as_str() != Some("cas"))
                                {
                                    conflicts.push(path.clone());
                                }
                            }
                        }
                        _ => conflicts.push(path.clone()),
                    }
                }
            }
            before.insert(path, working);
        }
        {
            let mut dirty = Vec::new();
            for path in TRACKED
                .iter()
                .filter(|p| p.ends_with(".json") || p.ends_with(".toml"))
            {
                if conflicts.iter().any(|conflict| conflict == path) { continue; }
                let Some(Some(bytes)) = before.get(*path) else {
                    continue;
                };
                let indexed = committed
                    .get(*path)
                    .map(|b| b.as_slice())
                    .unwrap_or_default();
                let baseline = owned_fragment(path, indexed)?;
                for candidate in std::iter::once(bytes.as_slice()).chain(index.get(*path).map(|(_, bytes)| bytes.as_slice())) {
                    let local = owned_fragment(path, candidate)?;
                    if local != baseline && writes.get(*path) != Some(&fragment_hash(&local)) {
                        dirty.push((path, local));
                    }
                }
            }
            if !dirty.is_empty() {
                let seed = before
                    .iter()
                    .filter_map(|(p, b)| b.clone().map(|b| (p.clone(), b)))
                    .collect();
                let projected = configuration_projection_excluding(root, &seed, &conflicts)?;
                for (path, local) in dirty {
                    let expected = std::fs::read(projected.path().join(path)).unwrap_or_default();
                    let expected = owned_fragment(path, &expected)?;
                    let baseline = owned_fragment(path, committed.get(*path).map(Vec::as_slice).unwrap_or_default())?;
                    if local != expected && !(*path == ".claude/settings.json"
                        && matches_settings_projection(&local, &baseline, &expected)) {
                        conflicts.push((*path).to_string());
                    }
                }
            }
        }
        conflicts.sort();
        conflicts.dedup();
        Ok(Some(Self {
            root: root.into(),
            index,
            before,
            index_signature,
            report: Report {
                outcome: if conflicts.is_empty() {
                    "planned"
                } else {
                    "conflicts"
                },
                conflicts,
                ..Report::default()
            },
        }))
    }

    pub(crate) fn allows(&self, path: &str) -> bool {
        !self.report.conflicts.iter().any(|conflict| Path::new(path).starts_with(conflict))
    }

    pub(crate) fn exclusions(&self) -> Vec<PathBuf> {
        self.report.conflicts.iter().map(|path| self.root.join(path)).collect()
    }

    pub(crate) fn finish(mut self, harnesses: &[SupervisorCli], cas_root: &Path) -> Result<Report> {
        let root = &self.root;
        if git(root, &["ls-files", "--stage", "-z"])? != self.index_signature {
            self.report.outcome = "staged";
            self.report
                .conflicts
                .push("index changed during update; rerun cas update".into());
            return Ok(self.report);
        }
        if self.allows(".gitignore") { builtins::ensure_builtin_gitignore(root, harnesses)?; }
        let mut writes = prior_writes(root);
        for path in TRACKED.iter().filter(|path| self.allows(path)) {
            if let Ok(bytes) = std::fs::read(root.join(path)) {
                writes.insert((*path).to_string(), fragment_hash(&owned_fragment(path, &bytes)?));
            }
        }
        let receipt = root.join(".cas").join(WRITE_RECORD);
        let body = serde_json::to_vec(&writes)?;
        if std::fs::read(&receipt).ok().as_deref() != Some(body.as_slice()) {
            std::fs::create_dir_all(receipt.parent().unwrap())?;
            std::fs::write(receipt, body)?;
        }
        let paths = ignored_paths(root, harnesses);
        for (path, (_, bytes)) in &self.index {
            if self.allows(".gitignore") && self.allows(path) && (ignored(path, &paths) || (!root.join(path).exists() && cassy_body(path, bytes))) {
                git(root, &["update-index", "--force-remove", "--", path])?;
                self.report.untracked += 1;
            }
        }
        // Render configuration from the INDEX's project content, never from
        // unstaged project edits. The same writers own the same keys in both
        // trees; the temporary tree cannot register trust or push anything.
        let seed = self
            .index
            .iter()
            .map(|(p, (_, b))| (p.clone(), b.clone()))
            .collect();
        let temp = configuration_projection_excluding(root, &seed, &self.report.conflicts)?;
        for path in TRACKED {
            if !self.allows(path) || ignored(path, &paths) {
                continue;
            }
            let indexed = self
                .index
                .get(*path)
                .map(|(_, b)| b.as_slice())
                .unwrap_or_default();
            let desired = if matches!(*path, ".gitignore" | "CLAUDE.md" | "AGENTS.md") {
                let (a, b) = if *path == ".gitignore" {
                    (IGNORE_BEGIN, IGNORE_END)
                } else {
                    (BEGIN, END)
                };
                let current = std::fs::read_to_string(root.join(path)).unwrap_or_default();
                let Some(owned) = block(&current, a, b)? else {
                    continue;
                };
                replace_block(text(indexed)?, owned, a, b)?.into_bytes()
            } else {
                let Ok(desired) = std::fs::read(temp.path().join(path)) else {
                    continue;
                };
                desired
            };
            if desired != indexed {
                self.stage(path, &desired)?;
            }
        }
        for path in [".env.worktree.template", "scripts/worktree-boot.sh"] {
            // These become project-owned after creation. Only a file this
            // invocation created may be committed automatically.
            if self.allows(path) && !self.index.contains_key(path) {
                if let Ok(bytes) = std::fs::read(root.join(path)) {
                    let expected = if path.ends_with(".sh") {
                        crate::cli::factory_tooling::BOOT_SCRIPT
                    } else {
                        crate::cli::factory_tooling::ENV_TEMPLATE
                    };
                    if bytes == expected.as_bytes() {
                        self.stage(path, &bytes)?;
                    }
                }
            }
        }
        let pending: Vec<_> = git(root, &["diff", "--cached", "--name-only", "-z"])?
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect();
        self.report.staged = pending.len();
        self.report.outcome = if pending.is_empty() {
            "current"
        } else if !self.report.conflicts.is_empty()
            || !may_commit_managed_files(root, cas_root).unwrap_or_else(|_| {
                self.report.commit_warning = Some(
                    "Commit guard config unreadable; repair staged. Fix config, then retry.".into(),
                );
                false
            })
            || !staged_sync_only(root).unwrap_or(false)
        {
            if self.report.commit_warning.is_none() {
                self.report.commit_warning = Some(
                    "Changes remain staged; review git diff --cached before committing.".into(),
                );
            }
            "staged"
        } else {
            // No repository configuration is changed. Normal Git hooks still
            // run; a refusal leaves the staged repair inspectable.
            match git(
                root,
                &[
                    "-c",
                    "user.name=Cassy",
                    "-c",
                    "user.email=cassy@localhost",
                    "commit",
                    "-m",
                    crate::worktree::git::MANAGED_SYNC_SUBJECT,
                ],
            ) {
                Ok(_) => {
                    self.report.commit = Some(
                        String::from_utf8(git(root, &["rev-parse", "HEAD"])?)?
                            .trim()
                            .into(),
                    );
                    "committed"
                }
                Err(error) => {
                    let hook = git(
                        root,
                        &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
                    )
                        .ok()
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                        .map(|path| path.trim().to_owned())
                        .filter(|path| Path::new(path).is_dir())
                        .map(|path| format!("; hooks directory {path}"))
                        .unwrap_or_default();
                    self.report.commit_error = Some(format!(
                        "Managed-files commit failed: {error}{hook}; run `git config --show-origin --show-scope --get core.hooksPath` in this checkout, review the hooks and staged diff with the supervisor/operator, then commit from an authorized checkout after restoring the intended hook scope"
                    ));
                    "commit_failed"
                }
            }
        };
        if !self.report.conflicts.is_empty() { self.report.outcome = "partial"; }
        Ok(self.report)
    }

    fn stage(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        child.stdin.take().unwrap().write_all(bytes)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!("could not stage {path}");
        }
        let hash = text(&output.stdout)?.trim();
        let mode =
            self.index
                .get(path)
                .map(|(m, _)| m.as_str())
                .unwrap_or(if path.ends_with(".sh") {
                    "100755"
                } else {
                    "100644"
                });
        git(
            &self.root,
            &["update-index", "--add", "--cacheinfo", mode, hash, path],
        )?;
        Ok(())
    }
}

/// True only when every staged blob/mode can be produced from HEAD by sync.
/// In particular, a managed path may also contain staged project content.
/// Read-only: callers use this both for commit admission and merge recovery.
pub(crate) fn staged_sync_only(root: &Path) -> Result<bool> {
    let harnesses = harnesses(root);
    let Some(plan) = Reconcile::begin(root, &harnesses)? else { return Ok(false); };
    if !plan.report.conflicts.is_empty() { return Ok(false); }
    let pending = git(root, &["diff", "--cached", "--name-only", "-z"])?;
    if pending.is_empty() { return Ok(false); }
    let committed: BTreeMap<_, _> = TRACKED.iter().filter_map(|path| {
        git(root, &["show", &format!("HEAD:{path}")]).ok().map(|bytes| ((*path).to_string(), bytes))
    }).collect();
    let projected = configuration_projection_excluding(root, &committed, &[])?;
    let ignored_paths = ignored_paths(root, &harnesses);
    for path in pending.split(|b| *b == 0).filter(|path| !path.is_empty()) {
        let path = text(path)?;
        let head = git(root, &["ls-tree", "HEAD", "--", path])?;
        let head_mode = text(&head)?.split_whitespace().next();
        let Some((mode, indexed)) = plan.index.get(path) else {
            // Sync untracks generated files, retaining their working copies.
            let retired = !root.join(path).exists()
                && git(root, &["show", &format!("HEAD:{path}")]).is_ok_and(|bytes| cassy_body(path, &bytes));
            if (ignored(path, &ignored_paths) || retired)
                && matches!(head_mode, Some("100644" | "100755")) { continue; }
            return Ok(false);
        };
        let expected_mode = head_mode.unwrap_or(if path.ends_with(".sh") { "100755" } else { "100644" });
        if mode != expected_mode || !matches!(mode.as_str(), "100644" | "100755") { return Ok(false); }
        let expected = if matches!(path, ".env.worktree.template" | "scripts/worktree-boot.sh") && head.is_empty() {
            if path.ends_with(".sh") { crate::cli::factory_tooling::BOOT_SCRIPT.as_bytes().to_vec() }
            else { crate::cli::factory_tooling::ENV_TEMPLATE.as_bytes().to_vec() }
        } else if !TRACKED.contains(&path) {
            return Ok(false);
        } else if matches!(path, ".gitignore" | "CLAUDE.md" | "AGENTS.md") {
            let (a, b) = if path == ".gitignore" { (IGNORE_BEGIN, IGNORE_END) } else { (BEGIN, END) };
            let Some(owned) = block(text(indexed)?, a, b)? else { return Ok(false); };
            let baseline = committed.get(path).map(Vec::as_slice).unwrap_or_default();
            replace_block(text(baseline)?, owned, a, b)?.into_bytes()
        } else {
            std::fs::read(projected.path().join(path))?
        };
        if *indexed != expected { return Ok(false); }
    }
    Ok(true)
}

fn configuration_projection_excluding(
    root: &Path, seed: &BTreeMap<String, Vec<u8>>, excluded: &[String],
) -> Result<tempfile::TempDir> {
    let allowed = |path: &str| !excluded.iter().any(|conflict| Path::new(path).starts_with(conflict));
    let temp = tempfile::tempdir()?;
    for path in TRACKED {
        if !allowed(path) { continue; }
        if let Some(bytes) = seed.get(*path) {
            let dest = temp.path().join(path);
            std::fs::create_dir_all(dest.parent().unwrap())?;
            std::fs::write(dest, bytes)?;
        }
    }
    std::fs::create_dir_all(temp.path().join(".cas"))?;
    if let Ok(config) = std::fs::read(root.join(".cas/config.toml")) {
        std::fs::write(temp.path().join(".cas/config.toml"), config)?;
    }
    if allowed(".claude/settings.json") { crate::cli::hook::configure_claude_hooks_for_index(temp.path(), root)?; }
    if allowed(".mcp.json") { crate::cli::hook::configure_mcp_server(temp.path())?; }
    if root.join(".codex").is_dir() {
        crate::cli::hook::config_gen::configure_codex_project_files(
            temp.path(), !allowed(".codex/config.toml"), !allowed(".codex/hooks.json"),
        )?;
    }
    Ok(temp)
}

// Project-only edits do not become a managed-drift finding.
fn owned_fragment(path: &str, bytes: &[u8]) -> Result<String> {
    let s = text(bytes)?;
    if matches!(path, ".gitignore" | "CLAUDE.md" | "AGENTS.md") {
        let (a, b) = if path == ".gitignore" {
            (IGNORE_BEGIN, IGNORE_END)
        } else {
            (BEGIN, END)
        };
        return Ok(block(s, a, b)?.unwrap_or_default().to_string());
    }
    if path.ends_with(".toml") {
        let value: toml::Value = if s.is_empty() {
            toml::Value::Table(Default::default())
        } else {
            toml::from_str(s)?
        };
        return Ok(format!(
            "{:?}",
            value
                .get("mcp_servers")
                .and_then(|v| v.get("cs").or_else(|| v.get("cas")))
        ));
    }
    let v: serde_json::Value = if s.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(s)?
    };
    if path == ".mcp.json" {
        return Ok(v
            .pointer("/mcpServers/cas")
            .unwrap_or(&serde_json::Value::Null)
            .to_string());
    }
    let mut owned = serde_json::Map::new();
    if let Some(events) = v.get("hooks").and_then(|h| h.as_object()) {
        let mut hooks = serde_json::Map::new();
        for (event, groups) in events {
            let groups: Vec<_> = groups
                .as_array()
                .into_iter()
                .flatten()
                .filter(|g| {
                    g.get("hooks")
                        .and_then(|h| h.as_array())
                        .is_some_and(|hooks| {
                            hooks.iter().any(|h| {
                                h.get("command")
                                    .and_then(|c| c.as_str())
                                    .is_some_and(cassy_command)
                            })
                        })
                })
                .cloned()
                .collect();
            if !groups.is_empty() {
                hooks.insert(event.clone(), groups.into());
            }
        }
        if !hooks.is_empty() {
            owned.insert("hooks".into(), hooks.into());
        }
    }
    if v.pointer("/statusLine/command")
        .and_then(|v| v.as_str())
        .is_some_and(cassy_command)
    {
        owned.insert("statusLine".into(), v["statusLine"].clone());
    }
    let allow: Vec<_> = v
        .pointer("/permissions/allow")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(|v| {
            v.as_str().is_some_and(|v| {
                v.contains("cas ")
                    || v.contains("cas:")
                    || v.contains("mcp__cas")
                    || v.contains("mcp__cs")
            })
        })
        .cloned()
        .collect();
    if !allow.is_empty() {
        owned.insert("allow".into(), allow.into());
    }
    Ok(serde_json::Value::Object(owned).to_string())
}

// Doctor compares permission membership; repair retains its existing byte/order semantics.
fn drift_fragment(path: &str, bytes: &[u8]) -> Result<String> {
    let fragment = owned_fragment(path, bytes)?;
    if path != ".claude/settings.json" {
        return Ok(fragment);
    }
    let mut owned: serde_json::Value = serde_json::from_str(&fragment)?;
    if let Some(allow) = owned.get_mut("allow").and_then(|v| v.as_array_mut()) {
        allow.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        allow.dedup();
    }
    Ok(owned.to_string())
}

pub(crate) fn drift(root: &Path, harnesses: &[SupervisorCli]) -> Result<usize> {
    let Some(plan) = Reconcile::begin(root, harnesses)? else {
        return Ok(0);
    };
    let paths = ignored_paths(root, harnesses);
    let mut count = plan.report.conflicts.len();
    let mut seen = BTreeSet::new();
    for row in git(root, &["ls-tree", "-r", "--name-only", "-z", "HEAD"])
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|r| !r.is_empty())
    {
        let path = text(row)?;
        if ignored(path, &paths) {
            count += 1;
            seen.insert(path.to_string());
        } else if harness_path(path)
            && !root.join(path).exists()
            && git(root, &["show", &format!("HEAD:{path}")]).is_ok_and(|b| cassy_body(path, &b))
        {
            count += 1;
            seen.insert(path.to_string());
        }
    }
    for (path, (_, bytes)) in &plan.index {
        if !seen.contains(path)
            && (ignored(path, &paths) || (!root.join(path).exists() && cassy_body(path, bytes)))
        {
            count += 1;
        }
    }
    let current = std::fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    let expected = builtins::render_builtin_gitignore_block(&paths);
    if block(&current, IGNORE_BEGIN, IGNORE_END)?.map(str::trim) != Some(expected.trim()) {
        count += 1;
    }
    for path in TRACKED {
        if !plan.allows(path) { continue; }
        let Some(Some(bytes)) = plan.before.get(*path) else {
            continue;
        };
        let committed = git(root, &["show", &format!("HEAD:{path}")]).unwrap_or_default();
        if !ignored(path, &paths)
            && drift_fragment(path, bytes)? != drift_fragment(path, &committed)?
        {
            count += 1;
        }
    }
    Ok(count)
}
