//! Canonical worktree binding for factory workers (cas-30c6).
//!
//! A spawned worker owns exactly one branch — `factory/<its own name>` — and
//! exactly one directory, the worktree checked out on that branch. Every
//! surface that answers "which repository am I bound to?" must resolve that
//! same canonical binding from the worker's *registered identity*, never from
//! whatever directory the ambient process happens to be sitting in:
//!
//! - provisioning ([`crate::ui::factory::app::WorkerSpawnPrep::run`]) proves
//!   the binding before any harness process is started, and fails the spawn
//!   closed otherwise;
//! - the PreToolUse commit guard denies writes made from another worker's
//!   branch;
//! - `coordination my_context` reports the binding it actually resolved, so a
//!   misbinding is visible instead of silent.
//!
//! Two incidents motivated this module. In the first, a worker's harness stayed
//! bound to the dirty shared checkout because a stale plain directory sat where
//! its worktree belonged: git climbs out of a non-worktree directory into the
//! enclosing checkout, so a branch-only comparison matched. In the second, a
//! respawned worker was bound to a *sibling* worker's worktree, which every
//! existing check accepted because `factory/*` is a permitted branch prefix.

use std::path::Path;

/// The one branch a factory worker of this name owns.
///
/// Mirrors `WorktreeManager::branch_name_for_worker`, which is what
/// provisioning actually creates.
pub fn expected_worker_branch(worker_name: &str) -> String {
    format!("factory/{}", worker_name.trim())
}

/// The per-task branch a worker uses while its own `factory/<name>` is frozen
/// for a parked delivery under QA (cas-73b8): `factory/<name>-<task-id>`.
///
/// It cannot be `factory/<name>/<task-id>`: git refs are files, so a ref named
/// `factory/<name>` and one inside a `factory/<name>/` directory cannot
/// coexist.
pub fn worker_task_branch(worker_name: &str, task_id: &str) -> String {
    format!("factory/{}-{}", worker_name.trim(), task_id.trim())
}

/// cas-47a2: a reset ends the previous holder's delivery cycle. Its branch
/// moves to handoff history and its anchor to historical anchors, so the live
/// binding read by peers, epic views and merge alerts stops naming a dead
/// worker's branch; the next holder's commit or park binds its own. Returns
/// the retired branch.
pub fn retire_branch_binding(deliverables: &mut cas_types::TaskDeliverables) -> Option<String> {
    deliverables.factory_branch_start = None;
    let branch = deliverables.parked_branch.take();
    if let Some(branch) = branch.as_deref() {
        deliverables.record_handoff_branch(branch);
    }
    deliverables.retain_factory_branch_anchor_as_history();
    branch
}

/// Whether `value` reads as a task id: a letter first, a `-`, then more
/// letters, digits, `.`, `_` or `-` (for example `cas-73b8` or `cas-73b8.1`).
/// Mirrors the pattern the worker git hooks accept.
pub fn is_task_id_like(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && value
            .split_once('-')
            .is_some_and(|(_, rest)| rest.starts_with(|c: char| c.is_ascii_alphanumeric()))
}

/// The task id of `branch` when it is `worker_name`'s own per-task branch
/// (`factory/<worker>-<task-id>`), else `None`.
pub fn own_task_branch_task_id<'a>(worker_name: &str, branch: &'a str) -> Option<&'a str> {
    let worker_name = worker_name.trim();
    if worker_name.is_empty() {
        return None;
    }
    branch
        .trim()
        .strip_prefix("factory/")?
        .strip_prefix(worker_name)?
        .strip_prefix('-')
        .filter(|task_id| is_task_id_like(task_id))
}

/// What a branch is to `actor` (cas-ea31): the one ownership resolver.
///
/// Allow/deny ([`BranchOwnership::is_own`]) comes from the branch syntax and
/// the actor's own name alone, so the Rust guards, the shell Git hooks and a
/// store-less session all agree. A branch's owner is never inferred from its
/// name: `registered` (factory agent names) only names who owns a branch that
/// is not the actor's. Registration keeps factory names prefix-free (N1), so
/// the registry can never turn an own branch into a sibling's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchOwnership {
    /// `factory/<actor>`.
    Canonical,
    /// `factory/<actor>-<task-ref>`.
    OwnTask { task_ref: String },
    /// `factory/<actor>-<suffix>` whose suffix is not task-shaped.
    OwnUnrecognized { suffix: String },
    /// A registered agent's namespace.
    Sibling { owner: String },
    /// `factory/<name>` that no registered agent owns.
    Unclaimed { name: String },
    Protected,
    Other,
    Detached,
}

impl BranchOwnership {
    pub fn is_own(&self) -> bool {
        matches!(self, Self::Canonical | Self::OwnTask { .. })
    }
}

