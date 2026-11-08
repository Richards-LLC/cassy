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

/// The worker named by a `factory/<name>` branch, if it is one.
pub fn factory_branch_owner(branch: &str) -> Option<&str> {
    branch
        .trim()
        .strip_prefix("factory/")
        .filter(|owner| !owner.is_empty())
}

/// Where an agent's ambient git state sits relative to the branch its
/// registered identity owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerBinding {
    /// HEAD is the worker's own `factory/<name>` branch — correctly bound.
    Own,
    /// HEAD is another worker's factory branch.
    Sibling { owner: String },
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
    let worker_name = worker_name.trim();
    let branch = branch.map(str::trim).unwrap_or("");
    if branch.is_empty() {
        return WorkerBinding::SharedTrunk;
    }
    // cas-73b8: the worker's own per-task branch is as much its own as
    // `factory/<name>`; it is what the worker commits on while that branch is
    // frozen for a parked delivery under QA.
    if own_task_branch_task_id(worker_name, branch).is_some() {
        return WorkerBinding::Own;
    }
    match factory_branch_owner(branch) {
        Some(owner) if owner == worker_name => WorkerBinding::Own,
        Some(owner) => WorkerBinding::Sibling {
            owner: owner.to_string(),
        },
        None if matches!(branch, "main" | "master" | "staging") => WorkerBinding::SharedTrunk,
        None => WorkerBinding::Other,
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
    match classify_worker_binding(worker_name, branch.as_deref()) {
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
        WorkerBinding::SharedTrunk | WorkerBinding::Other => anyhow::bail!(
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
pub fn sibling_misbinding_message(worker_name: &str, owner: &str, cwd: &str) -> String {
    let expected_branch = expected_worker_branch(worker_name);
    format!(
        "🚫 WORKER ISOLATION MISBINDING: you are '{worker_name}', but {cwd} is checked out on \
         'factory/{owner}' — that is worker '{owner}''s worktree.\n\n\
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
    match classify_worker_binding(worker_name, branch) {
        WorkerBinding::Own => {
            output.push_str(&format!(
                "**Worktree Binding**: OK — own branch {expected_branch}\n"
            ));
        }
        WorkerBinding::Sibling { owner } => {
            output.push_str(&format!(
                "\n🚫 **ISOLATION MISBINDING**: {location} is checked out on 'factory/{owner}', \
                 which belongs to worker '{owner}', not to you ('{worker_name}'). You own \
                 '{expected_branch}'. Do not commit from here — report this to your supervisor \
                 and have your worker respawned onto its own worktree.\n"
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
        assert_eq!(
            classify_worker_binding("fair-pelican-51", Some("factory/bright-dolphin-92")),
            WorkerBinding::Sibling {
                owner: "bright-dolphin-92".to_string()
            }
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

    #[test]
    fn factory_branch_owner_ignores_non_factory_refs() {
        assert_eq!(
            factory_branch_owner("factory/zen-bear-56"),
            Some("zen-bear-56")
        );
        assert_eq!(
            factory_branch_owner("  factory/zen-bear-56 "),
            Some("zen-bear-56")
        );
        assert_eq!(factory_branch_owner("factory/"), None);
        assert_eq!(factory_branch_owner("epic/cas-1276"), None);
        assert_eq!(factory_branch_owner("main"), None);
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
        assert_eq!(
            classify_worker_binding("loyal-crow-4", Some("factory/loyal-crow-40")),
            WorkerBinding::Sibling {
                owner: "loyal-crow-40".to_string()
            }
        );
        // Another worker's per-task branch stays a sibling's.
        assert!(matches!(
            classify_worker_binding("loyal-crow-4", Some("factory/nimble-wolf-5-cas-1185")),
            WorkerBinding::Sibling { .. }
        ));
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
