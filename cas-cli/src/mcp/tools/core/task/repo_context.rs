use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cas_store::{KnownRepoBinding, KnownRepoStore};
use cas_types::WorkTarget;

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};

/// Host-local repository evidence resolved fresh for one lifecycle mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoContext {
    pub repo_selector: String,
    pub repo_root: PathBuf,
    pub git_common_dir: PathBuf,
    pub target_branch: String,
}

/// Preflight-only repository probe. Lifecycle mutations retain their existing
/// resolver; preflight supplies one absolute deadline across every Git call.
#[derive(Debug, Clone)]
pub(crate) struct BoundedRepoProbe {
    deadline: Deadline,
    per_command_cap: Duration,
    candidate_limit: usize,
    git_program: PathBuf,
}

impl BoundedRepoProbe {
    pub(crate) fn new(
        deadline: Deadline,
        per_command_cap: Duration,
        candidate_limit: usize,
    ) -> Self {
        Self {
            deadline,
            per_command_cap,
            candidate_limit,
            git_program: PathBuf::from("git"),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_git_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.git_program = program.into();
        self
    }

    pub(crate) fn output(&self, path: &Path, args: &[&str]) -> Result<String, BoundedRepoError> {
        let output = run_command(
            Command::new(&self.git_program)
                .arg("-C")
                .arg(path)
                .args(args),
            self.deadline,
            self.per_command_cap,
        )
        .map_err(|error| match error {
            BoundedCommandError::TimedOut => BoundedRepoError::TimedOut,
            BoundedCommandError::Io => BoundedRepoError::Unavailable,
        })?;
        if !output.status.success() {
            return Err(BoundedRepoError::Unavailable);
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            Err(BoundedRepoError::Unavailable)
        } else {
            Ok(value)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundedRepoError {
    TimedOut,
    CandidateLimit,
    Unavailable,
    Ambiguous,
    StaleBinding,
}

fn git_output(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        Err("git returned an empty value".to_string())
    } else {
        Ok(value)
    }
}

fn canonical(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn git_layout(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    let checkout_root = PathBuf::from(git_output(path, &["rev-parse", "--show-toplevel"])?);
    let common_raw = PathBuf::from(git_output(path, &["rev-parse", "--git-common-dir"])?);
    let common_dir = canonical(if common_raw.is_absolute() {
        common_raw
    } else {
        // `rev-parse --git-common-dir` is relative to Git's `-C <path>`,
        // not necessarily to the checkout root. Preflight intentionally
        // resolves from `<project>/.cas`; joining `../.git` to the checkout
        // root would escape to the parent repository and fabricate a mismatch.
        path.join(common_raw)
    });
    // Linked worktrees share `<main>/.git`; use its parent as the durable
    // host-local root. Ordinary repositories take the same path.
    let repo_root = common_dir
        .file_name()
        .filter(|name| *name == ".git")
        .and_then(|_| common_dir.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| canonical(checkout_root));
    Ok((canonical(repo_root), common_dir))
}

/// Resolve the actual checkout root and shared Git common directory for a
/// caller that supplied a local project root.
///
/// [`git_layout`] deliberately collapses linked worktrees to the primary
/// checkout so host-repository discovery and binding identity remain stable.
/// That is wrong for local config lookup: an ignored `.cas/` directory may
/// exist only in the linked checkout. Keep the checkout root here so its
/// project selector is checked before falling back to the host registry.
fn git_checkout_layout(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    let checkout_root = canonical(PathBuf::from(git_output(
        path,
        &["rev-parse", "--show-toplevel"],
    )?));
    let common_raw = PathBuf::from(git_output(path, &["rev-parse", "--git-common-dir"])?);
    let common_dir = canonical(if common_raw.is_absolute() {
        common_raw
    } else {
        path.join(common_raw)
    });
    Ok((checkout_root, common_dir))
}

fn bounded_git_layout(
    path: &Path,
    probe: &BoundedRepoProbe,
) -> Result<(PathBuf, PathBuf), BoundedRepoError> {
    let checkout_root = PathBuf::from(probe.output(path, &["rev-parse", "--show-toplevel"])?);
    let common_raw = PathBuf::from(probe.output(path, &["rev-parse", "--git-common-dir"])?);
    let common_dir = canonical(if common_raw.is_absolute() {
        common_raw
    } else {
        path.join(common_raw)
    });
    let repo_root = common_dir
        .file_name()
        .filter(|name| *name == ".git")
        .and_then(|_| common_dir.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| canonical(checkout_root));
    Ok((canonical(repo_root), common_dir))
}

/// Canonicalize the payload of a `remote:` selector to `<host>/<owner>/<repo>`.
///
/// cas-1a1c (GH #151). [`crate::cloud::normalize_git_remote_url`] already
/// parses every URL shape a checkout's `origin` can take (https, http,
/// `ssh://git@`, scp-like `git@host:owner/repo`, ±`.git`), but it deliberately
/// returns `None` for an already-bare `host/owner/repo` so that canonical-id
/// derivation never mistakes a local filesystem path for a remote. Selectors
/// persisted on a task ARE in that bare form, so the matcher needs the extra
/// step — added here rather than by loosening the cloud helper, whose `None`
/// is load-bearing for a different caller.
///
/// The host component is lowercased (DNS is case-insensitive); the owner and
/// repo segments are left exactly as written, because path case is significant
/// on some forges.
fn canonical_remote_payload(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = crate::cloud::normalize_git_remote_url(trimmed)
        .or_else(|| bare_host_owner_repo(trimmed))?;
    let (host, path) = normalized.split_once('/')?;
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{path}", host.to_ascii_lowercase()))
}

/// Accept an already-normalized `host/owner/repo[/...]` selector payload.
///
/// Deliberately strict so a filesystem path can never be read as a remote: the
/// value must carry no scheme and no userinfo, must not be absolute, must have
/// at least two `/` separators, and its first segment must look like a host
/// (contains a `.`, or is exactly `localhost`).
fn bare_host_owner_repo(raw: &str) -> Option<String> {
    if raw.contains("://") || raw.contains('@') || raw.starts_with('/') {
        return None;
    }
    let clean = raw.trim_end_matches('/');
    let clean = clean.strip_suffix(".git").unwrap_or(clean);
    if clean.matches('/').count() < 2 {
        return None;
    }
    let host = clean.split('/').next()?;
    if host.is_empty() || (!host.contains('.') && host != "localhost") {
        return None;
    }
    Some(clean.to_string())
}

/// Canonical comparison form for a work-target selector.
///
/// `project:` selectors compare verbatim (the id is opaque). `remote:`
/// selectors compare on their canonicalized payload so that every URL shape of
/// the same repository collapses to one value. An unparseable `remote:` payload
/// falls back to its trimmed literal rather than vanishing, so a malformed
/// selector still compares equal to itself.
pub(crate) fn canonical_selector(selector: &str) -> String {
    let trimmed = selector.trim();
    match trimmed.split_once(':') {
        Some(("remote", payload)) => canonical_remote_payload(payload)
            .map(|payload| format!("remote:{payload}"))
            .unwrap_or_else(|| trimmed.to_string()),
        _ => trimmed.to_string(),
    }
}

/// Every selector a checkout legitimately answers to, most authoritative first.
///
/// cas-1a1c (GH #151). [`selector_for_repo`] returns a single selector on a
/// priority order — a `[project] canonical_id` pin wins and the git remote is
/// never consulted. That is correct for *stamping* a new work target, but wrong
/// for *matching* an existing one: `declare_work_target` records whichever
/// selector was authoritative at creation time, so pinning a canonical_id
/// afterwards left every previously-created task holding the `remote:` form
/// unable to match its own repository. A checkout has a set of identities, not
/// one; matching tests membership in that set.
fn repo_identities(repo_root: &Path) -> Vec<String> {
    let mut identities = Vec::new();
    let cas_root = repo_root.join(".cas");
    if let Some(project_id) = crate::cloud::canonical_id_from_config_toml(&cas_root)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
    {
        identities.push(format!("project:{project_id}"));
    }
    if let Some(remote) = crate::cloud::derive_canonical_id_from_git_remote(repo_root) {
        identities.push(format!("remote:{remote}"));
    }
    identities
}

/// Whether a checkout answers to the declared target selector.
pub(crate) fn repo_answers_to(repo_root: &Path, target_selector: &str) -> bool {
    let want = canonical_selector(target_selector);
    repo_identities(repo_root)
        .iter()
        .any(|identity| canonical_selector(identity) == want)
}

fn selector_for_repo(repo_root: &Path) -> Result<String, String> {
    let cas_root = repo_root.join(".cas");
    if let Some(project_id) = crate::cloud::canonical_id_from_config_toml(&cas_root)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
    {
        return Ok(format!("project:{project_id}"));
    }
    crate::cloud::derive_canonical_id_from_git_remote(repo_root)
        .map(|remote| format!("remote:{remote}"))
        .ok_or_else(|| {
            format!(
                "repository {} has neither [project].canonical_id nor a normalizable origin URL",
                repo_root.display()
            )
        })
}

/// Resolve a path supplied to the host-local binding CLI. Bindings require a
/// real canonical repository root, not a symlink, nested path, or linked
/// worktree checkout.
pub(crate) fn binding_identity_for_path(path: &Path) -> Result<(String, PathBuf, PathBuf), String> {
    let absolute_input = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve current directory: {error}"))?
            .join(path)
    };
    let mut prefix = PathBuf::new();
    for component in absolute_input.components() {
        prefix.push(component.as_os_str());
        if matches!(component, std::path::Component::Normal(_))
            && std::fs::symlink_metadata(&prefix)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
        {
            return Err("repository path and its parents must not be symlinks".to_string());
        }
    }
    let metadata = std::fs::symlink_metadata(&absolute_input)
        .map_err(|error| format!("cannot inspect repository path: {error}"))?;
    if !metadata.is_dir() {
        return Err("repository path must be a directory".to_string());
    }
    let canonical_input = absolute_input
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize repository path: {error}"))?;
    let (repo_root, git_common_dir) = git_layout(&canonical_input)?;
    if repo_root != canonical_input {
        return Err(
            "repository path must be the canonical root whose Git common directory is being bound"
                .to_string(),
        );
    }
    if !git_common_dir.is_dir() {
        return Err("resolved Git common directory is unavailable".to_string());
    }
    let selector = selector_for_repo(&repo_root)?;
    Ok((selector, repo_root, git_common_dir))
}

fn validate_binding(
    binding: &KnownRepoBinding,
    expected_selector: &str,
) -> Result<(PathBuf, PathBuf), ()> {
    if binding.selector != expected_selector
        || !binding.repo_root.is_absolute()
        || !binding.git_common_dir.is_absolute()
        || !binding.repo_root.exists()
        || !binding.git_common_dir.exists()
    {
        return Err(());
    }
    let (repo_root, git_common_dir) = git_layout(&binding.repo_root).map_err(|_| ())?;
    if repo_root != binding.repo_root
        || git_common_dir != binding.git_common_dir
        || selector_for_repo(&repo_root).map_err(|_| ())? != expected_selector
    {
        return Err(());
    }
    Ok((repo_root, git_common_dir))
}

pub(crate) fn binding_is_live(binding: &KnownRepoBinding) -> bool {
    validate_binding(binding, &binding.selector).is_ok()
}

fn host_binding(selector: &str) -> Result<Option<KnownRepoBinding>, String> {
    let store = crate::store::known_repos::open_host_known_repo_store().map_err(|_| {
        "WORK TARGET BINDING UNAVAILABLE: host-local repository binding state could not be read"
            .to_string()
    })?;
    match store.get_binding(selector) {
        Ok(binding) => Ok(binding),
        // Hosts predating m214 retain legacy unbound resolution until the
        // maintenance CLI or migration runner installs the table.
        Err(error) if error.to_string().contains("no such table") => Ok(None),
        Err(_) => Err(
            "WORK TARGET BINDING UNAVAILABLE: host-local repository binding state could not be read"
                .to_string(),
        ),
    }
}

fn bounded_selector_for_repo(
    repo_root: &Path,
    probe: &BoundedRepoProbe,
) -> Result<String, BoundedRepoError> {
    let cas_root = repo_root.join(".cas");
    if let Some(project_id) = crate::cloud::canonical_id_from_config_toml(&cas_root)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
    {
        return Ok(format!("project:{project_id}"));
    }
    let remote = probe.output(repo_root, &["remote", "get-url", "origin"])?;
    crate::cloud::normalize_git_remote_url(&remote)
        .map(|remote| format!("remote:{remote}"))
        .ok_or(BoundedRepoError::Unavailable)
}

/// Bounded twin of [`repo_identities`] — same set semantics under the preflight
/// deadline. A missing or unparseable `origin` contributes no remote identity
/// rather than failing the probe: the pin alone is still a valid identity.
fn bounded_repo_identities(
    repo_root: &Path,
    probe: &BoundedRepoProbe,
) -> Result<Vec<String>, BoundedRepoError> {
    let mut identities = Vec::new();
    let cas_root = repo_root.join(".cas");
    if let Some(project_id) = crate::cloud::canonical_id_from_config_toml(&cas_root)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
    {
        identities.push(format!("project:{project_id}"));
    }
    match probe.output(repo_root, &["remote", "get-url", "origin"]) {
        Ok(remote) => {
            if let Some(remote) = crate::cloud::normalize_git_remote_url(&remote) {
                identities.push(format!("remote:{remote}"));
            }
        }
        Err(BoundedRepoError::Unavailable) => {}
        Err(error) => return Err(error),
    }
    Ok(identities)
}

pub(crate) fn resolve_default_branch(repo_root: &Path) -> Result<String, String> {
    if let Ok(reference) = git_output(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        && let Some(branch) = reference.strip_prefix("refs/remotes/origin/")
        && !branch.is_empty()
        && let Ok(branch) = validate_target_branch(repo_root, branch)
    {
        return Ok(branch);
    }
    for candidate in ["main", "master"] {
        if Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{candidate}"),
            ])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return Ok(candidate.to_string());
        }
    }
    Err(format!(
        "cannot resolve a default branch for {} (origin/HEAD, main, and master are absent)",
        repo_root.display()
    ))
}

/// Validate and normalize a branch name using Git's own branch grammar.
pub(crate) fn validate_target_branch(repo_root: &Path, branch: &str) -> Result<String, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("WORK TARGET REJECTED: target branch is empty".to_string());
    }
    git_output(repo_root, &["check-ref-format", "--branch", branch]).map_err(|reason| {
        format!("WORK TARGET REJECTED: invalid target branch `{branch}`: {reason}")
    })
}