pub fn branch_ownership(actor: &str, branch: Option<&str>, registered: &[String]) -> BranchOwnership {
    let actor = actor.trim();
    let Some(branch) = branch.map(str::trim).filter(|branch| !branch.is_empty()) else {
        return BranchOwnership::Detached;
    };
    let Some(name) = branch.strip_prefix("factory/").filter(|name| !name.is_empty()) else {
        return if matches!(branch, "main" | "master" | "staging") {
            BranchOwnership::Protected
        } else {
            BranchOwnership::Other
        };
    };
    let own = (!actor.is_empty()).then(|| {
        if name == actor {
            return Some(BranchOwnership::Canonical);
        }
        let suffix = name.strip_prefix(actor)?.strip_prefix('-')?;
        Some(if is_task_id_like(suffix) {
            BranchOwnership::OwnTask { task_ref: suffix.to_string() }
        } else {
            BranchOwnership::OwnUnrecognized { suffix: suffix.to_string() }
        })
    }).flatten();
    if let Some(own) = own.as_ref().filter(|own| own.is_own()) {
        return own.clone();
    }
    let owner = registered.iter().map(|owner| owner.trim())
        .filter(|owner| !owner.is_empty() && *owner != actor)
        .filter(|owner| name == *owner
            || name.strip_prefix(owner).is_some_and(|rest| rest.starts_with('-')))
        .max_by_key(|owner| owner.len());
    match (owner, own) {
        (Some(owner), _) => BranchOwnership::Sibling { owner: owner.to_string() },
        (None, Some(own)) => own,
        (None, None) => BranchOwnership::Unclaimed { name: name.to_string() },
    }
}

/// Factory agent names in the store at `cas_root`, for naming owners only.
pub fn registered_factory_names(cas_root: &Path) -> Vec<String> {
    let Ok(store) = crate::store::open_agent_store(cas_root) else { return Vec::new() };
    let Ok(agents) = store.list(None) else { return Vec::new() };
    agents.into_iter()
        .filter(|agent| matches!(agent.role, cas_types::AgentRole::Worker | cas_types::AgentRole::Supervisor))
        .map(|agent| agent.name)
        .collect()
}

/// cas-dc08 (N1 at spawn, design v3 row 23): why `name` cannot become a new
/// worker, else `None`. Registration refuses an overlap with this machine's
/// workers; `branches` (the names under `factory/` in local and origin refs)
/// carry names from every machine. `is_task` says whether an id is a known
/// task. A new name is refused when:
/// - it extends, or is extended by, a registered worker name with `-`;
/// - a branch `factory/<m>` exists and the name extends `m` (a: `m` already
///   owns that namespace);
/// - `factory/<name>` does not exist but `factory/<name>-<s>` does and `s`
///   does not start with a known task id (b: someone else's branch the new
///   name would own by syntax).
pub fn worker_name_overlap(
    name: &str,
    workers: &[String],
    branches: &std::collections::HashSet<String>,
    is_task: &dyn Fn(&str) -> bool,
) -> Option<String> {
    use cas_store::factory_name_extends as extends;
    let name = name.trim();
    let refusal = |what: String| {
        format!(
            "WORKER_NAME_OVERLAPS: '{name}' would share the branch namespace of {what}. \
             Choose a name that does not extend, and is not extended by, another worker \
             name or factory branch with a hyphen, or omit the name to get a generated one."
        )
    };
    let hyphen_prefixes = |value: &str| -> Vec<String> {
        value.match_indices('-').map(|(at, _)| value[..at].to_string()).collect()
    };
    if let Some(other) = workers.iter().map(|other| other.trim())
        .find(|other| extends(name, other) || extends(other, name))
    {
        return Some(refusal(format!("registered worker '{other}'")));
    }
    if let Some(owner) = hyphen_prefixes(name).into_iter().find(|owner| branches.contains(owner)) {
        return Some(refusal(format!("branch factory/{owner}")));
    }
    if branches.contains(name) {
        return None;
    }
    let starts_with_task = |rest: &str| {
        hyphen_prefixes(rest).iter().map(String::as_str).chain([rest])
            .any(|id| is_task_id_like(id) && is_task(id))
    };
    branches.iter()
        .filter(|branch| extends(branch, name) && !starts_with_task(&branch[name.len() + 1..]))
        .min()
        .map(|branch| refusal(format!("branch factory/{branch}")))
}

/// N1 naming inputs, read once (cas-dc08): this machine's worker names, the
/// `factory/*` branches in local and origin refs (name to full ref; never the
/// network) and the task ids those branch names carry that the task store
/// knows. Everything is read in [`WorkerNameRegistry::load`]; the checks hold
/// no store, so they are safe to run anywhere.
#[derive(Default)]
pub struct WorkerNameRegistry {
    pub workers: Vec<String>,
    pub refs: std::collections::HashMap<String, String>,
    branches: std::collections::HashSet<String>,
    known_tasks: std::collections::HashSet<String>,
}

