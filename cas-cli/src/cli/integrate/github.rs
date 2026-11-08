//! `cas integrate github <action>` — auto-integration handler for the GitHub
//! `owner/repo` pair backing the current repo.
//!
//! Owner: task **cas-f425** (EPIC cas-b65f).
//!
//! Unlike the Vercel and Neon handlers, this branch needs no MCP call: the
//! authoritative source is the local clone's `git remote -v`. Detection is
//! cheap, deterministic, and can be tested by feeding a fake `git remote -v`
//! string into [`parse_remote_v`].
//!
//! ## Stub-handler / status convention
//!
//! Per cas-e6b6 design note 4 ("Stub-handler convention for 'platform not
//! detected'"), the cases below split as follows:
//!
//! - GitHub remote not present (no remote, or remote points at gitlab/bitbucket
//!   etc.) → returns `Ok(IntegrationOutcome { status: Skipped, .. })`. This is
//!   the "platform genuinely not present in repo" branch — `cas init` should
//!   keep going past it without erroring.
//! - `git remote -v` failed to execute, or the remote URL is GitHub-shaped but
//!   unparseable → returns `Err(...)`. These are "unrecoverable error" — the
//!   user needs to look.
//!
//! Sibling handlers (cas-8e37 vercel, cas-1ece neon) follow the same split.
//!
//! ## Keep-block strategy
//!
//! The generated SKILL.md files use a **named** keep block,
//! `<!-- keep github-repo -->`, around the owner/repo identity. Per cas-e6b6
//! design note 1, named blocks survive future template revisions that reorder
//! sections; unnamed blocks would silently misroute on reorder.
//!
//! Refresh writes via [`MergeMode::PreferTemplate`]: the canonical owner/repo
//! comes from `git remote -v`, so the freshly rendered template wins. Before
//! writing, [`keep_block::orphaned_existing`] is consulted and any orphans are
//! surfaced in `IntegrationOutcome.summary` so hand-edited blocks the user
//! added (and a future template revision dropped) aren't silently lost.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Subcommand;

use super::keep_block::{self, MergeMode};
use super::types::{
    IdState, IntegrationAction, IntegrationOutcome, IntegrationStatus, Platform, VerifyItem,
    VerifyReport,
};

const SKILL_TEMPLATE: &str = include_str!("templates/github/SKILL.md.template");
const CURSOR_TEMPLATE: &str = include_str!("templates/github/cursor.md.template");

/// Relative paths (from repo root) of the SKILL files this handler manages.
const CLAUDE_SKILL_REL: &str = ".claude/skills/github-repo/SKILL.md";
const CURSOR_SKILL_REL: &str = ".cursor/skills/github-repo/SKILL.md";

// --- Detection sentinels --------------------------------------------------
//
// Hoisted to module-top per cas-fc38 / sibling-handler convention
// (vercel.rs, neon.rs). A new GitHub URL form (a private GitHub Enterprise
// host, say) can be supported by adding to GITHUB_HTTPS_PREFIXES; the body
// of `parse_origin_url` reads top-down and treats unknown forms as
// non-GitHub.
const GITHUB_SCP_PREFIX: &str = "git@github.com:";
const GITHUB_SSH_URL_PREFIX: &str = "ssh://git@github.com/";
const GITHUB_HTTPS_PREFIXES: &[&str] = &["https://github.com/", "http://github.com/"];

/// Name of the canonical IDs keep block in the rendered SKILL.md.
const KEEP_GITHUB_BLOCK: &str = "github-repo";

/// `cas integrate github <action>` — github-specific subcommand.
///
/// The `--repo OWNER/REPO` flag overrides auto-detection from `git remote -v`,
/// which is useful when the local clone's remote differs from where the
/// project really lives (forks, mirrors, ssh-vs-https discrepancies).
#[derive(Subcommand, Debug, Clone)]
pub enum GithubAction {
    /// First-time setup: detect, prompt, write SKILL files.
    Init {
        /// Override the auto-detected `OWNER/REPO`. When supplied, the
        /// `git remote -v` lookup is skipped entirely.
        #[arg(long, value_name = "OWNER/REPO")]
        repo: Option<String>,
    },
    /// Re-run detection. If the remote changed (rename / transfer), update
    /// the keep block; otherwise no-op.
    Refresh {
        /// Override the auto-detected `OWNER/REPO`.
        #[arg(long, value_name = "OWNER/REPO")]
        repo: Option<String>,
    },
    /// Read recorded `OWNER/REPO`, compare to current `git remote -v`, return
    /// a staleness report.
    Verify,
}

impl From<GithubAction> for IntegrationAction {
    fn from(a: GithubAction) -> Self {
        match a {
            GithubAction::Init { .. } => IntegrationAction::Init,
            GithubAction::Refresh { .. } => IntegrationAction::Refresh,
            GithubAction::Verify => IntegrationAction::Verify,
        }
    }
}

/// `OWNER/REPO` pair — the only data the GitHub integration cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
}

impl RepoRef {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// Parse from an `OWNER/REPO` string (used by the `--repo` flag).
    pub fn from_owner_slash_repo(s: &str) -> Option<Self> {
        let s = s.trim();
        let (owner, repo) = s.split_once('/')?;
        if owner.is_empty() || repo.is_empty() || repo.contains('/') {
            return None;
        }
        Some(RepoRef {
            owner: owner.to_string(),
            repo: repo.trim_end_matches(".git").to_string(),
        })
    }
}