/// Create the portable durable target. An omitted target remains `None` for
/// legacy/non-git task stores; an explicitly supplied path always fails closed.
pub(crate) fn declare_work_target(
    cas_root: &Path,
    target_repo: Option<&str>,
    target_branch: Option<&str>,
) -> Result<Option<WorkTarget>, String> {
    if target_repo.is_none() && target_branch.is_none() {
        return Ok(None);
    }
    let input = target_repo
        .map(PathBuf::from)
        .unwrap_or_else(|| cas_root.to_path_buf());
    let (repo_root, _) = git_layout(&input).map_err(|reason| {
        format!(
            "WORK TARGET REJECTED: cannot resolve target repository {}: {reason}",
            input.display()
        )
    })?;
    let repo_selector = selector_for_repo(&repo_root)
        .map_err(|reason| format!("WORK TARGET REJECTED: {reason}"))?;
    let target_branch = match target_branch {
        Some(branch) => validate_target_branch(&repo_root, branch)?,
        None => resolve_default_branch(&repo_root)
            .map_err(|reason| format!("WORK TARGET REJECTED: {reason}"))?,
    };
    crate::store::known_repos::register_repo_strict(&repo_root).map_err(|error| {
        crate::store::known_repos::host_registry_write_error(&repo_root, &error)
    })?;
    Ok(Some(WorkTarget {
        repo_selector,
        target_branch,
    }))
}

/// Select the durable WorkTarget a child should inherit from an epic.
///
/// The live epic branch is the delivery lane whenever it is recorded.  Its
/// repository binding still comes from the epic's WorkTarget; when the live
/// lane is absent, that WorkTarget is itself the delivery target.  This is the
/// same live-epic-before-epic-target precedence used by worker spawn and
/// worktree merge, expressed as a portable child WorkTarget for close gates.
pub(crate) fn inherited_work_target_from_epic(epic: &cas_types::Task) -> Option<WorkTarget> {
    let epic_target = epic.deliverables.work_target.as_ref()?;
    let target_branch = epic
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .unwrap_or(&epic_target.target_branch)
        .to_string();
    Some(WorkTarget {
        repo_selector: epic_target.repo_selector.clone(),
        target_branch,
    })
}

/// Return the inherited target only when the child has no target, or is still
/// on the epic's own base target.  A distinct child target is an operator
/// choice and must remain authoritative.
pub(crate) fn default_child_work_target_from_epic(
    child: &cas_types::Task,
    epic: &cas_types::Task,
) -> Option<WorkTarget> {
    let inherited = inherited_work_target_from_epic(epic)?;
    let child_target = child.deliverables.work_target.as_ref();
    let epic_target = epic.deliverables.work_target.as_ref()?;
    match child_target {
        None => Some(inherited),
        Some(target)
            if target.repo_selector == epic_target.repo_selector
                && target.target_branch == epic_target.target_branch =>
        {
            Some(inherited)
        }
        Some(_) => None,
    }
}