/// Every task-id-shaped piece of `branch` that the overlap rule can ask about:
/// each hyphen-delimited stretch that starts after a hyphen and either ends at
/// a hyphen or at the end of the name.
fn task_id_candidates(branch: &str, into: &mut std::collections::HashSet<String>) {
    for (at, _) in branch.match_indices('-') {
        let rest = &branch[at + 1..];
        let ends = rest.match_indices('-').map(|(end, _)| end).chain([rest.len()]);
        into.extend(ends.map(|end| &rest[..end]).filter(|id| is_task_id_like(id)).map(str::to_string));
    }
}

impl WorkerNameRegistry {
    pub fn load(cas_root: &Path, repo: &Path) -> Result<Self, String> {
        use std::process::Command;
        use std::time::Duration;
        let mut command = Command::new("git");
        command.current_dir(repo).args([
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/factory/",
            "refs/remotes/origin/factory/",
        ]);
        let budget = Duration::from_secs(5);
        let output = crate::bounded_process::run_command(
            &mut command,
            crate::bounded_process::Deadline::after(budget),
            budget,
        )
        .map_err(|error| format!("could not inspect worker branch names in {}: {error:?}", repo.display()))?;
        // A project that is not a repository (a `--no-worktrees` fleet) has no
        // factory branches; any other Git failure leaves the names unchecked.
        let not_a_repository = !output.status.success()
            && String::from_utf8_lossy(&output.stderr).contains("not a git repository");
        if !output.status.success() && !not_a_repository {
            return Err(format!(
                "could not inspect worker branch names in {}: {}",
                repo.display(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let refs: std::collections::HashMap<String, String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|reference| {
                reference
                    .strip_prefix("refs/heads/factory/")
                    .or_else(|| reference.strip_prefix("refs/remotes/origin/factory/"))
                    .map(|name| (name.to_string(), reference.to_string()))
            })
            .collect();
        let agents = crate::store::open_agent_store(cas_root)
            .and_then(|store| store.list(None).map_err(Into::into))
            .map_err(|error| error.to_string())?;
        // Known task membership is resolved here, once, so the overlap checks
        // that run on the daemon loop never touch the store.
        let mut candidates = std::collections::HashSet::new();
        refs.keys().for_each(|branch| task_id_candidates(branch, &mut candidates));
        let known_tasks: std::collections::HashSet<String> = crate::store::open_task_store(cas_root)
            .map(|tasks| candidates.into_iter().filter(|id| tasks.get(id).is_ok()).collect())
            .unwrap_or_default();
        Ok(Self {
            workers: agents.into_iter()
                .filter(|agent| agent.role == cas_types::AgentRole::Worker)
                .map(|agent| agent.name)
                .collect(),
            branches: refs.keys().cloned().collect(),
            refs,
            known_tasks,
        })
    }

    /// [`worker_name_overlap`] against this registry.
    pub fn overlap(&self, name: &str) -> Option<String> {
        worker_name_overlap(name, &self.workers, &self.branches, &|id| self.is_task(id))
    }

    /// [`worker_name_audit`] of this registry's workers.
    pub fn audit(&self) -> Vec<String> {
        worker_name_audit(&self.workers, &self.branches, &|id| self.is_task(id))
    }

    fn is_task(&self, id: &str) -> bool {
        self.known_tasks.contains(id)
    }
}

/// cas-dc08 (design v3 row 26): registered worker names that break N1, for
/// `cas doctor`. Each pair is reported once. Report only.
pub fn worker_name_audit(
    workers: &[String],
    branches: &std::collections::HashSet<String>,
    is_task: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    workers.iter().enumerate()
        .filter_map(|(at, name)| worker_name_overlap(name, &workers[at + 1..], branches, is_task))
        .collect()
}

/// The registry of the checkout at `path`, never an ambient `CAS_ROOT`.
fn registered_factory_names_at(path: &Path) -> Vec<String> {
    crate::store::find_cas_root_ignoring_env(path)
        .map(|root| registered_factory_names(&root))
        .unwrap_or_default()
}

/// Where an agent's ambient git state sits relative to the branch its
/// registered identity owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerBinding {
    /// HEAD is the worker's own `factory/<name>` branch — correctly bound.
    Own,
    /// HEAD is a registered agent's factory branch.
    Sibling { owner: String },
    /// HEAD is a factory branch that is neither this worker's nor a
    /// registered agent's (an unrecognized own suffix or a retired name).
    Unowned,
    /// HEAD is a shared trunk branch, or unresolvable (detached / not a repo).
    /// The shared checkout is where every agent's `main` lives, so this is
    /// never a worker's own workspace.
    SharedTrunk,
    /// Some other named branch (`feature/*`, `epic/*`, ...). Legitimate for
    /// workers spawned without isolation; not evidence of misbinding.
    Other,
}

/// Classify `branch` against the branch `worker_name` owns.
///
/// `branch` is the resolved HEAD at the directory the worker is bound to;
/// `None` means git could not name a branch there (detached HEAD, missing
/// directory, or not a repository). `my_context` first recognizes a detached
/// HEAD in a dedicated linked worktree ([`render_worker_binding_at`]), so only
/// an unresolvable shared or non-worktree directory reaches this as trunk.
pub fn classify_worker_binding(worker_name: &str, branch: Option<&str>) -> WorkerBinding {
    binding_from(branch_ownership(worker_name, branch, &[]))
}

/// [`classify_worker_binding`] with owners named from `path`'s registry.
pub fn classify_worker_binding_at(worker_name: &str, branch: Option<&str>, path: &Path) -> WorkerBinding {
    let ownership = branch_ownership(worker_name, branch, &[]);
    if ownership.is_own() || !matches!(ownership,
        BranchOwnership::OwnUnrecognized { .. } | BranchOwnership::Unclaimed { .. }) {
        return binding_from(ownership);
    }
    binding_from(branch_ownership(worker_name, branch, &registered_factory_names_at(path)))
}

fn binding_from(ownership: BranchOwnership) -> WorkerBinding {
    match ownership {
        BranchOwnership::Canonical | BranchOwnership::OwnTask { .. } => WorkerBinding::Own,
        BranchOwnership::Sibling { owner } => WorkerBinding::Sibling { owner },
        BranchOwnership::OwnUnrecognized { .. } | BranchOwnership::Unclaimed { .. } => WorkerBinding::Unowned,
        BranchOwnership::Protected | BranchOwnership::Detached => WorkerBinding::SharedTrunk,
        BranchOwnership::Other => WorkerBinding::Other,
    }
}

/// Resolve HEAD at `path` as a branch name.
///
/// Returns `None` for a detached HEAD, a missing directory, or a path that is
/// not inside a git repository. Note that for a plain directory *inside* a
/// repository git answers with the enclosing checkout's HEAD — which is
/// exactly the misbinding [`worktree_root_at`] exists to catch.
pub fn branch_at(path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty()).then_some(branch)
}

/// The working-tree root that git resolves for `path`.
///
/// For a real worktree this is the worktree itself. For a stale plain
/// directory it is the *enclosing* checkout — the signal that the directory is
/// not a workspace of its own.
pub fn worktree_root_at(path: &Path) -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!root.is_empty()).then(|| std::path::PathBuf::from(root))
}