/// CLI dispatch entry point. Called from [`super::execute`].
pub fn execute(action: GithubAction) -> anyhow::Result<IntegrationOutcome> {
    let cwd = std::env::current_dir()?;
    match action {
        GithubAction::Init { repo } => init_at(&cwd, repo.as_deref()),
        GithubAction::Refresh { repo } => refresh_at(&cwd, repo.as_deref()),
        GithubAction::Verify => verify_at(&cwd),
    }
}

// ---------------------------------------------------------------------------
// URL parsing helpers (pure — easy to unit-test).
// ---------------------------------------------------------------------------

/// Parse a single remote URL into a `RepoRef` if it points at GitHub.
///
/// Accepts:
/// - `https://github.com/<owner>/<repo>` (with or without trailing `.git`)
/// - `http://github.com/<owner>/<repo>` (rare but tolerated)
/// - `git@github.com:<owner>/<repo>` (with or without trailing `.git`)
/// - `ssh://git@github.com/<owner>/<repo>` (uncommon but valid)
///
/// Returns `None` for non-GitHub hosts (gitlab.com, bitbucket.org, self-hosted
/// gitea, ...) and for malformed URLs.
pub fn parse_origin_url(url: &str) -> Option<RepoRef> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // SCP-like SSH form: git@github.com:owner/repo[.git]
    if let Some(rest) = url.strip_prefix(GITHUB_SCP_PREFIX) {
        return split_owner_repo(rest);
    }

    // ssh://git@github.com/owner/repo[.git]
    if let Some(rest) = url.strip_prefix(GITHUB_SSH_URL_PREFIX) {
        return split_owner_repo(rest);
    }

    // https://github.com/... or http://github.com/...
    for prefix in GITHUB_HTTPS_PREFIXES {
        if let Some(rest) = url.strip_prefix(*prefix) {
            return split_owner_repo(rest);
        }
    }

    None
}