/// Local evidence about whether a task is anchored in the current project.
///
/// cas-156b (GH #135). `declare_work_target` returns `Ok(None)` whenever a task
/// carries neither `target_repo` nor `target_branch`, so `task start` had
/// nothing to check and silently leased tasks foreign to the current project.
/// One contaminated database ended up with live `task_lease_history` and
/// `verifications` rows for tasks belonging to two other repositories,
/// including a supervisor-bypass close recorded against a replica while the
/// authoritative row never moved.
///
/// The naive rule — "no `target_repo` means foreign" — is unusable: factory
/// tasks routinely carry no work target at all, so it would fire on nearly
/// every legitimate start and train people to ignore the warning. The usable
/// discriminator is structural rather than name-based, and it comes from the
/// sync surface itself: [`crate::cloud::syncer`]'s pull path upserts only
/// entries, tasks, rules, skills and specs — it never writes rows to
/// `dependencies`, and per-project pull never writes `agents`. A
/// cloud-replicated foreign task therefore arrives as a *dependency orphan*
/// whose assignee names nobody registered on this host, while a native factory
/// task is created through `create_atomic(&task, &blocked_by_ids, epic_id)`
/// and carries a `ParentChild` edge to its epic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TaskAnchorEvidence {
    /// The task declares an explicit work target. Those tasks are already
    /// resolved (and failed closed) by [`resolve_repo_context`].
    pub has_work_target: bool,
    /// Count of dependency edges of any type touching this task in the local
    /// database — epic parent, blockers, children, related.
    pub dependency_edge_count: usize,
    /// The task's assignee matches an agent registered on this host.
    pub assignee_is_local_agent: bool,
    /// Cloud sync is configured for this project. A database that never syncs
    /// cannot have received a replica, so it can never produce this warning.
    pub cloud_sync_configured: bool,
}

/// Whether a task about to be started has no anchor in the current project.
///
/// Fail-silent by design: each field is a *veto*, so the burden of proof is on
/// warning, not on staying quiet. Only a task that clears every veto at once is
/// reported. See [`TaskAnchorEvidence`] for why these four signals discriminate.
pub(crate) fn task_has_no_local_anchor(evidence: &TaskAnchorEvidence) -> bool {
    !evidence.has_work_target
        && evidence.dependency_edge_count == 0
        && !evidence.assignee_is_local_agent
        && evidence.cloud_sync_configured
}

/// Advisory text for a start whose task has no anchor in the current project.
///
/// Deliberately non-blocking: the lease still proceeds. A false positive must
/// cost a line of output, never a blocked start.
pub(crate) fn unanchored_task_start_warning(
    task_id: &str,
    repo_root: &Path,
    canonical_id: Option<&str>,
) -> String {
    let project = match canonical_id {
        Some(id) => format!("{} (cloud project `{id}`)", repo_root.display()),
        None => repo_root.display().to_string(),
    };
    format!(
        "\n\n⚠️  NO ANCHOR IN THIS PROJECT — task {task_id} has no evidence of belonging here.\n\
         It declares no target repository, has no dependency edge (no epic parent, no \
         blocker, no child) in this database, and its assignee is not an agent registered \
         on this host. Current project: {project}.\n\
         That is the fingerprint of a task replicated from another project: cloud pull \
         writes tasks but never dependency edges, so a foreign row arrives orphaned. The \
         lease was still taken — this is advisory, not a block — but if this task belongs \
         to a different repository, stop now: working it here records lease, verification \
         and close rows against a replica while the authoritative task never updates, \
         corrupting both projects' histories.\n\
         If it does belong here: `mcp__cas__task action=update id={task_id} \
         target_repo=<path> target_branch=<branch>` to anchor it and silence this.\n\
         If it is contamination: `cas cloud purge-foreign` (preview first) to drop \
         foreign rows and re-pull."
    )
}

fn candidate_paths(cas_root: &Path) -> Vec<PathBuf> {
    let mut raw = Vec::new();
    if let Ok(store) = crate::store::known_repos::open_host_known_repo_store()
        && let Ok(known) = store.list()
    {
        raw.extend(known.into_iter().map(|repo| repo.path));
    }
    raw.push(cas_root.to_path_buf());
    let mut seen = HashSet::new();
    raw.into_iter()
        // The host registry intentionally retains historical checkouts. Avoid
        // one failed `git` process per deleted TempDir/stale path during
        // bounded preflight and lifecycle resolution.
        .filter(|path| path.exists())
        .filter_map(|path| git_layout(&path).ok().map(|(root, _)| root))
        .filter(|root| seen.insert(root.clone()))
        .collect()
}

fn bounded_registry_paths(candidate_limit: usize) -> Result<Vec<PathBuf>, BoundedRepoError> {
    let db_path = crate::store::known_repos::host_cas_dir().join("cas.db");
    if !db_path.is_file() {
        return Ok(Vec::new());
    }
    let connection = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| BoundedRepoError::Unavailable)?;
    connection
        .busy_timeout(Duration::from_millis(50))
        .map_err(|_| BoundedRepoError::Unavailable)?;
    let mut statement = match connection
        .prepare("SELECT path FROM known_repos ORDER BY last_touched_at DESC LIMIT ?1")
    {
        Ok(statement) => statement,
        // An uninitialized host registry is equivalent to no known repos.
        Err(_) => return Ok(Vec::new()),
    };
    let query_limit = candidate_limit.saturating_add(1) as i64;
    let paths = statement
        .query_map([query_limit], |row| row.get::<_, String>(0))
        .map_err(|_| BoundedRepoError::Unavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| BoundedRepoError::Unavailable)?;
    if paths.len() > candidate_limit {
        return Err(BoundedRepoError::CandidateLimit);
    }
    Ok(paths.into_iter().map(PathBuf::from).collect())
}