/// Compare two paths after resolving symlinks, falling back to a literal
/// comparison when a path cannot be canonicalized.
fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Fail-closed proof that `path` is the worktree `worker_name` owns.
///
/// Verifies both halves of the binding, because either alone is forgeable:
/// the working-tree root must *be* `path` (a stale plain directory resolves to
/// the enclosing shared checkout instead), and HEAD there must be the worker's
/// own `factory/<name>` branch (a real worktree can still belong to a sibling).
///
/// `expected_branch` is the branch provisioning intends to bind, defaulting to
/// [`expected_worker_branch`] when the caller has none to hand.
///
/// Callers must treat an error as a spawn failure: a worker that cannot be
/// proven to own its directory must never be handed to a harness.
pub fn verify_worker_worktree_binding(
    worker_name: &str,
    path: &Path,
    expected_branch: Option<&str>,
) -> anyhow::Result<()> {
    let expected_branch = expected_branch
        .map(str::to_string)
        .unwrap_or_else(|| expected_worker_branch(worker_name));

    let Some(root) = worktree_root_at(path) else {
        anyhow::bail!(
            "Worker '{worker_name}': {} is not inside a git repository, so it cannot be that \
             worker's worktree on '{expected_branch}'. Remove the path and retry the spawn.",
            path.display(),
        );
    };
    if !same_path(&root, path) {
        anyhow::bail!(
            "ISOLATION BUG: worker '{worker_name}' would be bound to {}, which is not a git \
             worktree root — git resolves it to the shared checkout at {}. Every commit made \
             there would land in that shared checkout, not on '{expected_branch}'. Remove {} and \
             retry the spawn.",
            path.display(),
            root.display(),
            path.display(),
        );
    }

    let branch = branch_at(path);
    if branch.as_deref().map(str::trim) == Some(expected_branch.as_str()) {
        return Ok(());
    }
    // cas-73b8: a worktree left on the worker's own per-task branch is still
    // that worker's worktree.
    if branch
        .as_deref()
        .is_some_and(|branch| own_task_branch_task_id(worker_name, branch).is_some())
    {
        return Ok(());
    }
    match classify_worker_binding_at(worker_name, branch.as_deref(), path) {
        // Reachable only when the caller's expected branch disagrees with the
        // worker's canonical one; refuse rather than pick a winner.
        WorkerBinding::Own => anyhow::bail!(
            "ISOLATION BUG: worker '{worker_name}' would be bound to {}, checked out on '{}', but \
             provisioning intended '{expected_branch}'.",
            path.display(),
            branch.as_deref().unwrap_or("<unknown>"),
        ),
        WorkerBinding::Sibling { owner } => anyhow::bail!(
            "ISOLATION BUG: worker '{worker_name}' would be bound to {}, which is checked out on \
             '{}' — that worktree belongs to worker '{owner}', not to '{worker_name}'. \
             '{worker_name}' owns '{expected_branch}'.",
            path.display(),
            branch.as_deref().unwrap_or("<unknown>"),
        ),
        WorkerBinding::Unowned | WorkerBinding::SharedTrunk | WorkerBinding::Other => anyhow::bail!(
            "ISOLATION BUG: worker '{worker_name}' would be bound to {}, which is checked out on \
             '{}' instead of its own '{expected_branch}'.",
            path.display(),
            branch.as_deref().unwrap_or("<detached HEAD>"),
        ),
    }
}