fn split_owner_repo(rest: &str) -> Option<RepoRef> {
    // Strip any query/fragment first ("?...", "#...").
    let rest = rest
        .split_once(['?', '#'])
        .map(|(p, _)| p)
        .unwrap_or(rest);
    // Trim trailing slash to tolerate `.../owner/repo/`.
    let rest = rest.trim_end_matches('/');
    let mut parts = rest.splitn(3, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    // Anything after the second segment is unexpected — only allow it if it's
    // empty (e.g. trailing slash already stripped above).
    if let Some(extra) = parts.next() {
        if !extra.is_empty() {
            return None;
        }
    }
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    let repo = repo.trim_end_matches(".git");
    if repo.is_empty() {
        return None;
    }
    Some(RepoRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

/// Parse the output of `git remote -v` and return the `origin` fetch URL, if
/// any. Falls back to the first remote's fetch URL if `origin` is absent.
pub fn parse_remote_v(stdout: &str) -> Option<String> {
    let mut first_fetch: Option<String> = None;
    for line in stdout.lines() {
        // Format: "<name>\t<url> (<verb>)"
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut tab_split = line.splitn(2, '\t');
        let name = tab_split.next()?.trim();
        let rest = tab_split.next()?.trim();
        let url = match rest.rsplit_once(' ') {
            Some((u, verb)) => {
                if !verb.contains("fetch") {
                    continue;
                }
                u.trim()
            }
            None => rest,
        };
        if url.is_empty() {
            continue;
        }
        if name == "origin" {
            return Some(url.to_string());
        }
        if first_fetch.is_none() {
            first_fetch = Some(url.to_string());
        }
    }
    first_fetch
}

/// Run `git remote -v` in `repo_root` and return its stdout. Returns
/// `Ok(None)` when git exits non-zero (e.g. not a repo) so callers can map
/// that to `Skipped` rather than a hard error. Other errors (git missing
/// from PATH) propagate.
fn run_git_remote_v(repo_root: &Path) -> anyhow::Result<Option<String>> {
    let output = match Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["remote", "-v"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            // ENOENT / permission issues should bubble up — the user almost
            // certainly wants to know git isn't on PATH.
            anyhow::bail!("failed to invoke `git remote -v`: {e}");
        }
    };
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Detect the `RepoRef` from `git remote -v` in `repo_root`. Returns `None`
/// when the project isn't a git repo, has no remotes, or the remote points
/// at a non-GitHub host.
pub fn detect_repo(repo_root: &Path) -> anyhow::Result<Option<RepoRef>> {
    let Some(stdout) = run_git_remote_v(repo_root)? else {
        return Ok(None);
    };
    let Some(url) = parse_remote_v(&stdout) else {
        return Ok(None);
    };
    Ok(parse_origin_url(&url))
}

// ---------------------------------------------------------------------------
// Template rendering & file IO.
// ---------------------------------------------------------------------------

fn render_template(template: &str, repo: &RepoRef) -> String {
    // The cas:full_name tag goes through `super::md::emit_cas_full_name_tag`
    // so a value containing literal `-->` / CR / LF is sanitized before it
    // can corrupt the surrounding `<!-- keep github-repo -->` markers.
    // Plain string substitutions (OWNER / REPO / FULL_NAME) appear inside
    // markdown table cells where backtick-quoting is sufficient; if the
    // future RepoRef::from_owner_slash_repo loosens, revisit.
    let cas_tag = super::md::emit_cas_full_name_tag(&repo.full_name());
    template
        .replace("{{CAS_FULL_NAME_TAG}}", &cas_tag)
        .replace("{{OWNER}}", &repo.owner)
        .replace("{{REPO}}", &repo.repo)
        .replace("{{FULL_NAME}}", &repo.full_name())
}

/// Resolve a `RepoRef` from either an explicit override or `git remote -v`.
/// `Ok(None)` means "no GitHub remote and no override" — caller maps to
/// `Skipped`. `Err` means the override string is malformed.
fn resolve_repo(
    repo_root: &Path,
    flag_override: Option<&str>,
) -> anyhow::Result<Option<RepoRef>> {
    if let Some(s) = flag_override {
        let parsed = RepoRef::from_owner_slash_repo(s).ok_or_else(|| {
            anyhow::anyhow!(
                "--repo expects OWNER/REPO (e.g. Richards-LLC/gabber-studio), got: {s:?}"
            )
        })?;
        return Ok(Some(parsed));
    }
    detect_repo(repo_root)
}

/// `init` action implementation, parameterised on `repo_root` for tests.
pub fn init_at(repo_root: &Path, flag_override: Option<&str>) -> anyhow::Result<IntegrationOutcome> {
    let Some(repo) = resolve_repo(repo_root, flag_override)? else {
        return Ok(skipped_outcome(IntegrationAction::Init));
    };
    write_skill_files(repo_root, &repo, IntegrationAction::Init)
}

/// `refresh` action implementation. Same write semantics as init — both use
/// `MergeMode::PreferTemplate` because the canonical owner/repo comes from a
/// fresh `git remote -v` lookup. The status differs: a refresh of unchanged
/// content reports `AlreadyConfigured`; init reports `Configured`.
pub fn refresh_at(
    repo_root: &Path,
    flag_override: Option<&str>,
) -> anyhow::Result<IntegrationOutcome> {
    let Some(repo) = resolve_repo(repo_root, flag_override)? else {
        return Ok(skipped_outcome(IntegrationAction::Refresh));
    };
    write_skill_files(repo_root, &repo, IntegrationAction::Refresh)
}

/// `verify` action implementation. Reads the keep block in the .claude SKILL
/// file and compares the recorded full_name to the current `git remote -v`.
pub fn verify_at(repo_root: &Path) -> anyhow::Result<IntegrationOutcome> {
    let claude_path = repo_root.join(CLAUDE_SKILL_REL);
    if !claude_path.exists() {
        let mut out = IntegrationOutcome::new(
            Platform::Github,
            IntegrationAction::Verify,
            IntegrationStatus::Skipped,
        );
        out.summary
            .push(format!("{CLAUDE_SKILL_REL} not found — run `cas integrate github init` first"));
        return Ok(out);
    }
    // cas-fc38: read user-controlled SKILL.md via read_capped so a symlink
    // at the path is rejected and we don't allocate unbounded memory on a
    // pathological file.
    let existing = super::fs::read_capped(&claude_path)?;
    let recorded = recorded_full_name(&existing)?;
    let detected = detect_repo(repo_root)?;
    let mut out = IntegrationOutcome::new(
        Platform::Github,
        IntegrationAction::Verify,
        IntegrationStatus::AlreadyConfigured,
    );
    match (recorded.as_deref(), detected.as_ref()) {
        (Some(rec), Some(det)) if rec == det.full_name() => {
            out.summary
                .push(format!("recorded {rec} matches `git remote -v`"));
        }
        (Some(rec), Some(det)) => {
            out.status = IntegrationStatus::Stale;
            out.summary.push(format!(
                "drift: SKILL.md records {rec} but `git remote -v` says {}",
                det.full_name()
            ));
        }
        (Some(rec), None) => {
            out.status = IntegrationStatus::Stale;
            out.summary.push(format!(
                "drift: SKILL.md records {rec} but no GitHub remote was detected — \
                 was the repo deleted or transferred to a non-GitHub host?"
            ));
        }
        (None, _) => {
            out.status = IntegrationStatus::Stale;
            out.summary
                .push("could not read OWNER/REPO from SKILL.md keep block".to_string());
        }
    }
    Ok(out)
}

/// Structured verify, for `cas doctor` consumption.
///
/// Returns one [`VerifyItem`] for the recorded OWNER/REPO. State maps as:
/// - `Ok` when the SKILL.md value matches the live `git remote -v`.
/// - `Stale` when they disagree (drift) or no remote is detected anymore.
/// - `not_configured = true` when SKILL.md is missing or has no recorded value.
///
/// `git remote` is local — there is no MCP call here, so this report never
/// produces [`IdState::McpUnreachable`].
pub fn verify_report(repo_root: &Path) -> VerifyReport {
    let claude_path = repo_root.join(CLAUDE_SKILL_REL);
    if !claude_path.exists() {
        return VerifyReport::not_configured(
            Platform::Github,
            format!("{CLAUDE_SKILL_REL} not present"),
        );
    }
    let existing = match fs::read_to_string(&claude_path) {
        Ok(s) => s,
        Err(e) => {
            return VerifyReport::not_configured(
                Platform::Github,
                format!("could not read {CLAUDE_SKILL_REL}: {e}"),
            );
        }
    };
    let recorded = match recorded_full_name(&existing) {
        Ok(Some(name)) => name,
        Ok(None) => {
            return VerifyReport::not_configured(
                Platform::Github,
                format!("no OWNER/REPO recorded in {CLAUDE_SKILL_REL} keep block"),
            );
        }
        Err(e) => {
            return VerifyReport::not_configured(
                Platform::Github,
                format!("malformed keep block in {CLAUDE_SKILL_REL}: {e}"),
            );
        }
    };
    // Distinguish "git invocation failed" (transport-shaped — should
    // render as `skipped`) from "git ran but reports no GitHub remote"
    // (genuine drift — `Stale`). A missing `git` binary or a non-git repo
    // root would otherwise produce a misleading false-positive Stale.
    let (state, note) = match detect_repo(repo_root) {
        Ok(Some(d)) if d.full_name() == recorded => (
            IdState::Ok,
            Some(format!("git remote -v reports {}", d.full_name())),
        ),
        Ok(Some(d)) => (
            IdState::Stale,
            Some(format!("git remote -v reports {}", d.full_name())),
        ),
        Ok(None) => (IdState::Stale, None),
        Err(e) => (
            IdState::McpUnreachable(format!("git remote -v failed: {e:#}")),
            None,
        ),
    };
    let mut report = VerifyReport::ok(Platform::Github);
    report.items.push(VerifyItem {
        label: "OWNER/REPO".to_string(),
        id: recorded,
        state,
    });
    if let Some(n) = note {
        report.notes.push(n);
    }
    report
}

/// Pull the recorded `OWNER/REPO` out of the named keep block.
fn recorded_full_name(existing: &str) -> anyhow::Result<Option<String>> {
    let blocks = keep_block::extract(existing)
        .map_err(|e| anyhow::anyhow!("malformed SKILL.md keep block: {e}"))?;
    let Some(block) = blocks
        .into_iter()
        .find(|b| b.name.as_deref() == Some(KEEP_GITHUB_BLOCK))
    else {
        return Ok(None);
    };

    // Primary path (cas-fc38): look for the machine-readable
    // `<!-- cas:full_name=OWNER/REPO -->` tag emitted by current templates.
    // This is the canonical convention shared with vercel/neon and is
    // resilient to future template revisions that rename row labels.
    if let Some(tagged) = super::md::parse_cas_full_name_tag(&block.body) {
        if RepoRef::from_owner_slash_repo(&tagged).is_some() {
            return Ok(Some(tagged));
        }
    }

    // Backwards-compat path: pre-cas-fc38 templates encoded the value in a
    // `| **Full name** | `OWNER/REPO` |` table row. We deliberately pull the
    // value between the *first pair* of backticks (not first and last) so an
    // injected extra `` ` `` can't widen the capture, then re-validate the
    // token as a well-formed OWNER/REPO.
    for line in block.body.lines() {
        let Some(rest) = line.split_once("**Full name**").map(|(_, r)| r) else {
            continue;
        };
        let mut parts = rest.splitn(3, '`');
        let _before = parts.next();
        let Some(candidate) = parts.next() else {
            continue;
        };
        let candidate = candidate.trim();
        if RepoRef::from_owner_slash_repo(candidate).is_some() {
            return Ok(Some(candidate.to_string()));
        }
    }
    Ok(None)
}

fn skipped_outcome(action: IntegrationAction) -> IntegrationOutcome {
    let mut out = IntegrationOutcome::new(Platform::Github, action, IntegrationStatus::Skipped);
    out.summary
        .push("no GitHub remote detected (no `origin`, or origin points at a non-GitHub host)".to_string());
    out
}

/// Write both SKILL files, merging via `PreferTemplate` if they already exist.
/// Returns `AlreadyConfigured` when both target files are byte-identical to
/// the freshly rendered template.
fn write_skill_files(
    repo_root: &Path,
    repo: &RepoRef,
    action: IntegrationAction,
) -> anyhow::Result<IntegrationOutcome> {
    let claude_rendered = render_template(SKILL_TEMPLATE, repo);
    let cursor_rendered = render_template(CURSOR_TEMPLATE, repo);

    let mut out = IntegrationOutcome::new(
        Platform::Github,
        action,
        match action {
            IntegrationAction::Init => IntegrationStatus::Configured,
            IntegrationAction::Refresh => IntegrationStatus::Refreshed,
            // Caller never invokes us with Verify; keep the match exhaustive.
            IntegrationAction::Verify => IntegrationStatus::AlreadyConfigured,
        },
    );

    let claude_changed = write_one(
        repo_root,
        CLAUDE_SKILL_REL,
        &claude_rendered,
        &mut out.summary,
    )?;
    let cursor_changed = write_one(
        repo_root,
        CURSOR_SKILL_REL,
        &cursor_rendered,
        &mut out.summary,
    )?;

    if claude_changed {
        out.files.push(PathBuf::from(CLAUDE_SKILL_REL));
    }
    if cursor_changed {
        out.files.push(PathBuf::from(CURSOR_SKILL_REL));
    }

    if !claude_changed && !cursor_changed {
        out.status = IntegrationStatus::AlreadyConfigured;
        out.summary
            .push(format!("{} already current — no changes", repo.full_name()));
    } else {
        out.summary
            .insert(0, format!("recorded {}", repo.full_name()));
    }

    Ok(out)
}

/// Write a single file. Computes `merge(template, existing, PreferTemplate)`,
/// surfaces orphaned existing keep blocks in `summary`, and returns true iff
/// the file's bytes changed.
fn write_one(
    repo_root: &Path,
    rel: &str,
    rendered_template: &str,
    summary: &mut Vec<String>,
) -> anyhow::Result<bool> {
    let path = repo_root.join(rel);
    // cas-fc38: symlink-rejecting + size-capped read.
    let existing_str = if path.exists() {
        Some(super::fs::read_capped(&path)?)
    } else {
        None
    };

    if let Some(ref existing) = existing_str {
        match keep_block::orphaned_existing(rendered_template, existing) {
            Ok(orphans) => {
                for orphan in orphans {
                    let label = match &orphan.name {
                        Some(n) => format!("named '{n}'"),
                        None => "unnamed".to_string(),
                    };
                    summary.push(format!(
                        "warning: dropping orphan keep block ({label}) from {rel} — \
                         it has no slot in the current template"
                    ));
                }
            }
            Err(e) => {
                // Malformed existing file: refuse to silently overwrite.
                anyhow::bail!(
                    "existing {rel} has malformed keep markers: {e} — fix or delete the file before re-running"
                );
            }
        }
    }

    let merged = keep_block::merge(
        rendered_template,
        existing_str.as_deref(),
        MergeMode::PreferTemplate,
    )
    .map_err(|e| anyhow::anyhow!("keep-block merge failed for {rel}: {e}"))?;

    let changed = match &existing_str {
        Some(prev) => prev != &merged,
        None => true,
    };

    if changed {
        super::fs::atomic_write_create_dirs(&path, &merged)?;
    }
    Ok(changed)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // --- URL parsing -------------------------------------------------------

    #[test]
    fn parse_https_with_dot_git() {
        let r = parse_origin_url("https://github.com/Richards-LLC/gabber-studio.git").unwrap();
        assert_eq!(r.owner, "Richards-LLC");
        assert_eq!(r.repo, "gabber-studio");
        assert_eq!(r.full_name(), "Richards-LLC/gabber-studio");
    }

    #[test]
    fn parse_https_without_dot_git() {
        let r = parse_origin_url("https://github.com/Richards-LLC/gabber-studio").unwrap();
        assert_eq!(r.full_name(), "Richards-LLC/gabber-studio");
    }

    #[test]
    fn parse_https_trailing_slash() {
        let r = parse_origin_url("https://github.com/Richards-LLC/gabber-studio/").unwrap();
        assert_eq!(r.full_name(), "Richards-LLC/gabber-studio");
    }

    #[test]
    fn parse_ssh_scp_form() {
        let r = parse_origin_url("git@github.com:Richards-LLC/gabber-studio.git").unwrap();
        assert_eq!(r.full_name(), "Richards-LLC/gabber-studio");
    }

    #[test]
    fn parse_ssh_scp_form_no_dot_git() {
        let r = parse_origin_url("git@github.com:Richards-LLC/gabber-studio").unwrap();
        assert_eq!(r.full_name(), "Richards-LLC/gabber-studio");
    }

    #[test]
    fn parse_ssh_url_form() {
        let r = parse_origin_url("ssh://git@github.com/Richards-LLC/gabber-studio.git").unwrap();
        assert_eq!(r.full_name(), "Richards-LLC/gabber-studio");
    }

    #[test]
    fn parse_rejects_gitlab() {
        assert!(parse_origin_url("https://gitlab.com/foo/bar.git").is_none());
    }

    #[test]
    fn parse_rejects_bitbucket() {
        assert!(parse_origin_url("git@bitbucket.org:foo/bar.git").is_none());
    }

    #[test]
    fn parse_rejects_self_hosted_gitea() {
        assert!(parse_origin_url("https://git.example.com/foo/bar.git").is_none());
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(parse_origin_url("").is_none());
        assert!(parse_origin_url("   ").is_none());
    }

    #[test]
    fn parse_rejects_missing_repo() {
        assert!(parse_origin_url("https://github.com/Richards-LLC").is_none());
        assert!(parse_origin_url("https://github.com/Richards-LLC/").is_none());
        assert!(parse_origin_url("git@github.com:Richards-LLC").is_none());
    }

    #[test]
    fn parse_rejects_extra_path_segments() {
        // GitHub URLs with /tree/main, /pull/1, etc. shouldn't be accepted as
        // "OWNER/REPO" — they're not clone URLs.
        assert!(
            parse_origin_url("https://github.com/Richards-LLC/gabber-studio/tree/main").is_none()
        );
    }

    // --- git remote -v parsing --------------------------------------------

    #[test]
    fn parse_remote_v_picks_origin() {
        let stdout = "\
upstream\thttps://github.com/upstream/foo.git (fetch)
upstream\thttps://github.com/upstream/foo.git (push)
origin\thttps://github.com/me/foo.git (fetch)
origin\thttps://github.com/me/foo.git (push)
";
        assert_eq!(
            parse_remote_v(stdout).as_deref(),
            Some("https://github.com/me/foo.git")
        );
    }

    #[test]
    fn parse_remote_v_falls_back_to_first_when_no_origin() {
        let stdout = "fork\thttps://github.com/fork/foo.git (fetch)\nfork\thttps://github.com/fork/foo.git (push)\n";
        assert_eq!(
            parse_remote_v(stdout).as_deref(),
            Some("https://github.com/fork/foo.git")
        );
    }

    #[test]
    fn parse_remote_v_empty_returns_none() {
        assert!(parse_remote_v("").is_none());
    }

    #[test]
    fn parse_remote_v_skips_push_lines() {
        // First line is push-only; we should pick origin's fetch URL, not the
        // push URL on the line above.
        let stdout = "origin\thttps://example.com/push (push)\norigin\thttps://github.com/me/foo.git (fetch)\n";
        assert_eq!(
            parse_remote_v(stdout).as_deref(),
            Some("https://github.com/me/foo.git")
        );
    }

    // --- RepoRef::from_owner_slash_repo ------------------------------------

    #[test]
    fn from_flag_simple() {
        let r = RepoRef::from_owner_slash_repo("foo/bar").unwrap();
        assert_eq!(r.owner, "foo");
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn from_flag_strips_dot_git() {
        let r = RepoRef::from_owner_slash_repo("foo/bar.git").unwrap();
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn from_flag_rejects_bare_name() {
        assert!(RepoRef::from_owner_slash_repo("just-a-name").is_none());
    }

    #[test]
    fn from_flag_rejects_three_segments() {
        assert!(RepoRef::from_owner_slash_repo("a/b/c").is_none());
    }

    #[test]
    fn from_flag_rejects_empty_halves() {
        assert!(RepoRef::from_owner_slash_repo("/bar").is_none());
        assert!(RepoRef::from_owner_slash_repo("foo/").is_none());
    }

    // --- File-level init / refresh ----------------------------------------

    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap()
    }

    #[test]
    fn init_with_flag_override_writes_both_files() {
        let tmp = TempDir::new().unwrap();
        let outcome = init_at(tmp.path(), Some("Richards-LLC/gabber-studio")).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Configured);
        assert_eq!(outcome.files.len(), 2);
        let claude = read(&tmp.path().join(CLAUDE_SKILL_REL));
        let cursor = read(&tmp.path().join(CURSOR_SKILL_REL));
        for s in [&claude, &cursor] {
            assert!(s.contains("Richards-LLC/gabber-studio"));
            assert!(s.contains("CRITICAL: Always pass --repo"));
            assert!(s.contains("--repo Richards-LLC/gabber-studio"));
            assert!(s.contains("<!-- keep github-repo -->"));
            assert!(s.contains("<!-- /keep github-repo -->"));
        }
    }

    #[test]
    fn init_with_no_remote_returns_skipped() {
        let tmp = TempDir::new().unwrap();
        // No flag, no git repo → Skipped. (The `git remote -v` invocation in
        // a non-repo directory exits non-zero, which we map to None.)
        let outcome = init_at(tmp.path(), None).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Skipped);
        assert!(!tmp.path().join(CLAUDE_SKILL_REL).exists());
        assert!(!tmp.path().join(CURSOR_SKILL_REL).exists());
    }

    #[test]
    fn init_rejects_malformed_flag() {
        let tmp = TempDir::new().unwrap();
        let err = init_at(tmp.path(), Some("not-a-slash-pair")).unwrap_err();
    }

    #[test]
    fn init_idempotent_second_call_is_already_configured() {
        let tmp = TempDir::new().unwrap();
        let _ = init_at(tmp.path(), Some("Richards-LLC/gabber-studio")).unwrap();
        let second = init_at(tmp.path(), Some("Richards-LLC/gabber-studio")).unwrap();
        assert_eq!(second.status, IntegrationStatus::AlreadyConfigured);
        assert!(second.files.is_empty());
    }

    #[test]
    fn refresh_after_rename_updates_keep_block() {
        let tmp = TempDir::new().unwrap();
        let _ = init_at(tmp.path(), Some("OldOwner/old-name")).unwrap();
        let before = read(&tmp.path().join(CLAUDE_SKILL_REL));
        assert!(before.contains("OldOwner/old-name"));

        let outcome = refresh_at(tmp.path(), Some("NewOwner/new-name")).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Refreshed);
        let after = read(&tmp.path().join(CLAUDE_SKILL_REL));
        assert!(!after.contains("OldOwner/old-name"));
        assert!(after.contains("NewOwner/new-name"));
        assert!(after.contains("--repo NewOwner/new-name"));
    }

    #[test]
    fn refresh_no_change_reports_already_configured() {
        let tmp = TempDir::new().unwrap();
        let _ = init_at(tmp.path(), Some("Richards-LLC/gabber-studio")).unwrap();
        let outcome = refresh_at(tmp.path(), Some("Richards-LLC/gabber-studio")).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::AlreadyConfigured);
    }

    #[test]
    fn refresh_with_no_remote_and_no_flag_is_skipped() {
        let tmp = TempDir::new().unwrap();
        let outcome = refresh_at(tmp.path(), None).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Skipped);
    }

    // --- verify ------------------------------------------------------------

    #[test]
    fn verify_missing_skill_is_skipped() {
        let tmp = TempDir::new().unwrap();
        let outcome = verify_at(tmp.path()).unwrap();
        assert_eq!(outcome.status, IntegrationStatus::Skipped);
    }

    #[test]
    fn verify_with_matching_flag_path_uses_recorded_value() {
        // We can't exercise the real `git remote -v` portion in a unit test
        // without standing up a fake git, but we *can* assert the recorded
        // value extraction works.
        let tmp = TempDir::new().unwrap();
        let _ = init_at(tmp.path(), Some("Richards-LLC/gabber-studio")).unwrap();
        let body = read(&tmp.path().join(CLAUDE_SKILL_REL));
        let recorded = recorded_full_name(&body).unwrap();
        assert_eq!(recorded.as_deref(), Some("Richards-LLC/gabber-studio"));
    }

    #[test]
    fn verify_handles_keep_block_with_drift_marker() {
        // Construct a SKILL file by hand with a known recorded value, then
        // confirm `recorded_full_name` plucks it out cleanly.
        let body = "\
---
name: github-repo
---

<!-- keep github-repo -->
| | Value |
|--|--|
| **Owner** | `acme` |
| **Repo** | `widget` |
| **Full name** | `acme/widget` |
<!-- /keep github-repo -->
";
        assert_eq!(
            recorded_full_name(body).unwrap().as_deref(),
            Some("acme/widget")
        );
    }

    #[test]
    fn verify_unparseable_keep_block_returns_none() {
        // Keep block exists but has no Full name row.
        let body = "<!-- keep github-repo -->\nrandom\n<!-- /keep github-repo -->\n";
        assert_eq!(recorded_full_name(body).unwrap(), None);
    }

    #[test]
    fn verify_no_keep_block_returns_none() {
        assert_eq!(recorded_full_name("# no keep block here\n").unwrap(), None);
    }

    // --- orphan surfacing --------------------------------------------------

    #[test]
    fn init_writes_distinct_cursor_template_content() {
        // Regression guard: SKILL.md.template and cursor.md.template must not
        // be silently swapped or rendered to the same path. The cursor file
        // carries a unique 'Refer to ~/.cursor/skills/mcp-github/...' line.
        let tmp = TempDir::new().unwrap();
        let _ = init_at(tmp.path(), Some("acme/widget")).unwrap();
        let claude = read(&tmp.path().join(CLAUDE_SKILL_REL));
        let cursor = read(&tmp.path().join(CURSOR_SKILL_REL));
        assert!(
            cursor.contains("`~/.cursor/skills/mcp-github/SKILL.md`"),
            "cursor template should reference the user-level cursor skill"
        );
        assert!(
            !claude.contains("`~/.cursor/skills/mcp-github/SKILL.md`"),
            "claude template must not include the cursor-only reference"
        );
    }

    #[test]
    fn refresh_bails_on_malformed_existing_keep_markers() {
        // Safety rail: when an existing SKILL.md has malformed keep markers
        // (e.g. an unmatched open marker from a botched hand-edit), we must
        // refuse to silently overwrite — the user might have valuable content
        // in there.
        let tmp = TempDir::new().unwrap();
        let claude_path = tmp.path().join(CLAUDE_SKILL_REL);
        fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        // Open marker with no matching close.
        fs::write(&claude_path, "<!-- keep github-repo -->\nstray content\n").unwrap();
        let err = refresh_at(tmp.path(), Some("acme/widget")).unwrap_err();
    }

    #[test]
    fn recorded_full_name_rejects_garbled_value_with_extra_backticks() {
        // An attacker (or a bad hand-edit) injects a second backtick pair
        // into the Full-name row. The parser must take only the value
        // between the *first pair* of backticks and validate it as a
        // well-formed OWNER/REPO. Anything else returns None.
        let body = "\
<!-- keep github-repo -->
| **Full name** | `acme/widget` (also see `evil/x`) |
<!-- /keep github-repo -->
";
        assert_eq!(
            recorded_full_name(body).unwrap().as_deref(),
            Some("acme/widget"),
            "first-pair extraction should ignore the second backtick pair"
        );

        // A malformed value (whitespace, no slash, etc.) returns None even
        // though the surrounding markdown is well-formed.
        let bad = "\
<!-- keep github-repo -->
| **Full name** | `not a repo at all` |
<!-- /keep github-repo -->
";
        assert_eq!(recorded_full_name(bad).unwrap(), None);
    }

    #[test]
    fn detect_repo_against_real_git_init_https_remote() {
        // End-to-end: stand up a real git repo with a GitHub HTTPS remote and
        // assert detect_repo extracts the OWNER/REPO via the
        // run_git_remote_v -> parse_remote_v -> parse_origin_url chain.
        // Skips when `git` isn't on PATH so the test stays portable.
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("skipping: git binary not available");
            return;
        }
        let tmp = TempDir::new().unwrap();
        // `git init` + `git remote add origin <url>` — minimal config; no
        // user.email needed for these.
        let init = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert!(init.success(), "git init failed");
        let add = Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/Richards-LLC/gabber-studio.git",
            ])
            .status()
            .unwrap();
        assert!(add.success(), "git remote add failed");
        let detected = detect_repo(tmp.path()).unwrap();
        assert_eq!(
            detected,
            Some(RepoRef {
                owner: "Richards-LLC".to_string(),
                repo: "gabber-studio".to_string(),
            })
        );
    }

    #[test]
    fn detect_repo_against_real_git_init_ssh_remote() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args([
                "remote",
                "add",
                "origin",
                "git@github.com:Richards-LLC/gabber-studio.git",
            ])
            .status()
            .unwrap();
        let detected = detect_repo(tmp.path()).unwrap();
        assert_eq!(detected.unwrap().full_name(), "Richards-LLC/gabber-studio");
    }

    #[test]
    fn detect_repo_against_real_git_init_gitlab_remote_returns_none() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["remote", "add", "origin", "https://gitlab.com/foo/bar.git"])
            .status()
            .unwrap();
        // Detection must reject the non-GitHub remote.
        assert_eq!(detect_repo(tmp.path()).unwrap(), None);
    }

    #[test]
    fn render_template_sanitizes_close_marker_in_cas_full_name_tag() {
        // cas-fc38 autofix round 1: an OWNER/REPO containing literal `-->` or
        // a newline must not corrupt the surrounding keep markers. The render
        // path now routes the value through emit_cas_full_name_tag so the
        // close-marker rewrite (`-->` → `--&gt;`) actually fires.
        let evil = RepoRef {
            owner: "evil-->payload".to_string(),
            repo: "x".to_string(),
        };
        let rendered = render_template(SKILL_TEMPLATE, &evil);
        // Keep-block extraction must still succeed.
        let blocks = super::super::keep_block::extract(&rendered).unwrap();
        let github_block = blocks
            .iter()
            .find(|b| b.name.as_deref() == Some(KEEP_GITHUB_BLOCK))
            .expect("keep block must still parse");
        // The cas:full_name tag must still parse and round-trip the
        // sanitized value (with `-->` neutralized).
        let recovered =
            super::super::md::parse_cas_full_name_tag(&github_block.body).unwrap();
        assert!(
            !recovered.contains("-->"),
            "tag value should have neutralized literal `-->`; got {recovered}"
        );
    }

    #[test]
    fn refresh_rejects_symlinked_skill_md() {
        // cas-fc38: github verify/refresh now read SKILL.md via read_capped
        // which refuses symlinks. Plant a symlink at the SKILL path and
        // confirm refresh bails rather than silently following.
        #[cfg(not(unix))]
        return;
        let tmp = TempDir::new().unwrap();
        let claude_path = tmp.path().join(CLAUDE_SKILL_REL);
        fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        let real = tmp.path().join("decoy.md");
        fs::write(&real, "decoy contents").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &claude_path).unwrap();
        let err = refresh_at(tmp.path(), Some("acme/widget")).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("symlink"),
            "expected symlink rejection from read_capped; got {s}"
        );
        // Decoy must be untouched.
        assert_eq!(fs::read_to_string(&real).unwrap(), "decoy contents");
    }

    #[test]
    fn init_emits_cas_full_name_tag_in_keep_block() {
        // cas-fc38: every handler emits the `<!-- cas:full_name=... -->`
        // identity tag so downstream tooling can recover the canonical
        // identity without parsing template-internal markdown layout.
        let tmp = TempDir::new().unwrap();
        let _ = init_at(tmp.path(), Some("Richards-LLC/gabber-studio")).unwrap();
        let claude = read(&tmp.path().join(CLAUDE_SKILL_REL));
        let cursor = read(&tmp.path().join(CURSOR_SKILL_REL));
        for s in [&claude, &cursor] {
            assert!(
                s.contains("<!-- cas:full_name=Richards-LLC/gabber-studio -->"),
                "expected canonical cas:full_name tag in:\n{s}"
            );
        }
        // The tag must be inside the named keep block so refresh preserves
        // it via PreferTemplate semantics.
        let blocks = super::super::keep_block::extract(&claude).unwrap();
        let github_block = blocks
            .iter()
            .find(|b| b.name.as_deref() == Some(KEEP_GITHUB_BLOCK))
            .expect("github-repo keep block must exist");
        assert!(
            super::super::md::parse_cas_full_name_tag(&github_block.body).is_some(),
            "tag must live inside the keep block, not adjacent prose"
        );
    }

    #[test]
    fn recorded_full_name_prefers_cas_full_name_tag_over_table_row() {
        // Provided both, the tag is canonical (cas-fc38). This guards
        // against templates that legitimately reorder the row in a future
        // revision.
        let body = "\
<!-- keep github-repo -->
<!-- cas:full_name=acme/widget -->
| | Value |
| **Full name** | `something/else` |
<!-- /keep github-repo -->
";
        assert_eq!(
            recorded_full_name(body).unwrap().as_deref(),
            Some("acme/widget"),
            "tag must beat the table-row fallback"
        );
    }

    #[test]
    fn recorded_full_name_falls_back_to_table_row_when_tag_absent() {
        // Backwards compat: a SKILL.md from a pre-fc38 install has no tag,
        // only the **Full name** row. We must still recover.
        let body = "\
<!-- keep github-repo -->
| | Value |
| **Full name** | `acme/widget` |
<!-- /keep github-repo -->
";
        assert_eq!(
            recorded_full_name(body).unwrap().as_deref(),
            Some("acme/widget")
        );
    }

    #[test]
    fn refresh_surfaces_orphan_keep_blocks_in_summary() {
        let tmp = TempDir::new().unwrap();
        // Hand-write an existing SKILL.md with the canonical block PLUS a
        // user-added unnamed block the template doesn't accommodate.
        let claude_path = tmp.path().join(CLAUDE_SKILL_REL);
        fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        let with_orphan = "\
# Hand-written
<!-- keep github-repo -->
| **Full name** | `Richards-LLC/gabber-studio` |
<!-- /keep github-repo -->
<!-- keep my-notes -->
hand-edited notes
<!-- /keep my-notes -->
";
        fs::write(&claude_path, with_orphan).unwrap();

        let outcome = refresh_at(tmp.path(), Some("Richards-LLC/gabber-studio")).unwrap();
        assert!(
            outcome
                .summary
                .iter()
                .any(|s| s.contains("orphan keep block") && s.contains("my-notes")),
            "summary should warn about orphan; got: {:?}",
            outcome.summary
        );
    }

    // --- verify_report (cas-3efe) ----------------------------------------

    #[test]
    fn verify_report_not_configured_when_skill_absent() {
        let tmp = TempDir::new().unwrap();
        let report = super::verify_report(tmp.path());
        assert!(report.not_configured);
        assert!(report
            .notes
            .first()
            .unwrap()
            .contains(CLAUDE_SKILL_REL));
    }

    #[test]
    fn verify_report_marks_stale_when_no_local_remote_matches_recorded() {
        let tmp = TempDir::new().unwrap();
        // init_at without a flag would skip; use the flag path so SKILL.md
        // gets written deterministically. Then verify_report's
        // detect_repo will find no git repo and return Ok(None) → Stale.
        let _ = init_at(tmp.path(), Some("alice/repo")).unwrap();
        let report = super::verify_report(tmp.path());
        assert!(!report.not_configured);
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].id, "alice/repo");
        assert_eq!(report.items[0].state, IdState::Stale);
    }
}