fn bounded_host_binding(selector: &str) -> Result<Option<KnownRepoBinding>, BoundedRepoError> {
    let db_path = crate::store::known_repos::host_cas_dir().join("cas.db");
    if !db_path.is_file() {
        return Ok(None);
    }
    let connection = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| BoundedRepoError::Unavailable)?;
    connection
        .busy_timeout(Duration::from_millis(50))
        .map_err(|_| BoundedRepoError::Unavailable)?;
    let result = connection.query_row(
        "SELECT selector, repo_root, git_common_dir, created_at, updated_at
         FROM known_repo_bindings WHERE selector = ?1 COLLATE BINARY",
        [selector],
        |row| {
            let selector: String = row.get(0)?;
            let repo_root: String = row.get(1)?;
            let git_common_dir: String = row.get(2)?;
            let created_at: String = row.get(3)?;
            let updated_at: String = row.get(4)?;
            Ok(KnownRepoBinding {
                selector,
                repo_root: PathBuf::from(repo_root),
                git_common_dir: PathBuf::from(git_common_dir),
                created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                    .map(|value| value.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                updated_at: chrono::DateTime::parse_from_rfc3339(&updated_at)
                    .map(|value| value.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
            })
        },
    );
    match result {
        Ok(binding) => Ok(Some(binding)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        // Hosts predating m214 retain legacy unbound resolution until the
        // maintenance CLI or normal migration runner installs the table.
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("no such table") =>
        {
            Ok(None)
        }
        Err(_) => Err(BoundedRepoError::Unavailable),
    }
}

fn validate_binding_bounded(
    binding: &KnownRepoBinding,
    expected_selector: &str,
    probe: &BoundedRepoProbe,
) -> Result<(PathBuf, PathBuf), BoundedRepoError> {
    if binding.selector != expected_selector
        || !binding.repo_root.is_absolute()
        || !binding.git_common_dir.is_absolute()
        || !binding.repo_root.exists()
        || !binding.git_common_dir.exists()
    {
        return Err(BoundedRepoError::StaleBinding);
    }
    let (repo_root, git_common_dir) =
        bounded_git_layout(&binding.repo_root, probe).map_err(|error| match error {
            BoundedRepoError::TimedOut => BoundedRepoError::TimedOut,
            _ => BoundedRepoError::StaleBinding,
        })?;
    if repo_root != binding.repo_root
        || git_common_dir != binding.git_common_dir
        || bounded_selector_for_repo(&repo_root, probe).map_err(|error| match error {
            BoundedRepoError::TimedOut => BoundedRepoError::TimedOut,
            _ => BoundedRepoError::StaleBinding,
        })? != expected_selector
    {
        return Err(BoundedRepoError::StaleBinding);
    }
    Ok((repo_root, git_common_dir))
}

fn bounded_candidate_paths(
    cas_root: &Path,
    probe: &BoundedRepoProbe,
) -> Result<Vec<(PathBuf, PathBuf)>, BoundedRepoError> {
    let mut raw = vec![cas_root.to_path_buf()];
    raw.extend(bounded_registry_paths(probe.candidate_limit)?);
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for path in raw.into_iter().filter(|path| path.exists()) {
        match bounded_git_layout(&path, probe) {
            Ok((root, common)) if seen.insert(root.clone()) => {
                candidates.push((root, common));
            }
            Ok(_) | Err(BoundedRepoError::Unavailable) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(candidates)
}

/// Resolve and identity-check a declared work target for one mutation.
pub(crate) fn resolve_repo_context(
    cas_root: &Path,
    target: &WorkTarget,
) -> Result<RepoContext, String> {
    if let Some(binding) = host_binding(&target.repo_selector)? {
        let (repo_root, git_common_dir) = validate_binding(&binding, &target.repo_selector)
            .map_err(|()| {
                format!(
                    "⚠️ STALE WORK TARGET BINDING\n\n\
                     Task selector `{}` has a host-local binding whose live \
                     repository identity no longer matches. Refusing lifecycle \
                     mutation without fallback. Run `cas known-repos status`, \
                     then explicitly unbind and bind the intended repository.",
                    target.repo_selector
                )
            })?;
        let target_branch = validate_target_branch(&repo_root, &target.target_branch)?;
        return Ok(RepoContext {
            repo_selector: target.repo_selector.clone(),
            repo_root,
            git_common_dir,
            target_branch,
        });
    }
    let mut matches = Vec::new();
    let mut observed = Vec::new();
    for candidate in candidate_paths(cas_root) {
        let identities = repo_identities(&candidate);
        if identities.iter().any(|identity| {
            canonical_selector(identity) == canonical_selector(&target.repo_selector)
        }) && let Ok((repo_root, git_common_dir)) = git_layout(&candidate)
        {
            matches.push((repo_root, git_common_dir));
        }
        // cas-1a1c: report each checkout's identities as one GROUP rather than
        // flattening them. When the mismatch is the pin-vs-remote shape, seeing
        // `project:<pin> + remote:<origin>` together is what makes it obvious
        // that the pin was considered and the remote still did not match.
        // Deliberately path-free: these errors do not disclose host paths.
        if !identities.is_empty() {
            observed.push(
                identities
                    .iter()
                    .map(|identity| canonical_selector(identity))
                    .collect::<Vec<_>>()
                    .join(" + "),
            );
        }
    }
    observed.sort();
    observed.dedup();
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [(repo_root, common_dir)] => {
            let target_branch = validate_target_branch(repo_root, &target.target_branch)?;
            Ok(RepoContext {
                repo_selector: target.repo_selector.clone(),
                repo_root: repo_root.clone(),
                git_common_dir: common_dir.clone(),
                target_branch,
            })
        }
        [] => Err(format!(
            "⚠️ WORK TARGET REPOSITORY MISMATCH\n\n\
             Task targets `{}` (normalized `{}`), but no current-host known \
             repository or verified path hint resolves to that selector. \
             Each known checkout's full normalized identity set was compared: \
             {}. Register/open the target repo with Cassy, then retry. No git \
             merge/reachability check was run.",
            target.repo_selector,
            canonical_selector(&target.repo_selector),
            if observed.is_empty() {
                "none".to_string()
            } else {
                observed.join(", ")
            }
        )),
        many => Err(format!(
            "⚠️ AMBIGUOUS WORK TARGET\n\n\
             Task selector `{}` matched {} repositories on this host. Refusing \
             lifecycle mutation before git merge/reachability checks. Run \
             `cas known-repos status`, then `cas known-repos bind --repo <path>` \
             to make an explicit host-local choice.",
            target.repo_selector,
            many.len()
        )),
    }
}

/// Resolve a work target from the explicitly supplied project root only.
///
/// Informational paths such as merge-request revalidation may already hold the
/// authoritative project root, but the general resolver also consults the
/// host-wide known-repository registry so cross-repository lifecycle mutations
/// can resolve portable selectors. That registry is ambient state: a stale or
/// concurrently registered checkout can make an otherwise local selector look
/// ambiguous. Callers that have an explicit local root should use this helper
/// first and fall back to [`resolve_repo_context`] only for cross-repository
/// targets.
pub(crate) fn resolve_repo_context_from_local_root(
    cas_root: &Path,
    target: &WorkTarget,
) -> Result<RepoContext, String> {
    let (repo_root, git_common_dir) = match git_checkout_layout(cas_root) {
        Ok(layout) => layout,
        Err(error) => {
            tracing::debug!(
                resolution = "local_layout_error",
                cas_root = %cas_root.display(),
                target_selector = %target.repo_selector,
                error = %error,
                "merge revalidation local-root resolver could not inspect checkout"
            );
            return Err(error);
        }
    };
    if !repo_answers_to(&repo_root, &target.repo_selector) {
        tracing::debug!(
            resolution = "local_selector_mismatch",
            cas_root = %cas_root.display(),
            checkout_root = %repo_root.display(),
            git_common_dir = %git_common_dir.display(),
            target_selector = %target.repo_selector,
            identities = ?repo_identities(&repo_root),
            "merge revalidation local-root resolver rejected checkout identity"
        );
        return Err("explicit project root does not match work target selector".to_string());
    }
    let target_branch = validate_target_branch(&repo_root, &target.target_branch)?;
    tracing::debug!(
        resolution = "local_checkout",
        cas_root = %cas_root.display(),
        checkout_root = %repo_root.display(),
        git_common_dir = %git_common_dir.display(),
        target_selector = %target.repo_selector,
        target_branch = %target_branch,
        "merge revalidation resolved from explicit checkout root"
    );
    Ok(RepoContext {
        repo_selector: target.repo_selector.clone(),
        repo_root,
        git_common_dir,
        target_branch,
    })
}

pub(crate) fn resolve_repo_context_bounded(
    cas_root: &Path,
    target: &WorkTarget,
    probe: &BoundedRepoProbe,
) -> Result<RepoContext, BoundedRepoError> {
    if let Some(binding) = bounded_host_binding(&target.repo_selector)? {
        let (repo_root, git_common_dir) =
            validate_binding_bounded(&binding, &target.repo_selector, probe)?;
        return probe
            .output(
                &repo_root,
                &["check-ref-format", "--branch", &target.target_branch],
            )
            .map(|target_branch| RepoContext {
                repo_selector: target.repo_selector.clone(),
                repo_root,
                git_common_dir,
                target_branch,
            });
    }
    let want = canonical_selector(&target.repo_selector);
    let mut matches = Vec::new();
    for (repo_root, git_common_dir) in bounded_candidate_paths(cas_root, probe)? {
        match bounded_repo_identities(&repo_root, probe) {
            Ok(identities)
                if identities
                    .iter()
                    .any(|identity| canonical_selector(identity) == want) =>
            {
                matches.push((repo_root, git_common_dir));
            }
            Ok(_) | Err(BoundedRepoError::Unavailable) => {}
            Err(error) => return Err(error),
        }
    }
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [(repo_root, common_dir)] => probe
            .output(
                repo_root,
                &["check-ref-format", "--branch", &target.target_branch],
            )
            .map(|target_branch| RepoContext {
                repo_selector: target.repo_selector.clone(),
                repo_root: repo_root.clone(),
                git_common_dir: common_dir.clone(),
                target_branch,
            }),
        [] => Err(BoundedRepoError::Unavailable),
        _ => Err(BoundedRepoError::Ambiguous),
    }
}

pub(crate) fn resolve_path_context(
    path: &Path,
    target_branch: &str,
) -> Result<RepoContext, String> {
    let (repo_root, git_common_dir) = git_layout(path)?;
    Ok(RepoContext {
        repo_selector: selector_for_repo(&repo_root)?,
        repo_root,
        git_common_dir,
        target_branch: target_branch.to_string(),
    })
}

pub(crate) fn resolve_path_context_bounded(
    path: &Path,
    target_branch: &str,
    probe: &BoundedRepoProbe,
) -> Result<RepoContext, BoundedRepoError> {
    let (repo_root, git_common_dir) = bounded_git_layout(path, probe)?;
    Ok(RepoContext {
        repo_selector: bounded_selector_for_repo(&repo_root, probe)?,
        repo_root,
        git_common_dir,
        target_branch: target_branch.to_string(),
    })
}

pub(crate) fn validate_worktree_binding(
    task_id: &str,
    expected: &RepoContext,
    actual: &RepoContext,
    actual_branch: &str,
    worktree_path: &Path,
) -> Result<(), String> {
    // cas-1a1c (GH #151). `expected.repo_selector` is copied verbatim from the
    // task by `resolve_repo_context`, while `actual.repo_selector` is computed
    // fresh by `selector_for_repo`, which returns only the highest-priority
    // identity. A pinned checkout therefore compared `project:<pin>` against a
    // task's `remote:<origin>` and failed on its own repository — the same
    // defect as the work-target matcher, one door further in. Compare against
    // the checkout's full identity set, canonicalized on both sides.
    let want = canonical_selector(&expected.repo_selector);
    let selector_matches = canonical_selector(&actual.repo_selector) == want
        || repo_identities(&actual.repo_root)
            .iter()
            .any(|identity| canonical_selector(identity) == want);
    if selector_matches
        && actual.repo_root == expected.repo_root
        && actual.git_common_dir == expected.git_common_dir
        && actual_branch == expected.target_branch
    {
        return Ok(());
    }
    Err(format!(
        "⚠️ WORKTREE REPOSITORY MISMATCH\n\n\
         Task {task_id} targets repository `{}` (normalized `{want}`) branch \
         `{}`, but worktree {} resolves to repository `{}` (normalized `{}`) \
         branch `{actual_branch}` with a different canonical host-local \
         root/common-dir identity. Refusing before merge/reachability checks.",
        expected.repo_selector,
        expected.target_branch,
        worktree_path.display(),
        actual.repo_selector,
        canonical_selector(&actual.repo_selector),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

    fn git(repo: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .unwrap()
                .success()
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_git_probe_returns_typed_timeout_without_command_details() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let fake_git = dir.path().join("slow-git");
        std::fs::write(&fake_git, "#!/bin/sh\nsleep 10\n").unwrap();
        std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o700)).unwrap();
        let probe = BoundedRepoProbe::new(
            Deadline::after(Duration::from_millis(75)),
            Duration::from_millis(75),
            8,
        )
        .with_git_program(fake_git);
        let started = std::time::Instant::now();
        assert_eq!(
            probe.output(dir.path(), &["rev-parse", "HEAD"]),
            Err(BoundedRepoError::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn bounded_registry_fails_closed_before_scanning_too_many_host_repos() {
        TestEnvGuard::run_with_temp_home(|_home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let fixtures = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            for index in 0..3 {
                let repo = fixtures.path().join(format!("repo-{index}"));
                std::fs::create_dir(&repo).unwrap();
                crate::store::known_repos::register_repo_strict(&repo).unwrap();
            }
            assert_eq!(
                bounded_registry_paths(2),
                Err(BoundedRepoError::CandidateLimit)
            );
        });
    }

    #[test]
    fn linked_worktree_and_symlink_share_selector_and_common_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q", "-b", "master"]);
        git(
            &main,
            &["remote", "add", "origin", "git@github.com:org/repo.git"],
        );
        std::fs::write(main.join("a"), "a").unwrap();
        git(&main, &["add", "a"]);
        git(
            &main,
            &[
                "-c",
                "user.name=Cassy",
                "-c",
                "user.email=cas@example.com",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        let linked = dir.path().join("linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/w",
                linked.to_str().unwrap(),
            ],
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(&linked, dir.path().join("alias")).unwrap();

        let a = resolve_path_context(&main, "master").unwrap();
        let b = resolve_path_context(&linked, "master").unwrap();
        assert_eq!(a.repo_selector, b.repo_selector);
        assert_eq!(a.git_common_dir, b.git_common_dir);
        #[cfg(unix)]
        {
            let c = resolve_path_context(&dir.path().join("alias"), "master").unwrap();
            assert_eq!(a.git_common_dir, c.git_common_dir);
        }
    }

    #[test]
    fn local_root_resolver_uses_linked_checkout_root_for_identity() {
        TestEnvGuard::run_with_temp_home(|_| {
            let dir = tempfile::TempDir::new().unwrap();
            let main = dir.path().join("main");
            std::fs::create_dir(&main).unwrap();
            git(&main, &["init", "-q", "-b", "main"]);
            std::fs::write(main.join("base.txt"), "base\n").unwrap();
            git(&main, &["add", "base.txt"]);
            git(
                &main,
                &[
                    "-c",
                    "user.name=Cassy",
                    "-c",
                    "user.email=cas@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "base",
                ],
            );

            let linked = dir.path().join("linked");
            git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "factory/local",
                    linked.to_str().unwrap(),
                ],
            );
            std::fs::create_dir(linked.join(".cas")).unwrap();
            std::fs::write(
                linked.join(".cas/config.toml"),
                "[project]\ncanonical_id = \"linked-project\"\n",
            )
            .unwrap();

            let target = WorkTarget {
                repo_selector: "project:linked-project".to_string(),
                target_branch: "main".to_string(),
            };
            let resolved = resolve_repo_context_from_local_root(&linked.join(".cas"), &target)
                .expect("linked checkout-local config must resolve before registry fallback");
            assert_eq!(resolved.repo_root, canonical(linked));
            assert_eq!(resolved.git_common_dir, canonical(main.join(".git")));
        });
    }

    #[test]
    fn nonexistent_known_repo_is_skipped_before_git_resolution() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let stale = home.join("deleted-checkout");
            std::fs::create_dir_all(&stale).unwrap();
            crate::store::known_repos::register_repo_strict(&stale).unwrap();
            std::fs::remove_dir(&stale).unwrap();

            let project = home.join("active-project");
            std::fs::create_dir_all(&project).unwrap();
            git(&project, &["init", "-q", "-b", "main"]);
            git(
                &project,
                &[
                    "remote",
                    "add",
                    "origin",
                    "git@example.invalid:org/active-project.git",
                ],
            );
            std::fs::create_dir(project.join(".cas")).unwrap();
            std::fs::write(
                project.join(".cas/config.toml"),
                "[project]\ncanonical_id = \"active-project\"\n",
            )
            .unwrap();

            let target = WorkTarget {
                repo_selector: "project:active-project".to_string(),
                target_branch: "main".to_string(),
            };
            let resolved =
                resolve_repo_context(&project.join(".cas"), &target).expect("active repo resolves");
            assert_eq!(resolved.repo_selector, target.repo_selector);
            assert_eq!(resolved.target_branch, "main");
            assert!(
                !stale.exists(),
                "the stale registry entry must remain nonexistent"
            );
        });
    }

    #[test]
    fn explicit_target_is_normalized_registered_and_path_free_when_serialized() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let fixtures = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            let repo = fixtures.path().join("checkout");
            std::fs::create_dir(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            git(
                &repo,
                &["remote", "add", "origin", "https://github.com/Org/Repo.git"],
            );
            std::fs::write(repo.join("base"), "base").unwrap();
            git(&repo, &["add", "base"]);
            git(
                &repo,
                &[
                    "-c",
                    "user.name=Cassy",
                    "-c",
                    "user.email=cas@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "base",
                ],
            );
            std::fs::create_dir(repo.join(".cas")).unwrap();

            let target =
                declare_work_target(&repo.join(".cas"), Some(repo.to_str().unwrap()), None)
                    .unwrap()
                    .unwrap();
            assert_eq!(target.repo_selector, "remote:github.com/Org/Repo");
            assert_eq!(target.target_branch, "main");

            let json = serde_json::to_string(&target).unwrap();
            assert!(!json.contains(home.to_string_lossy().as_ref()));
            assert!(!json.contains("repo_root"));
            assert!(!json.contains("git_common_dir"));

            let known = crate::store::known_repos::open_host_known_repo_store()
                .unwrap()
                .list()
                .unwrap();
            assert_eq!(known.len(), 1);
            assert_eq!(known[0].path, repo.canonicalize().unwrap());
        });
    }

    #[test]
    fn declared_repo_b_resolves_from_repo_a_spawn_context() {
        TestEnvGuard::run_with_temp_home(|_home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let fixtures = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            let repo_a = fixtures.path().join("spawn-a");
            let repo_b = fixtures.path().join("work-b");
            for (repo, remote) in [
                (&repo_a, "git@github.com:org/spawn-a.git"),
                (&repo_b, "git@github.com:org/work-b.git"),
            ] {
                std::fs::create_dir(repo).unwrap();
                git(repo, &["init", "-q", "-b", "main"]);
                git(repo, &["remote", "add", "origin", remote]);
                std::fs::create_dir(repo.join(".cas")).unwrap();
            }
            let target = declare_work_target(
                &repo_a.join(".cas"),
                Some(repo_b.to_str().unwrap()),
                Some("main"),
            )
            .unwrap()
            .unwrap();
            let context = resolve_repo_context(&repo_a.join(".cas"), &target).unwrap();
            assert_eq!(context.repo_root, repo_b.canonicalize().unwrap());
            assert_eq!(context.repo_selector, "remote:github.com/org/work-b");
        });
    }

    #[test]
    fn duplicate_selector_binding_survives_restart_and_never_falls_back() {
        let mut guard = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let fixtures = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let repo_a = fixtures.path().join("clone-a");
        let repo_b = fixtures.path().join("clone-b");
        for repo in [&repo_a, &repo_b] {
            std::fs::create_dir(repo).unwrap();
            git(repo, &["init", "-q", "-b", "main"]);
            git(
                repo,
                &["remote", "add", "origin", "git@github.com:org/shared.git"],
            );
            std::fs::create_dir(repo.join(".cas")).unwrap();
            crate::store::known_repos::register_repo_strict(repo).unwrap();
        }
        let target = WorkTarget {
            repo_selector: "remote:github.com/org/shared".to_string(),
            target_branch: "main".to_string(),
        };
        let ambiguous = resolve_repo_context(&repo_a.join(".cas"), &target).unwrap_err();
        assert!(ambiguous.contains("AMBIGUOUS WORK TARGET"));
        assert!(ambiguous.contains("known-repos bind --repo"));

        let (_, root_b, common_b) = binding_identity_for_path(&repo_b).unwrap();
        {
            let store = crate::store::known_repos::open_host_known_repo_store().unwrap();
            store
                .bind(&target.repo_selector, &root_b, &common_b)
                .unwrap();
        }

        // A fresh store open and a cwd in the newer/unrelated clone cannot
        // influence the explicit host-local choice.
        guard.set_current_dir(&repo_a);
        let resolved = resolve_repo_context(&repo_a.join(".cas"), &target).unwrap();
        assert_eq!(resolved.repo_root, repo_b.canonicalize().unwrap());
        assert_eq!(
            resolved.git_common_dir,
            repo_b.join(".git").canonicalize().unwrap()
        );

        // Live identity drift invalidates the binding. Clone A remains a
        // matching candidate, but stale bindings never fall back.
        git(
            &repo_b,
            &[
                "remote",
                "set-url",
                "origin",
                "git@github.com:org/changed.git",
            ],
        );
        let stale = resolve_repo_context(&repo_a.join(".cas"), &target).unwrap_err();
        assert!(stale.contains("STALE WORK TARGET BINDING"));
        assert!(!stale.contains(repo_a.to_string_lossy().as_ref()));
        assert!(!stale.contains(repo_b.to_string_lossy().as_ref()));

        let store = crate::store::known_repos::open_host_known_repo_store().unwrap();
        assert_eq!(store.unbind(&target.repo_selector).unwrap(), 1);
        let after_unbind = resolve_repo_context(&repo_a.join(".cas"), &target).unwrap();
        assert_eq!(
            after_unbind.repo_root,
            repo_a.canonicalize().unwrap(),
            "unbind removes only the binding and restores ordinary unique matching"
        );
        assert!(
            store
                .list()
                .unwrap()
                .iter()
                .any(|repo| repo.path == repo_b.canonicalize().unwrap()),
            "unbind must retain the known-repo registration"
        );
    }

    #[test]
    fn wrong_or_disappeared_binding_fails_closed_without_path_disclosure() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let intended = home.join("intended");
            let wrong = home.join("wrong");
            for (repo, remote) in [
                (&intended, "git@github.com:org/intended.git"),
                (&wrong, "git@github.com:org/wrong.git"),
            ] {
                std::fs::create_dir(repo).unwrap();
                git(repo, &["init", "-q", "-b", "main"]);
                git(repo, &["remote", "add", "origin", remote]);
                std::fs::create_dir(repo.join(".cas")).unwrap();
                crate::store::known_repos::register_repo_strict(repo).unwrap();
            }
            let (_, wrong_root, wrong_common) = binding_identity_for_path(&wrong).unwrap();
            let selector = "remote:github.com/org/intended";
            crate::store::known_repos::open_host_known_repo_store()
                .unwrap()
                .bind(selector, &wrong_root, &wrong_common)
                .unwrap();
            let target = WorkTarget {
                repo_selector: selector.to_string(),
                target_branch: "main".to_string(),
            };
            let wrong_error = resolve_repo_context(&intended.join(".cas"), &target).unwrap_err();
            assert!(wrong_error.contains("STALE WORK TARGET BINDING"));
            assert!(!wrong_error.contains(home.to_string_lossy().as_ref()));

            std::fs::remove_dir_all(&wrong).unwrap();
            let missing_error = resolve_repo_context(&intended.join(".cas"), &target).unwrap_err();
            assert!(missing_error.contains("STALE WORK TARGET BINDING"));
            assert!(!missing_error.contains(home.to_string_lossy().as_ref()));
        });
    }

    #[cfg(unix)]
    #[test]
    fn binding_cli_identity_rejects_symlinks_nested_paths_and_linked_worktrees() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("main");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(
            &main,
            &["remote", "add", "origin", "git@github.com:org/shared.git"],
        );
        std::fs::create_dir(main.join("nested")).unwrap();
        std::os::unix::fs::symlink(&main, root.join("alias")).unwrap();
        let parent_alias = root.join("parent-alias");
        std::os::unix::fs::symlink(&root, &parent_alias).unwrap();
        assert!(binding_identity_for_path(&root.join("alias")).is_err());
        assert!(binding_identity_for_path(&parent_alias.join("main")).is_err());
        assert!(binding_identity_for_path(&main.join("nested")).is_err());

        std::fs::write(main.join("base"), "base").unwrap();
        git(&main, &["add", "base"]);
        git(
            &main,
            &[
                "-c",
                "user.name=Cassy",
                "-c",
                "user.email=cas@example.com",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        let linked = root.join("linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/worker",
                linked.to_str().unwrap(),
            ],
        );
        assert!(binding_identity_for_path(&linked).is_err());
        assert!(binding_identity_for_path(&main).is_ok());
    }

    #[test]
    fn unknown_selector_fails_before_git_evidence_is_consulted() {
        TestEnvGuard::run_with_temp_home(|_| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let dir = tempfile::TempDir::new().unwrap();
            let target = WorkTarget {
                repo_selector: "remote:github.com/missing/repo".to_string(),
                target_branch: "main".to_string(),
            };
            let error = resolve_repo_context(dir.path(), &target).unwrap_err();
            assert!(error.contains("no current-host known repository"));
            assert!(error.contains("No git merge/reachability check was run"));
        });
    }

    /// cas-1a1c (GH #151) reproduction. The checkout carries BOTH identities:
    /// a `[project] canonical_id` pin and an `origin` that normalizes to
    /// exactly the selector the task declares. `selector_for_repo` only ever
    /// returns the higher-priority one, so the remote-form task could never
    /// match and `task start` reported a mismatch against its own repository.
    #[test]
    fn remote_form_target_matches_a_checkout_that_also_carries_a_project_pin() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let repo = home.join("cas-src");
            std::fs::create_dir_all(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            git(
                &repo,
                &["remote", "add", "origin", "git@github.com:pippenz/cas.git"],
            );
            std::fs::create_dir(repo.join(".cas")).unwrap();
            std::fs::write(
                repo.join(".cas/config.toml"),
                "[project]\ncanonical_id = \"cas-src\"\n",
            )
            .unwrap();

            let target = WorkTarget {
                repo_selector: "remote:github.com/pippenz/cas".to_string(),
                target_branch: "main".to_string(),
            };
            let resolved = resolve_repo_context(&repo.join(".cas"), &target)
                .expect("a remote-form target must match its own checkout");
            assert_eq!(resolved.repo_root, canonical(repo.clone()));
            assert_eq!(resolved.target_branch, "main");
        });
    }

    /// AC1: every URL shape a checkout's `origin` can take must match the same
    /// remote-form target, including from a linked worktree, and including the
    /// bare `host/owner/repo` form a persisted selector actually carries.
    #[test]
    fn remote_form_target_matches_every_origin_url_shape_and_linked_worktrees() {
        for origin in [
            "https://github.com/pippenz/cas.git",
            "https://github.com/pippenz/cas",
            "http://github.com/pippenz/cas.git",
            "git@github.com:pippenz/cas.git",
            "git@github.com:pippenz/cas",
            "ssh://git@github.com/pippenz/cas.git",
        ] {
            TestEnvGuard::run_with_temp_home(|home| {
                crate::store::known_repos::ensure_host_schema().unwrap();
                let repo = home.join("checkout");
                std::fs::create_dir_all(&repo).unwrap();
                git(&repo, &["init", "-q", "-b", "main"]);
                git(&repo, &["remote", "add", "origin", origin]);
                // `candidate_paths` drops non-existent hints, and this checkout
                // carries no pin — the origin is its only identity, which is
                // exactly the case under test.
                std::fs::create_dir(repo.join(".cas")).unwrap();
                std::fs::write(repo.join("a"), "a").unwrap();
                git(&repo, &["add", "a"]);
                git(
                    &repo,
                    &[
                        "-c",
                        "user.name=Cassy",
                        "-c",
                        "user.email=cas@example.com",
                        "commit",
                        "-q",
                        "-m",
                        "base",
                    ],
                );

                let target = WorkTarget {
                    repo_selector: "remote:github.com/pippenz/cas".to_string(),
                    target_branch: "main".to_string(),
                };
                resolve_repo_context(&repo.join(".cas"), &target)
                    .unwrap_or_else(|e| panic!("origin {origin} must match the target: {e}"));

                // Same repository, reached through a linked worktree: git_layout
                // resolves it to the primary checkout, so the identity holds.
                let linked = home.join("linked");
                git(
                    &repo,
                    &[
                        "worktree",
                        "add",
                        "-q",
                        "-b",
                        "factory/w",
                        linked.to_str().unwrap(),
                    ],
                );
                let from_linked = resolve_path_context(&linked, "main").unwrap();
                assert!(
                    repo_answers_to(&from_linked.repo_root, &target.repo_selector),
                    "linked worktree of origin {origin} must answer to the target"
                );
            });
        }
    }

    /// AC1 (selector side): the persisted selector itself may be written in any
    /// shape; all of them canonicalize to one comparison value.
    #[test]
    fn selector_payloads_canonicalize_across_url_shapes() {
        let want = "remote:github.com/pippenz/cas";
        for selector in [
            "remote:github.com/pippenz/cas",
            "remote:github.com/pippenz/cas.git",
            "remote:https://github.com/pippenz/cas",
            "remote:https://github.com/pippenz/cas.git",
            "remote:git@github.com:pippenz/cas.git",
            "remote:ssh://git@github.com/pippenz/cas.git",
            "remote:GitHub.com/pippenz/cas",
            "remote:github.com/pippenz/cas/",
        ] {
            assert_eq!(canonical_selector(selector), want, "selector {selector}");
        }
        // Opaque project ids are untouched, and a malformed remote payload
        // still compares equal to itself instead of collapsing to empty.
        assert_eq!(canonical_selector("project:cas-src"), "project:cas-src");
        assert_eq!(canonical_selector("remote:nonsense"), "remote:nonsense");
        // A filesystem path must never be read as a bare remote.
        assert_eq!(bare_host_owner_repo("/tmp/foo/bar"), None);
        assert_eq!(bare_host_owner_repo("tmp/foo/bar"), None);
        assert_eq!(bare_host_owner_repo("github.com/pippenz"), None);
    }

    /// AC2 regression: canonicalization must not turn the matcher into a
    /// rubber stamp. A genuinely different repository still mismatches, and the
    /// error names both normalized sides (AC3).
    #[test]
    fn a_genuinely_different_repository_still_mismatches_and_names_both_sides() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let repo = home.join("other");
            std::fs::create_dir_all(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            git(
                &repo,
                &[
                    "remote",
                    "add",
                    "origin",
                    "git@github.com:someone/other.git",
                ],
            );
            std::fs::create_dir(repo.join(".cas")).unwrap();
            std::fs::write(
                repo.join(".cas/config.toml"),
                "[project]\ncanonical_id = \"other-project\"\n",
            )
            .unwrap();

            let target = WorkTarget {
                repo_selector: "remote:github.com/pippenz/cas".to_string(),
                target_branch: "main".to_string(),
            };
            let error = resolve_repo_context(&repo.join(".cas"), &target)
                .expect_err("a different repository must still mismatch");
            assert!(error.contains("WORK TARGET REPOSITORY MISMATCH"));
            // AC3: both normalized sides are printed.
            assert!(
                error.contains("normalized `remote:github.com/pippenz/cas`"),
                "target side missing from: {error}"
            );
            // The checkout's identities are reported as one SET, so a reader
            // sees immediately that the pin was considered and the remote
            // still did not match — not two unrelated-looking lines.
            assert!(
                error.contains("project:other-project + remote:github.com/someone/other"),
                "grouped identity set missing from: {error}"
            );
            // Path-free by convention for these errors.
            assert!(
                !error.contains(home.to_string_lossy().as_ref()),
                "host paths must not be disclosed: {error}"
            );
        });
    }

    /// AC4: an absolute-path target still stamps exactly the selector it always
    /// did — `declare_work_target` is untouched by the matcher change.
    #[test]
    fn absolute_path_target_declaration_is_unchanged() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let repo = home.join("abs-target");
            std::fs::create_dir_all(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            git(
                &repo,
                &["remote", "add", "origin", "git@github.com:pippenz/cas.git"],
            );
            std::fs::create_dir(repo.join(".cas")).unwrap();
            std::fs::write(
                repo.join(".cas/config.toml"),
                "[project]\ncanonical_id = \"pinned-id\"\n",
            )
            .unwrap();

            let declared = declare_work_target(
                &repo.join(".cas"),
                Some(repo.to_str().unwrap()),
                Some("main"),
            )
            .unwrap()
            .expect("an explicit target must be declared");
            // The pin still wins when stamping: priority order is preserved.
            assert_eq!(declared.repo_selector, "project:pinned-id");
            assert_eq!(declared.target_branch, "main");
        });
    }

    #[test]
    fn default_branch_master_survives_detached_head() {
        let dir = tempfile::TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "master"]);
        std::fs::write(dir.path().join("a"), "a").unwrap();
        git(dir.path(), &["add", "a"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=Cassy",
                "-c",
                "user.email=cas@example.com",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        git(dir.path(), &["checkout", "-q", "--detach"]);
        assert_eq!(resolve_default_branch(dir.path()).unwrap(), "master");
    }

    #[test]
    fn branch_validation_uses_git_ref_grammar() {
        let dir = tempfile::TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        assert_eq!(
            validate_target_branch(dir.path(), "feature/valid").unwrap(),
            "feature/valid"
        );
        for invalid in ["-option", "bad..name", "bad name", "trailing."] {
            assert!(
                validate_target_branch(dir.path(), invalid).is_err(),
                "Git must reject invalid branch {invalid:?}"
            );
        }
    }

    #[test]
    fn epic_branch_scan_uses_declared_repo_not_process_cwd() {
        let mut guard = TestEnvGuard::temp_home();
        let repo_a = guard.home().join("spawn-a");
        let repo_b = guard.home().join("work-b");
        for repo in [&repo_a, &repo_b] {
            std::fs::create_dir(repo).unwrap();
            git(repo, &["init", "-q", "-b", "main"]);
            std::fs::write(repo.join("base"), "base").unwrap();
            git(repo, &["add", "base"]);
            git(
                repo,
                &[
                    "-c",
                    "user.name=Cassy",
                    "-c",
                    "user.email=cas@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "base",
                ],
            );
        }

        git(&repo_a, &["checkout", "-q", "-b", "cas-epic/noise"]);
        std::fs::write(repo_a.join("noise"), "repo a only").unwrap();
        git(&repo_a, &["add", "noise"]);
        git(
            &repo_a,
            &[
                "-c",
                "user.name=Cassy",
                "-c",
                "user.email=cas@example.com",
                "commit",
                "-q",
                "-m",
                "noise",
            ],
        );
        guard.set_current_dir(&repo_a);

        assert!(
            crate::mcp::tools::check_unmerged_epic_branches(&repo_b, "cas-epic", "main").is_empty(),
            "repo A epic branch noise must not contaminate repo B close"
        );

        git(&repo_b, &["checkout", "-q", "-b", "cas-epic/real"]);
        std::fs::write(repo_b.join("work"), "repo b").unwrap();
        git(&repo_b, &["add", "work"]);
        git(
            &repo_b,
            &[
                "-c",
                "user.name=Cassy",
                "-c",
                "user.email=cas@example.com",
                "commit",
                "-q",
                "-m",
                "work",
            ],
        );
        git(&repo_b, &["checkout", "-q", "main"]);
        assert_eq!(
            crate::mcp::tools::check_unmerged_epic_branches(&repo_b, "cas-epic", "main"),
            vec!["cas-epic/real"]
        );
    }

    #[test]
    fn wrong_spawn_repo_is_not_used_for_declared_repo_close_gate() {
        use crate::mcp::tools::TaskCloseRequest;
        use crate::mcp::tools::core::task::lifecycle::close_ops::{
            MergeStateGateOutcome, TaskCommitReceiptWindow, count_unmerged_factory_commits,
            run_factory_branch_merge_gate, validate_task_commit_receipt,
        };
        use crate::types::{Task, TaskStatus};

        TestEnvGuard::run_with_temp_home(|_home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let fixtures = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            let repo_a = fixtures.path().join("spawn-a");
            let repo_b = fixtures.path().join("work-b");
            for (repo, remote) in [
                (&repo_a, "git@github.com:org/spawn-a.git"),
                (&repo_b, "git@github.com:org/work-b.git"),
            ] {
                std::fs::create_dir(repo).unwrap();
                git(repo, &["init", "-q", "-b", "master"]);
                git(repo, &["remote", "add", "origin", remote]);
                std::fs::write(repo.join("base"), "base").unwrap();
                git(repo, &["add", "base"]);
                git(
                    repo,
                    &[
                        "-c",
                        "user.name=Cassy",
                        "-c",
                        "user.email=cas@example.com",
                        "commit",
                        "-q",
                        "-m",
                        "base",
                    ],
                );
                std::fs::create_dir(repo.join(".cas")).unwrap();
            }

            // Spawn repo A carries inherited epic history not on its trunk.
            git(&repo_a, &["checkout", "-q", "-b", "epic/x"]);
            std::fs::write(repo_a.join("epic"), "inherited").unwrap();
            git(&repo_a, &["add", "epic"]);
            git(
                &repo_a,
                &[
                    "-c",
                    "user.name=Cassy",
                    "-c",
                    "user.email=cas@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "epic",
                ],
            );
            git(&repo_a, &["checkout", "-q", "-b", "factory/worker"]);

            // Actual work is in B and has already landed on B/master.
            git(&repo_b, &["checkout", "-q", "-b", "factory/worker"]);
            std::fs::write(repo_b.join("feature"), "done").unwrap();
            git(&repo_b, &["add", "feature"]);
            git(
                &repo_b,
                &[
                    "-c",
                    "user.name=Cassy",
                    "-c",
                    "user.email=cas@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "feature",
                ],
            );
            let receipt = git_output(&repo_b, &["rev-parse", "HEAD"]).unwrap();
            git(&repo_b, &["checkout", "-q", "master"]);
            git(
                &repo_b,
                &[
                    "-c",
                    "user.name=Cassy",
                    "-c",
                    "user.email=cas@example.com",
                    "merge",
                    "-q",
                    "--no-ff",
                    "factory/worker",
                ],
            );

            let target = declare_work_target(
                &repo_a.join(".cas"),
                Some(repo_b.to_str().unwrap()),
                Some("master"),
            )
            .unwrap()
            .unwrap();
            let context = resolve_repo_context(&repo_a.join(".cas"), &target).unwrap();
            let mut task = Task::new("cas-cross".to_string(), "cross repo".to_string());
            task.assignee = Some("worker".to_string());
            task.status = TaskStatus::InProgress;
            task.deliverables.work_target = Some(target);
            let request = TaskCloseRequest {
                stranded_branch_override: None,
                id: task.id.clone(),
                reason: None,
                supervisor_override: Some(true),
                legacy_bypass_code_review: None,
                search_manifest: None,
                commit_receipt: None,
            };

            assert_eq!(
                count_unmerged_factory_commits(&repo_a, "factory/worker", "master"),
                1,
                "precondition: spawn repo would falsely report inherited epic history"
            );
            assert!(matches!(
                run_factory_branch_merge_gate(
                    &task,
                    &request,
                    &context.target_branch,
                    &context.repo_root
                ),
                MergeStateGateOutcome::Proceed
            ));

            let window = TaskCommitReceiptWindow {
                not_before: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                basis: "test task creation",
                task_floor: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                identity: Default::default(),
            };
            assert!(
                validate_task_commit_receipt(&repo_a, &receipt, "master", &window).is_err(),
                "receipt absent from spawn repo must never validate"
            );
            assert!(
                validate_task_commit_receipt(
                    &context.repo_root,
                    &receipt,
                    &context.target_branch,
                    &window
                )
                .is_ok(),
                "receipt must validate in the declared work repository"
            );
        });
    }

    #[test]
    fn worktree_binding_rejects_repo_clone_or_branch_mismatch_before_merge() {
        let expected = RepoContext {
            repo_selector: "remote:github.com/org/work".to_string(),
            repo_root: PathBuf::from("/runtime/work"),
            git_common_dir: PathBuf::from("/runtime/work/.git"),
            target_branch: "master".to_string(),
        };
        let wrong_repo = RepoContext {
            repo_selector: "remote:github.com/org/spawn".to_string(),
            repo_root: PathBuf::from("/runtime/spawn"),
            git_common_dir: PathBuf::from("/runtime/spawn/.git"),
            target_branch: "master".to_string(),
        };
        let error = validate_worktree_binding(
            "cas-x",
            &expected,
            &wrong_repo,
            "master",
            Path::new("/runtime/spawn/wt"),
        )
        .unwrap_err();
        assert!(error.contains("before merge/reachability checks"));

        let wrong_clone = RepoContext {
            repo_selector: expected.repo_selector.clone(),
            repo_root: PathBuf::from("/runtime/work-clone"),
            git_common_dir: PathBuf::from("/runtime/work-clone/.git"),
            target_branch: expected.target_branch.clone(),
        };
        assert!(
            validate_worktree_binding(
                "cas-x",
                &expected,
                &wrong_clone,
                "master",
                Path::new("/runtime/work-clone/wt")
            )
            .is_err(),
            "a portable selector is not sufficient identity for one live clone"
        );

        let linked_worktree = RepoContext {
            repo_selector: expected.repo_selector.clone(),
            repo_root: expected.repo_root.clone(),
            git_common_dir: expected.git_common_dir.clone(),
            target_branch: expected.target_branch.clone(),
        };
        assert!(
            validate_worktree_binding(
                "cas-x",
                &expected,
                &linked_worktree,
                "master",
                Path::new("/runtime/linked-worktree")
            )
            .is_ok(),
            "git_layout canonicalizes linked worktrees to their main root/common-dir identity"
        );

        assert!(
            validate_worktree_binding(
                "cas-x",
                &expected,
                &expected,
                "main",
                Path::new("/runtime/work/wt")
            )
            .is_err()
        );
        assert!(
            validate_worktree_binding(
                "cas-x",
                &expected,
                &expected,
                "master",
                Path::new("/runtime/work/wt")
            )
            .is_ok()
        );
    }
}