/// The refusal shown when a worker acts from a sibling worker's branch.
///
/// Shared by the PreToolUse commit guard and `my_context` so both name the
/// same canonical binding.
pub fn sibling_misbinding_message(
    worker_name: &str,
    owner: &str,
    branch: &str,
    cwd: &str,
    recovery: Option<&TakeoverPatch>,
) -> String {
    let expected_branch = expected_worker_branch(worker_name);
    // cas-ea31 (takeover case 1): in the worker's own registered checkout the
    // way out is to keep the edits and switch back, not to move directories.
    if let Some(patch) = recovery {
        return format!(
            "🚫 WORKER ISOLATION MISBINDING: you are '{worker_name}', but your checkout {cwd} is \
             on '{branch}', which belongs to worker '{owner}'. Committing here would put your work \
             on that worker's branch.\n\n{}",
            recovery_steps(worker_name, patch),
        );
    }
    format!(
        "🚫 WORKER ISOLATION MISBINDING: you are '{worker_name}', but {cwd} is checked out on \
         '{branch}' — that is worker '{owner}''s worktree.\n\n\
         Committing here would put your work on another worker's branch, where its owner and the \
         supervisor's merge sequence will not expect it.\n\n\
         You own '{expected_branch}'. Move to your own worktree before committing:\n  \
         cd \"$CAS_CLONE_PATH\"   # your assigned worktree\n  \
         git rev-parse --abbrev-ref HEAD   # must print {expected_branch}\n\n\
         If your assigned worktree is not on {expected_branch}, stop and report the misbinding to \
         your supervisor — do not commit from another worker's tree.\n\n\
         For an authorized takeover, ask your supervisor to run `task action=transfer id=<task-id> \
         to_agent={worker_name} adopt_branch=true supervisor_override=true` from its own context. \
         This copies the task's recorded tip to your own per-task branch in your registered \
         worktree, including when it is on that task's source branch; no worker checkout is \
         needed first. Then commit on the adopted branch.\n\n\
         Note: --no-verify does NOT bypass this guard (it only skips git hooks, not the Claude \
         Code PreToolUse harness)."
    )
}

/// Where a refusal tells the worker to save its edits: shell-quoted directory
/// and file, both already safe to paste.
pub struct TakeoverPatch {
    pub dir: String,
    pub file: String,
}

/// The recovery H-22 prints in the worker's own registered checkout. Every
/// command is one H-22 admits: save the edits, clean the tree, switch back,
/// then optionally bring the edits along.
fn recovery_steps(worker_name: &str, patch: &TakeoverPatch) -> String {
    let expected_branch = expected_worker_branch(worker_name);
    format!(
        "Keep your edits, clean the tree, then switch back to your own branch:\n  \
         mkdir -p {dir}\n  \
         git diff --binary HEAD > {file}\n  \
         git restore --staged --worktree .\n  \
         git switch {expected_branch}\n\n\
         To bring the edits along, apply them on your branch:\n  \
         git apply {file}\n\n\
         To take over another worker's delivery, ask your supervisor to run `task action=transfer \
         id=<task-id> to_agent={worker_name} adopt_branch=true supervisor_override=true`; it copies \
         the delivery to your own per-task branch.\n\n\
         Note: --no-verify does NOT bypass this guard (it only skips git hooks, not the Claude \
         Code PreToolUse harness).",
        dir = patch.dir,
        file = patch.file,
    )
}

/// H-22 refusal for a factory branch no registered agent owns (cas-ea31,
/// cas-7e76a): no owner is invented from the branch name.
pub fn branch_not_owned_message(worker_name: &str, branch: &str, patch: &TakeoverPatch) -> String {
    let expected_branch = expected_worker_branch(worker_name);
    format!(
        "WORKER_BRANCH_NOT_OWNED: 🚫 '{branch}' is not one of your branches, and no registered \
         worker owns it. Yours are '{expected_branch}' and '{expected_branch}-<task-id>'.\n\n{}",
        recovery_steps(worker_name, patch),
    )
}

