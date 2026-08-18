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
/// directory, or not a repository).
pub fn classify_worker_binding(worker_name: &str, branch: Option<&str>) -> WorkerBinding {
    let worker_name = worker_name.trim();
    let branch = branch.map(str::trim).unwrap_or("");
    if branch.is_empty() {
        return WorkerBinding::SharedTrunk;
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
         Note: --no-verify does NOT bypass this guard (it only skips git hooks, not the Claude \
         Code PreToolUse harness)."
    )
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
        assert!(error.contains("not a git worktree root"), "{error}");
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
        assert!(rendered.contains("ISOLATION MISBINDING"), "{rendered}");
        assert!(rendered.contains("bright-dolphin-92"), "{rendered}");
        assert!(rendered.contains("factory/fair-pelican-51"), "{rendered}");
    }

    #[test]
    fn my_context_reports_a_shared_trunk_binding_as_a_misbinding() {
        let rendered = render_worker_binding("zen-bear-56", true, Some("/repo"), Some("main"));
        assert!(rendered.contains("ISOLATION MISBINDING"), "{rendered}");
        assert!(rendered.contains("shared trunk"), "{rendered}");
        assert!(rendered.contains("factory/zen-bear-56"), "{rendered}");
    }

    #[test]
    fn my_context_confirms_a_correct_binding_without_alarm() {
        let rendered = render_worker_binding(
            "own-worker",
            true,
            Some("/repo/.cas/worktrees/own-worker"),
            Some("factory/own-worker"),
        );
        assert!(rendered.contains("**Worktree Binding**: OK"), "{rendered}");
        assert!(!rendered.contains("MISBINDING"), "{rendered}");
    }

    /// Supervisors legitimately sit on trunk in the shared checkout; the
    /// worker-identity check must not fire for them.
    #[test]
    fn non_workers_get_the_plain_branch_line() {
        let rendered = render_worker_binding("wise-viper-85", false, Some("/repo"), Some("main"));
        assert!(rendered.contains("**Git Branch**: main"), "{rendered}");
        assert!(!rendered.contains("MISBINDING"), "{rendered}");
    }
}