/// cas-156b (GH #135): the nativity predicate guarding `task start`.
///
/// AC2 is proven by ABSENCE — the normal case must stay silent — so each
/// zero-friction shape gets its own no-warn test rather than one combined
/// case. A refactor that silently breaks a single veto then fails a named
/// test instead of quietly making every start noisy.
#[cfg(test)]
mod task_anchor_tests {
    use super::*;

    /// The shape observed in GH #135: a task replicated from another project.
    /// Cloud pull writes the task row but never dependency edges and never
    /// per-project agents, so the replica lands orphaned with an assignee that
    /// means nothing on this host.
    fn foreign_replica() -> TaskAnchorEvidence {
        TaskAnchorEvidence {
            has_work_target: false,
            dependency_edge_count: 0,
            assignee_is_local_agent: false,
            cloud_sync_configured: true,
        }
    }

    #[test]
    fn foreign_replica_start_is_reported_cas_156b() {
        assert!(
            task_has_no_local_anchor(&foreign_replica()),
            "the #135 incident shape must be reported"
        );
    }

    // ── AC2: each zero-friction shape stays silent, pinned individually ──

    #[test]
    fn epic_child_task_never_warns_cas_156b() {
        // The overwhelmingly common factory shape. `create_atomic` writes a
        // ParentChild edge for the epic, which cloud pull can never produce.
        assert!(!task_has_no_local_anchor(&TaskAnchorEvidence {
            dependency_edge_count: 1,
            ..foreign_replica()
        }));
    }