/// Render the worktree-binding section of `coordination my_context` for the
/// directory this session is bound to, resolving HEAD at that directory.
pub fn render_worker_binding_at(worker_name: &str, is_worker: bool, path: Option<&Path>) -> String {
    let branch = path.and_then(branch_at);
    let location = path.map(|path| path.display().to_string());
    // cas-f0f3: independent QA runs `git switch --detach <bound_head>` in its
    // own worktree. A missing branch there is not the shared checkout.
    if let (Some(path), Some(location), None) = (path, location.as_deref(), branch.as_deref())
        && is_worker
        && detached_linked_worktree_at(path)
    {
        return format!(
            "**Clone Path**: {location}\n**Git Branch**: (detached HEAD)\n\
             **Worktree Binding**: OK — detached HEAD in a dedicated linked worktree, not the \
             shared checkout\n"
        );
    }
    render_worker_binding(worker_name, is_worker, location.as_deref(), branch.as_deref())
}

/// Whether `path` is the root of a linked worktree (its own HEAD and index,
/// separate from the shared main checkout) with HEAD detached. A plain
/// directory inside a checkout resolves to the enclosing root and the main
/// checkout's git directory is the common directory, so neither qualifies.
pub fn detached_linked_worktree_at(path: &Path) -> bool {
    if branch_at(path).is_some()
        || !worktree_root_at(path).is_some_and(|root| same_path(&root, path))
    {
        return false;
    }
    let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir", "--verify", "HEAD"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines().map(str::trim);
    match (lines.next(), lines.next(), lines.next()) {
        (Some(git_dir), Some(common_dir), Some(head)) if !head.is_empty() => {
            !same_path(Path::new(git_dir), Path::new(common_dir))
        }
        _ => false,
    }
}

