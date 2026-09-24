//! QA evidence close gate (cas-0cd5).
//!
//! Runs on every delivery close, immediately before the factory merge gate.
//! A user-facing delivery can then neither park for merge (and so never gets
//! an independent reviewer spawned, cas-619f) nor finish its post-merge
//! re-close without valid implementer evidence for the delivered head. The
//! same check refuses any delivery that adds test skip/focus markers without
//! a `cas-allow-skip:` reason.
//!
//! Eligibility is `qa_pass::user_facing_reasons`, shared with cas-619f.
//! Validation lives in `crate::qa_evidence`. Design:
//! `docs/qa/evidence-close-gate.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::mcp::tools::core::imports::*;
use crate::qa_evidence::{
    EvidenceContext, EvidenceTier, SkipMarker, added_skip_markers, delivery_range,
    delivery_test_diff, range_paths, run_close_gate,
};
use crate::qa_pass::{
    catalog_journeys_for, first_user_facing_path, is_non_surface_path, user_facing_reasons,
};

use super::close_ops::resolve_branch_sha;

/// The delivered commit this close answers for.
///
/// An explicit `commit_receipt` wins: the close names the commit it delivers,
/// and the worker's branch may already hold later, unrelated work. Judging the
/// live tip instead refused a Rust-only close as "user-facing hub-web" because
/// the branch had moved on to another task's hub-web commit (cas-e4d2).
///
/// Without a receipt, for a factory worker: the `factory/<assignee>` tip while
/// it still carries unmerged work; once that tip is contained in the target,
/// the anchor the park recorded (the tip may since have moved on to the
/// worker's next task). Otherwise the repository HEAD.
pub(crate) fn delivered_head(
    task: &Task,
    repo: &Path,
    target_branch: &str,
    commit_receipt: Option<&str>,
) -> Option<String> {
    let rev = |spec: &str| resolve_branch_sha(repo, &format!("{spec}^{{commit}}"));
    if let Some(receipt) = commit_receipt
        .map(str::trim)
        .filter(|receipt| !receipt.is_empty())
        .and_then(rev)
    {
        return Some(receipt);
    }
    if let Some(assignee) = task.assignee.as_deref()
        && let Some(tip) = resolve_branch_sha(repo, &format!("factory/{assignee}"))
    {
        let merged = !target_branch.is_empty()
            && Command::new("git")
                .args(["merge-base", "--is-ancestor", &tip, target_branch])
                .current_dir(repo)
                .status()
                .is_ok_and(|status| status.success());
        if !merged {
            return Some(tip);
        }
        return task
            .deliverables
            .factory_branch_anchor
            .as_deref()
            .and_then(rev)
            .or(Some(tip));
    }
    rev("HEAD")
}

/// Which evidence the shared user-facing reasons demand. `terminal_render`
/// says the diff touches a `qa.terminal_render_paths` glob.
pub(crate) fn evidence_tier(reasons: &[String], terminal_render: bool) -> EvidenceTier {
    if reasons.is_empty() {
        EvidenceTier::None
    } else if reasons
        .iter()
        .any(|reason| reason.starts_with("journeys:") || reason.starts_with("path:"))
    {
        EvidenceTier::Bundle
    } else {
        EvidenceTier::Ledger {
            terminal_qa: terminal_render,
        }
    }
}

/// Run the gate. `Ok(notes)` carries decision-note lines to record on close;
/// `Err(message)` is the complete rejection text.
pub(crate) fn qa_evidence_close_gate(
    cas_root: &Path,
    task: &Task,
    repo: &Path,
    target_branch: &str,
    commit_receipt: Option<&str>,
) -> Result<Vec<String>, String> {
    qa_evidence_close_gate_for_paths(cas_root, task, repo, target_branch, commit_receipt, None)
}

/// The target a delivery is diffed against: `origin/<target>` when that ref
/// exists, since a worker's local target is routinely stale after a PR merge,
/// otherwise the local branch (cas-bde8, GH #978).
fn live_target_ref(repo: &Path, target_branch: &str) -> String {
    let origin = format!("origin/{target_branch}");
    if resolve_branch_sha(repo, &origin).is_some() {
        origin
    } else {
        target_branch.to_string()
    }
}