    #[test]
    fn director_assigned_task_never_warns_cas_156b() {
        // Assignee resolves to an agent registered on this host.
        assert!(!task_has_no_local_anchor(&TaskAnchorEvidence {
            assignee_is_local_agent: true,
            ..foreign_replica()
        }));
    }

    #[test]
    fn explicitly_targeted_task_never_warns_cas_156b() {
        // Work-target tasks are resolved (and failed closed) by
        // `resolve_repo_context`; this predicate must not double-report them.
        assert!(!task_has_no_local_anchor(&TaskAnchorEvidence {
            has_work_target: true,
            ..foreign_replica()
        }));
    }

    #[test]
    fn cloud_unconfigured_project_never_warns_cas_156b() {
        // A database that never syncs cannot have received a replica.
        assert!(!task_has_no_local_anchor(&TaskAnchorEvidence {
            cloud_sync_configured: false,
            ..foreign_replica()
        }));
    }

    /// ACCEPTED BY DESIGN — do not "fix" this into a fifth veto.
    ///
    /// A standalone task in a cloud-synced project with no epic, no
    /// dependency edge, no assignee and no work target is indistinguishable
    /// from a replica using local evidence alone, so it warns. This residual
    /// was raised and explicitly accepted when the heuristic was reviewed:
    /// the output is advisory, never blocks the lease, and names the one-line
    /// remedy that silences it (`task action=update ... target_repo=`).
    ///
    /// The quieter alternative — warn only when the database already shows a
    /// task anchored to a DIFFERENT repository — was DECLINED because it
    /// couples this guard to the foreign-row detector in cas-fc6fa (GH #133),
    /// which has not landed. If that detector ships and this residual annoys
    /// a real user, tighten it there; do not add a veto here without
    /// revisiting that decision.
    #[test]
    fn unassigned_standalone_task_warns_and_that_is_accepted_cas_156b() {
        assert!(
            task_has_no_local_anchor(&TaskAnchorEvidence {
                assignee_is_local_agent: false,
                ..foreign_replica()
            }),
            "documented false positive: see this test's doc comment before changing"
        );
    }