/// Render the worktree-binding section of `coordination my_context`.
///
/// `clone_path` is the directory the harness was launched in
/// (`CAS_CLONE_PATH`); `branch` is HEAD resolved *at that directory* rather
/// than wherever the MCP server process happens to be running. A worker whose
/// binding does not match its registered identity gets an explicit misbinding
/// block instead of a branch line that looks fine.
pub fn render_worker_binding(
    worker_name: &str,
    is_worker: bool,
    clone_path: Option<&str>,
    branch: Option<&str>,
) -> String {
    let mut output = String::new();
    if let Some(path) = clone_path {
        output.push_str(&format!("**Clone Path**: {path}\n"));
    }
    if let Some(branch) = branch {
        output.push_str(&format!("**Git Branch**: {branch}\n"));
    } else {
        output.push_str("**Git Branch**: (unresolved — detached HEAD or not a git repository)\n");
    }
    if !is_worker {
        return output;
    }

    let expected_branch = expected_worker_branch(worker_name);
    let location = clone_path.unwrap_or("this session's working directory");
    let binding = match clone_path {
        Some(path) => classify_worker_binding_at(worker_name, branch, Path::new(path)),
        None => classify_worker_binding(worker_name, branch),
    };
    match binding {
        WorkerBinding::Own => {
            output.push_str(&format!(
                "**Worktree Binding**: OK — own branch {expected_branch}\n"
            ));
        }
        WorkerBinding::Sibling { owner } => {
            output.push_str(&format!(
                "\n🚫 **ISOLATION MISBINDING**: {location} is checked out on '{}', \
                 which belongs to worker '{owner}', not to you ('{worker_name}'). You own \
                 '{expected_branch}'. Do not commit from here — report this to your supervisor \
                 and have your worker respawned onto its own worktree.\n",
                branch.unwrap_or("<unknown>"),
            ));
        }
        WorkerBinding::Unowned => {
            output.push_str(&format!(
                "\n🚫 **BRANCH NOT OWNED**: {location} is checked out on '{}', which is not one \
                 of your branches and no registered worker owns. You own '{expected_branch}'. \
                 Switch back before committing: git switch {expected_branch}\n",
                branch.unwrap_or("<unknown>"),
            ));
        }
        WorkerBinding::SharedTrunk => {
            output.push_str(&format!(
                "\n🚫 **ISOLATION MISBINDING**: {location} resolves to a shared trunk checkout \
                 ({}), not to your own worktree on '{expected_branch}'. Work done here lands in \
                 the checkout every other agent shares. Report this to your supervisor and have \
                 your worker respawned onto its own worktree.\n",
                branch.unwrap_or("detached HEAD"),
            ));
        }
        WorkerBinding::Other => {
            output.push_str(&format!(
                "**Worktree Binding**: not isolated — on '{}' rather than '{expected_branch}'\n",
                branch.unwrap_or("<unknown>"),
            ));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo(dir: &Path) {
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "test@cas.test"],
            vec!["config", "user.name", "Cassy Test"],
        ] {
            Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .expect("git setup");
        }
        std::fs::write(dir.join("README.md"), "# test").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "init"]] {
            Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .expect("git commit");
        }
    }

    #[test]
    fn own_factory_branch_is_the_only_correct_binding() {
        assert_eq!(
            classify_worker_binding("fair-pelican-51", Some("factory/fair-pelican-51")),
            WorkerBinding::Own
        );
        // Without a registry no owner is invented from the branch name.
        assert_eq!(
            classify_worker_binding("fair-pelican-51", Some("factory/bright-dolphin-92")),
            WorkerBinding::Unowned
        );
        assert_eq!(
            branch_ownership("fair-pelican-51", Some("factory/bright-dolphin-92-cas-1"), &["bright-dolphin-92".into()]),
            BranchOwnership::Sibling { owner: "bright-dolphin-92".into() }
        );
        for trunk in ["main", "master", "staging"] {
            assert_eq!(
                classify_worker_binding("fair-pelican-51", Some(trunk)),
                WorkerBinding::SharedTrunk,
                "{trunk} is a shared checkout, never a worker's own workspace"
            );
        }
        // Detached / unresolvable HEAD is treated as shared, not as "fine".
        assert_eq!(
            classify_worker_binding("fair-pelican-51", None),
            WorkerBinding::SharedTrunk
        );
        // Non-trunk named branches remain legitimate for non-isolated workers.
        assert_eq!(
            classify_worker_binding("fair-pelican-51", Some("epic/cas-1276")),
            WorkerBinding::Other
        );
    }

    /// The stale-directory shape: git answers the enclosing checkout's HEAD,
    /// so only the working-tree root distinguishes it from a real worktree.
    #[test]
    fn stale_directory_inside_the_shared_checkout_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        Command::new("git")
            .args(["switch", "-c", "factory/zen-bear-56"])
            .current_dir(&repo)
            .output()
            .expect("park shared HEAD on the worker's branch");

        let stale = repo.join(".cas").join("worktrees").join("zen-bear-56");
        std::fs::create_dir_all(&stale).unwrap();

        // The branch alone looks correct — that is the trap.
        assert_eq!(
            branch_at(&stale).as_deref(),
            Some("factory/zen-bear-56"),
            "git climbs out of a plain directory into the enclosing checkout"
        );
        let error = verify_worker_worktree_binding("zen-bear-56", &stale, None)
            .expect_err("a plain directory is not a worktree")
            .to_string();

        assert!(error.contains(&repo.display().to_string()), "{error}");
    }

    #[test]
    fn sibling_worktree_is_rejected_and_names_its_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let sibling = tmp.path().join("bright-dolphin-92");
        Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                "factory/bright-dolphin-92",
                sibling.to_str().unwrap(),
                "main",
            ])
            .current_dir(&repo)
            .output()
            .expect("git worktree add");

        let error = verify_worker_worktree_binding("fair-pelican-51", &sibling, None)
            .expect_err("a sibling's worktree is not this worker's binding")
            .to_string();
        assert!(error.contains("bright-dolphin-92"), "{error}");
        assert!(error.contains("fair-pelican-51"), "{error}");
    }

    #[test]
    fn own_worktree_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let own = tmp.path().join("own-worker");
        Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                "factory/own-worker",
                own.to_str().unwrap(),
                "main",
            ])
            .current_dir(&repo)
            .output()
            .expect("git worktree add");

        verify_worker_worktree_binding("own-worker", &own, Some("factory/own-worker"))
            .expect("own worktree must verify");
    }

    #[test]
    fn my_context_reports_a_sibling_binding_as_a_misbinding() {
        let rendered = render_worker_binding(
            "fair-pelican-51",
            true,
            Some("/repo/.cas/worktrees/bright-dolphin-92"),
            Some("factory/bright-dolphin-92"),
        );

        assert!(rendered.contains("bright-dolphin-92"), "{rendered}");
        assert!(rendered.contains("factory/fair-pelican-51"), "{rendered}");
    }

    #[test]
    fn my_context_reports_a_shared_trunk_binding_as_a_misbinding() {
        let rendered = render_worker_binding("zen-bear-56", true, Some("/repo"), Some("main"));

        assert!(rendered.contains("factory/zen-bear-56"), "{rendered}");
    }

    /// cas-f0f3: independent QA detaches its own linked worktree at the bound
    /// SHA. Only a detached shared checkout, or a plain directory inside it,
    /// is a trunk misbinding.
    #[test]
    fn my_context_accepts_a_detached_own_linked_worktree_cas_f0f3() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        let git = |dir: &Path, args: &[&str]| {
            let output = Command::new("git").args(args).current_dir(dir).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        let qa = tmp.path().join("subtle-pelican-6");
        git(&repo, &["worktree", "add", "--detach", qa.to_str().unwrap(), "main"]);
        assert!(detached_linked_worktree_at(&qa));

        git(&repo, &["switch", "-q", "--detach", "main"]);
        let stale = repo.join(".cas").join("worktrees").join("subtle-pelican-6");
        std::fs::create_dir_all(&stale).unwrap();
        for shared in [&repo, &stale] {
            assert!(!detached_linked_worktree_at(shared));
        }
    }


    /// Supervisors legitimately sit on trunk in the shared checkout; the
    /// worker-identity check must not fire for them.

    /// cas-ea31: the Git hooks and the Rust resolver must admit exactly the
    /// same task-branch suffixes; the hooks run with no registry or binary.
    #[test]
    fn hook_task_branch_predicate_matches_rust_grammar_cas_ea31() {
        use crate::ui::factory::daemon::runtime::teams::TeamsManager;
        let suffixes = [
            "cas-ea31", "cas-73b8.1", "cas-ef33-diag", "release-notes", "staging",
            "e9e5", "2", "2-cas-x", "cas--x", "a-.b-c", "", "x/y-z",
        ];
        for template in [TeamsManager::WORKER_PRE_COMMIT_HOOK, TeamsManager::WORKER_PRE_PUSH_HOOK] {
            let start = template.find("own_task_branch() {").expect("hook defines own_task_branch");
            let end = start + template[start..].find("\n}\n").expect("function end") + 3;
            let script = format!(
                "expected=factory/kevin\n{}for b in \"$@\"; do if own_task_branch \"$b\"; then echo 1; else echo 0; fi; done\n",
                &template[start..end],
            );
            let branches: Vec<String> = suffixes.iter().map(|suffix| format!("factory/kevin-{suffix}")).collect();
            let output = std::process::Command::new("sh").arg("-c").arg(&script).arg("sh")
                .args(&branches).output().expect("run hook predicate");
            assert!(output.status.success(), "{output:?}");
            let shell: Vec<bool> = String::from_utf8_lossy(&output.stdout).lines().map(|line| line == "1").collect();
            let rust: Vec<bool> = branches.iter()
                .map(|branch| own_task_branch_task_id("kevin", branch).is_some()).collect();
            assert_eq!(shell, rust, "{suffixes:?}");
        }
    }

    /// cas-73b8: a worker whose `factory/<name>` is frozen for a parked
    /// delivery commits its next task on `factory/<name>-<task-id>`.
    #[test]
    fn own_per_task_branch_is_the_workers_own() {
        assert_eq!(
            worker_task_branch("loyal-crow-4", "cas-1185"),
            "factory/loyal-crow-4-cas-1185"
        );
        assert_eq!(
            own_task_branch_task_id("loyal-crow-4", "factory/loyal-crow-4-cas-1185"),
            Some("cas-1185")
        );
        assert_eq!(
            classify_worker_binding("loyal-crow-4", Some("factory/loyal-crow-4-cas-1185")),
            WorkerBinding::Own
        );
        // A sibling whose name only shares a prefix is not this worker's.
        assert_eq!(
            own_task_branch_task_id("loyal-crow-4", "factory/loyal-crow-40"),
            None
        );
        // cas-ea31: without a `-` boundary the longer worker's task branch
        // is its own under both identities. With one (`ab` / `ab-owner`) the
        // shorter name would parse it too, which is why registration
        // refuses that pair.
        let longer = "factory/eager-panda-27-cas-1234";
        assert_eq!(own_task_branch_task_id("eager-panda-27", longer), Some("cas-1234"));
        assert_eq!(own_task_branch_task_id("eager-panda-2", longer), None);
        assert!(own_task_branch_task_id("ab", "factory/ab-owner-cas-1234").is_some());
        assert_eq!(
            classify_worker_binding("loyal-crow-4", Some("factory/loyal-crow-40")),
            WorkerBinding::Unowned
        );
        // Another worker's per-task branch is never this worker's.
        assert_eq!(
            classify_worker_binding("loyal-crow-4", Some("factory/nimble-wolf-5-cas-1185")),
            WorkerBinding::Unowned
        );
        // The suffix must read as a task id.
        for branch in [
            "factory/loyal-crow-4-",
            "factory/loyal-crow-4-cas",
            "factory/loyal-crow-4-cas-",
            "factory/loyal-crow-4-1185",
            "factory/loyal-crow-4-cas-11 85",
            "factory/loyal-crow-4-cas-1185/x",
        ] {
            assert_eq!(
                own_task_branch_task_id("loyal-crow-4", branch),
                None,
                "{branch}"
            );
        }
        assert!(is_task_id_like("cas-73b8.1"));
        assert_eq!(own_task_branch_task_id("", "factory/-cas-1"), None);
    }

    #[test]
    fn a_worktree_left_on_the_own_task_branch_still_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        Command::new("git")
            .args(["checkout", "-q", "-b", "factory/own-worker-cas-1234"])
            .current_dir(repo)
            .output()
            .unwrap();
        verify_worker_worktree_binding("own-worker", repo, None)
            .expect("the worker's own per-task branch is its binding");
        assert!(verify_worker_worktree_binding("other-worker", repo, None).is_err());
    }
}