/// [`qa_evidence_close_gate`] judged on `attributed_paths`: the paths this
/// task's own delivery commits changed, as selected by the close path's task
/// attribution (cas-bde8, GH #978).
///
/// Diffing the branch from its merge-base with the target counted an earlier
/// task's commits on a reused factory branch as this task's diff. That happens
/// whenever the target has not absorbed them as ancestors: a stale local
/// target, or a squash-merged PR. A backend-only task was then asked for web
/// QA evidence for the previous task's UI. When the attribution found this
/// task's changes, only those are judged. Otherwise (no attributable commits,
/// or legacy history) the branch diff against the live target applies, as
/// before.
pub(crate) fn qa_evidence_close_gate_for_paths(
    cas_root: &Path,
    task: &Task,
    repo: &Path,
    target_branch: &str,
    commit_receipt: Option<&str>,
    attributed_paths: Option<&[String]>,
) -> Result<Vec<String>, String> {
    let Ok(config) = crate::config::Config::load(cas_root) else {
        return Ok(Vec::new());
    };
    let qa = config.qa();
    if !qa.evidence_gate
        || task.task_type == TaskType::Epic
        || task.execution_note.as_deref() == Some("no-code")
    {
        return Ok(Vec::new());
    }
    let Some(head) = delivered_head(task, repo, target_branch, commit_receipt) else {
        return Ok(Vec::new());
    };
    let range = (!target_branch.is_empty())
        .then(|| delivery_range(repo, &head, &live_target_ref(repo, target_branch)))
        .flatten();
    let changed = match attributed_paths.filter(|paths| !paths.is_empty()) {
        Some(paths) => Some(paths.to_vec()),
        None => range
            .as_ref()
            .and_then(|(from, to)| range_paths(repo, from, to)),
    };
    let journeys = changed
        .as_deref()
        .map(|paths| catalog_journeys_for(repo, paths))
        .unwrap_or_default();
    let reasons = user_facing_reasons(task, &qa, changed.as_deref(), &journeys).reasons;
    let terminal_render = changed.as_deref().is_some_and(|paths| {
        let surface: Vec<String> = paths
            .iter()
            .filter(|path| !is_non_surface_path(path))
            .cloned()
            .collect();
        first_user_facing_path(&surface, &qa.terminal_render_paths).is_some()
    });
    let markers: Vec<SkipMarker> = match (range.as_ref(), changed.as_deref()) {
        (Some((from, to)), Some(paths)) => delivery_test_diff(repo, from, to, paths)
            .map(|diff| added_skip_markers(&diff))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let artifacts_dir: PathBuf =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref())
            .join(&task.id);
    let ctx = EvidenceContext {
        task_id: &task.id,
        task_artifacts_dir: &artifacts_dir,
        repo,
        delivered_head: &head,
        notes: &task.notes,
    };
    run_close_gate(
        &ctx,
        evidence_tier(&reasons, terminal_render),
        &reasons,
        &markers,
    )
    .map(|pass| pass.notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(["-c", "user.name=QA", "-c", "user.email=qa@example.invalid"])
            .args(args)
            .current_dir(repo)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn commit_file(repo: &Path, path: &str, body: &str) -> String {
        let file = repo.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
        git(repo, &["add", path]);
        git(repo, &["commit", "-q", "-m", path]);
        git(repo, &["rev-parse", "HEAD"])
    }

    /// cas-e4d2: the worker's branch holds a later hub-web commit from its
    /// next task. A close that names its Rust-only commit is judged on that
    /// commit, not refused as user-facing because of the branch tip.
    #[test]
    fn commit_receipt_is_judged_instead_of_the_live_factory_tip() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        commit_file(&repo, "README.md", "base\n");
        git(&repo, &["switch", "-q", "-c", "factory/worker"]);
        let rust_only = commit_file(&repo, "src/lib.rs", "pub fn fixed() {}\n");
        let hub_web = commit_file(&repo, "hub-web/dist/app.css", "body { color: red }\n");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();

        let mut task = Task::new("cas-e4d2-regression".to_string(), "Rust fix".to_string());
        task.assignee = Some("worker".to_string());

        assert_eq!(
            delivered_head(&task, &repo, "main", Some(&rust_only)).as_deref(),
            Some(rust_only.as_str())
        );
        assert_eq!(
            delivered_head(&task, &repo, "main", None).as_deref(),
            Some(hub_web.as_str()),
            "without a receipt the unmerged branch tip is still the delivery"
        );

        qa_evidence_close_gate(&cas_root, &task, &repo, "main", Some(&rust_only))
            .expect("a Rust-only receipt needs no QA evidence");
        let refusal = qa_evidence_close_gate(&cas_root, &task, &repo, "main", None)
            .expect_err("the live tip carries a web surface");
        assert!(refusal.contains("app.css"), "{refusal}");
    }

    /// cas-bde8 (GH #978): task A's UI commits on a reused factory branch were
    /// merged by PR, but the local `main` was never refreshed. Backend-only
    /// task B, next on the same branch, is judged against `origin/main`, where
    /// A's commits already live, so it owes no web QA evidence.
    #[test]
    fn a_stale_local_target_does_not_charge_the_previous_tasks_ui_to_this_task() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        commit_file(&repo, "README.md", "base\n");
        git(&repo, &["switch", "-q", "-c", "factory/worker"]);
        let task_a = commit_file(&repo, "hub-web/dist/app.css", "body { color: red }\n");
        // The PR merged A into the remote main; local main is stale.
        git(&repo, &["switch", "-q", "--detach", "main"]);
        git(
            &repo,
            &["merge", "-q", "--no-ff", "-m", "Merge PR: task A", &task_a],
        );
        let merged = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["update-ref", "refs/remotes/origin/main", &merged]);
        git(&repo, &["switch", "-q", "factory/worker"]);
        commit_file(&repo, "src/lib.rs", "pub fn backend() {}\n");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let mut task = Task::new("cas-bde8-b".to_string(), "Backend fix".to_string());
        task.assignee = Some("worker".to_string());

        qa_evidence_close_gate(&cas_root, &task, &repo, "main", None)
            .expect("task B's own diff is backend-only against origin/main");
    }

    /// cas-bde8 (GH #978): a squash-merged PR leaves task A's commits outside
    /// the target's ancestry, so no branch diff can separate them. The close
    /// path's task-attributed paths can, and they are what is judged.
    #[test]
    fn attributed_paths_judge_only_this_tasks_delivery_after_a_squash_merge() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        commit_file(&repo, "README.md", "base\n");
        git(&repo, &["switch", "-q", "-c", "factory/worker"]);
        commit_file(&repo, "hub-web/dist/app.css", "body { color: red }\n");
        // Squash merge: the remote main carries A's change as a new commit.
        git(&repo, &["switch", "-q", "--detach", "main"]);
        commit_file(&repo, "hub-web/dist/app.css", "body { color: red }\n");
        let squashed = git(&repo, &["rev-parse", "HEAD"]);
        git(
            &repo,
            &["update-ref", "refs/remotes/origin/main", &squashed],
        );
        git(&repo, &["switch", "-q", "factory/worker"]);
        commit_file(&repo, "src/lib.rs", "pub fn backend() {}\n");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let mut task = Task::new("cas-bde8-squash".to_string(), "Backend fix".to_string());
        task.assignee = Some("worker".to_string());

        let own = vec!["src/lib.rs".to_string()];
        qa_evidence_close_gate_for_paths(
            &cas_root,
            &task,
            &repo,
            "main",
            None,
            Some(own.as_slice()),
        )
        .expect("only this task's backend change is judged");
        // Without attribution the branch diff still sees A's UI: this is why
        // the close path passes the attributed paths.
        qa_evidence_close_gate(&cas_root, &task, &repo, "main", None)
            .expect_err("the branch-wide diff alone cannot tell A from B");
        // Nothing attributed falls back to the branch diff rather than
        // waving the gate through.
        qa_evidence_close_gate_for_paths(
            &cas_root,
            &task,
            &repo,
            "main",
            None,
            Some(Vec::<String>::new().as_slice()),
        )
        .expect_err("an empty attribution is not evidence of a backend-only task");
    }

    #[test]
    fn web_surface_reasons_need_the_bundle_and_demo_only_needs_the_ledger() {
        assert_eq!(evidence_tier(&[], true), EvidenceTier::None);
        assert_eq!(
            evidence_tier(
                &["path:web/a.css (**/*.css)".into(), "demo_statement".into()],
                true
            ),
            EvidenceTier::Bundle
        );
        assert_eq!(
            evidence_tier(&["journeys:J03".into()], false),
            EvidenceTier::Bundle
        );
        assert_eq!(
            evidence_tier(&["demo_statement".into()], false),
            EvidenceTier::Ledger { terminal_qa: false }
        );
        assert_eq!(
            evidence_tier(&["demo_statement".into()], true),
            EvidenceTier::Ledger { terminal_qa: true }
        );
    }
}