    // ── The warning text itself ──

    #[test]
    fn warning_names_task_project_and_both_remedies_cas_156b() {
        let warning = unanchored_task_start_warning(
            "cas-9999",
            Path::new("/home/dev/gabber-studio"),
            Some("gabber-studio"),
        );
        assert!(
            warning.contains("cas-9999"),
            "must name the task: {warning}"
        );
        assert!(
            warning.contains("/home/dev/gabber-studio"),
            "must name the current project root: {warning}"
        );
        assert!(
            warning.contains("gabber-studio"),
            "must name the cloud project: {warning}"
        );
        assert!(
            warning.contains("target_repo="),
            "must give the anchor-it remedy: {warning}"
        );
        assert!(
            warning.contains("cas cloud purge-foreign"),
            "must give the contamination remedy: {warning}"
        );
        assert!(
            warning.contains("advisory, not a block"),
            "must state that the lease still proceeded: {warning}"
        );
    }

    #[test]
    fn warning_without_canonical_id_still_names_the_root_cas_156b() {
        let warning = unanchored_task_start_warning("cas-9999", Path::new("/tmp/proj"), None);
        assert!(warning.contains("/tmp/proj"), "{warning}");
        assert!(
            !warning.contains("cloud project"),
            "must not invent a cloud project name it does not have: {warning}"
        );
    }

    #[test]
    fn child_inherits_live_epic_lane_but_keeps_nondefault_target_cas_edba() {
        let mut epic = cas_types::Task::new("cas-epic".into(), "Epic".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = Some("epic/live-delivery".into());
        epic.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:fixture".into(),
            target_branch: "main".into(),
        });

        let child = cas_types::Task::new("cas-child".into(), "Child".into());
        let inherited = default_child_work_target_from_epic(&child, &epic)
            .expect("an untargeted child inherits its epic lane");
        assert_eq!(inherited.target_branch, "epic/live-delivery");
        assert_eq!(inherited.repo_selector, "project:fixture");

        let mut trunk_child = child.clone();
        trunk_child.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:fixture".into(),
            target_branch: "main".into(),
        });
        assert_eq!(
            default_child_work_target_from_epic(&trunk_child, &epic)
                .expect("the inherited trunk default must move to the live lane")
                .target_branch,
            "epic/live-delivery"
        );

        let mut explicit_child = child;
        explicit_child.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:fixture".into(),
            target_branch: "release/operator-selected".into(),
        });
        assert!(
            default_child_work_target_from_epic(&explicit_child, &epic).is_none(),
            "a non-default child target is explicit operator authority"
        );
    }
}
