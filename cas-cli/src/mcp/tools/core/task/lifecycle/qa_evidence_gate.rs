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
    delivery_test_diff, range_paths, run_close_gate_with_write_dir,
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
        // cas-73b8: the per-task branch when the worker used one.
        && let Some(tip) = resolve_branch_sha(
            repo,
            &super::close_ops::close_measured_factory_branch(repo, task, assignee),
        )
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
/// says the diff touches command output rather than only interactive surfaces.
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

/// A command's stdout capture cannot exercise a factory mouse event or PTY
/// resize. Those surfaces keep the real-build ledger requirement. Examine
/// every path so an interactive change cannot hide a command-output change.
fn requires_terminal_qa(paths: &[String], qa: &crate::config::QaConfig) -> bool {
    if qa.terminal_render_paths.is_empty() {
        return false;
    }
    let cli = ["**/cli/**".to_string()];
    paths.iter().filter(|path| !is_non_surface_path(path)).any(|path| {
        let one = std::slice::from_ref(path);
        // Existing config files can retain the older default output globs,
        // without **/cli/**. Do not let an input exemption hide CLI output.
        first_user_facing_path(one, &cli).is_some()
            || (first_user_facing_path(one, &qa.terminal_render_paths).is_some()
                && first_user_facing_path(one, &qa.terminal_interaction_paths).is_none())
    })
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

fn task_qa_artifacts_dir(cas_root: &Path, base: &Path, task: &Task) -> PathBuf {
    let [scoped, legacy] = crate::config::factory_task_artifact_dirs(cas_root, base, &task.id);
    let cited = crate::qa_evidence::cited_bundle_path(&task.notes);
    // Explicit historical citations remain valid. Otherwise prefer new QA
    // evidence; a namespace created only for issue attachments must not hide
    // an existing legacy ledger or terminal receipt.
    let legacy_cited = cited.as_deref().is_some_and(|path| {
        let path = crate::qa_evidence::expand_home(path);
        path.starts_with(&legacy) && std::fs::symlink_metadata(path).is_ok()
    });
    let scoped_evidence = scoped.join("LEDGER.md").exists()
        || scoped.join("qa").exists()
        || scoped.join("terminal-qa").exists();
    if (legacy_cited || (cited.is_none() && !scoped_evidence))
        && crate::config::canonical_factory_task_artifact_dir(base, &legacy).is_some()
    {
        legacy
    } else {
        scoped
    }
}

/// A missing flat citation can outlive the namespace migration in a running
/// service. Read its corresponding scoped file without rewriting task history.
/// Existing historical files, unsafe relative paths and unrelated citations
/// keep their original validation; all mapped bundles still undergo the full
/// containment, freshness, task/head binding and trace checks.
fn task_qa_notes<'a>(
    paths: &crate::config::FactoryArtifactPaths,
    task: &'a Task,
    read_dir: &Path,
) -> std::borrow::Cow<'a, str> {
    let original = std::borrow::Cow::Borrowed(task.notes.as_str());
    let [scoped, legacy] = paths.task_dirs(&task.id);
    if read_dir != scoped {
        return original;
    }
    let Some(cited) = crate::qa_evidence::cited_bundle_path(&task.notes) else {
        return original;
    };
    let cited = crate::qa_evidence::expand_home(&cited);
    let Ok(relative) = cited.strip_prefix(&legacy) else {
        return original;
    };
    if relative.components().any(|part| !matches!(part, std::path::Component::Normal(_)))
        || !std::fs::symlink_metadata(&cited)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return original;
    }
    let replacement = scoped.join(relative);
    if !replacement.is_file() {
        return original;
    }
    std::borrow::Cow::Owned(format!("{}\nqa-bundle: {}", task.notes, replacement.display()))
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
/// task's changes, only those are judged, including an empty content range
/// from a merge-only or formatting-only task. With no selected task range or
/// legacy history, the branch diff against the live target applies.
pub(crate) fn qa_evidence_close_gate_for_paths(
    cas_root: &Path,
    task: &Task,
    repo: &Path,
    target_branch: &str,
    commit_receipt: Option<&str>,
    attributed_paths: Option<&[String]>,
) -> Result<Vec<String>, String> {
    qa_evidence_close_gate_for_delivery(cas_root, task, repo, target_branch, commit_receipt, attributed_paths, None)
}

pub(crate) fn qa_evidence_close_gate_for_delivery(
    cas_root: &Path,
    task: &Task,
    repo: &Path,
    target_branch: &str,
    commit_receipt: Option<&str>,
    attributed_paths: Option<&[String]>,
    attributed_base: Option<&str>,
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
    // An empty attributed set is authoritative: a merge-only or formatting-
    // only task delivered no reviewable UI, even if the branch's merge diff
    // contains UI files from the other parent (GH #1037).
    let changed = match attributed_paths {
        Some(paths) => Some(paths.to_vec()),
        None => range
            .as_ref()
            .and_then(|(from, to)| range_paths(repo, from, to)),
    };
    let journeys = changed
        .as_deref()
        .map(|paths| catalog_journeys_for(repo, Some(&head), paths))
        .unwrap_or_default();
    let mut reasons = user_facing_reasons(task, &qa, changed.as_deref(), &journeys).reasons;
    if let Some(changed) = changed.as_deref().filter(|p| crate::qa_evidence::journeys::affects_hub(p)) {
        let base = attributed_base.or_else(|| range.as_ref().map(|(base, _)| base.as_str()))
            .ok_or("QA EVIDENCE REJECTED: cannot establish the task-attributed journey selection base")?;
        let base = super::close_ops::resolve_branch_sha(repo, &format!("{base}^{{commit}}"))
            .ok_or("QA EVIDENCE REJECTED: journey selection base is unreadable")?;
        let selected = crate::qa_evidence::journeys::select_journeys(repo, &base, &head, Some(changed))
            .map_err(|e| format!("QA EVIDENCE REJECTED: {e}; affected paths [{}]; run scripts/journey-eval.sh for the repaired selection", changed.join(", ")))?;
        reasons.push(crate::qa_evidence::journeys::selection_reason(&base, &selected));
    }
    let terminal_render = changed.as_deref().is_some_and(|paths| requires_terminal_qa(paths, &qa));
    let markers: Vec<SkipMarker> = match (range.as_ref(), changed.as_deref()) {
        (Some((from, to)), Some(paths)) => delivery_test_diff(repo, from, to, paths)
            .map(|diff| added_skip_markers(&diff))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let paths = crate::config::resolved_factory_artifact_paths(cas_root, config.factory().artifacts_root.as_deref());
    let artifacts_dir = task_qa_artifacts_dir(cas_root, &paths.base, task);
    if artifacts_dir.exists() && crate::config::canonical_factory_task_artifact_dir(&paths.base, &artifacts_dir).is_none() {
        return Err("QA EVIDENCE REJECTED: task artifact directory aliases another project's namespace or escapes its configured base".into());
    }
    let notes = task_qa_notes(&paths, task, &artifacts_dir);
    let ctx = EvidenceContext {
        task_id: &task.id,
        task_artifacts_dir: &artifacts_dir,
        repo,
        delivered_head: &head,
        notes: &notes,
        deployed_origins: &qa.deployed_origins,
    };
    let tier = evidence_tier(&reasons, terminal_render);
    // Waivers authorize missing implementer ledger evidence only at the exact
    // delivered commit. Independent QA PASS and ancestor waivers do not do so.
    let waiver = if matches!(tier, EvidenceTier::Ledger { .. }) {
        cas_store::satisfying_qa_passes(cas_root, &task.id)
            .map_err(|err| format!("QA EVIDENCE REJECTED: cannot read QA waivers: {err}"))?
            .into_iter()
            .find(|pass| pass.bound_head == head && pass.state == cas_types::QaPassState::Waived)
    } else {
        None
    };
    let mut pass = run_close_gate_with_write_dir(
        &ctx,
        if waiver.is_some() {
            EvidenceTier::None
        } else {
            tier
        },
        &reasons,
        &markers,
        &paths.task_dirs(&task.id)[0],
    )?;
    if let Some(waiver) = waiver {
        pass.notes.push(format!(
            "QA evidence ledger waived: head={} waiver={} supervisor={} reason={}",
            head,
            waiver.id,
            waiver.issuer_agent_id.as_deref().unwrap_or(""),
            waiver.summary.as_deref().unwrap_or("")
        ));
    }
    if !pass.deferred_deployed.is_empty() {
        let agents = crate::store::open_agent_store(cas_root)
            .and_then(|store| Ok(store.list(None)?))
            .map_err(|err| {
                format!("QA EVIDENCE REJECTED: cannot validate deployed-verification owner: {err}")
            })?;
        for deferred in pass.deferred_deployed {
            let by_id = agents.iter().find(|agent| agent.id == deferred.owner);
            let named: Vec<_> = agents
                .iter()
                .filter(|agent| agent.name == deferred.owner)
                .collect();
            let owner = by_id.or_else(|| (named.len() == 1).then(|| named[0]));
            let Some(owner) = owner.filter(|agent| agent.role == cas_types::AgentRole::Supervisor)
            else {
                return Err(format!(
                    "QA EVIDENCE REJECTED: deployed-verification owner={} must identify one registered supervisor (use the agent id for ambiguous names)",
                    deferred.owner
                ));
            };
            pass.notes.push(format!(
                "POST-DEPLOY OBLIGATION: task={} head={} row={} ledger={} owner={} ({}); deferred deployed verification, not PASS.",
                task.id, head, deferred.row_id, deferred.ledger.display(), owner.name, owner.id
            ));
        }
    }
    Ok(pass.notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interaction_classification_is_conservative_and_configurable_cas_266e() {
        let mut qa = crate::config::QaConfig::default();
        let factory = vec!["cas-cli/src/ui/factory/daemon/runtime/output.rs".into()];
        assert!(!requires_terminal_qa(&factory, &qa));
        assert!(requires_terminal_qa(&["src/ui/status.rs".into()], &qa));
        assert!(requires_terminal_qa(&["src/theme.rs".into()], &qa));
        qa.terminal_interaction_paths.clear();
        assert!(requires_terminal_qa(&factory, &qa));
        qa.terminal_interaction_paths = vec!["**/tui/**".into()];
        assert!(!requires_terminal_qa(&["src/tui/input.rs".into()], &qa));
        qa.terminal_render_paths = vec!["**/ui/**".into()];
        qa.terminal_interaction_paths = vec!["**".into()];
        assert!(requires_terminal_qa(&["src/cli/status.rs".into()], &qa));
        assert!(!requires_terminal_qa(&["tests/cli/output_test.rs".into()], &qa));
        qa.terminal_render_paths.clear();
        assert!(!requires_terminal_qa(&["src/cli/status.rs".into()], &qa));
    }

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
    fn qa_prefers_scoped_evidence_and_honors_explicit_legacy_citations_cas_6ebf() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join("project/.cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let base = temp.path().join("artifacts");
        let [scoped, legacy] = crate::config::factory_task_artifact_dirs(&cas_root, &base, "cas-a4b1");
        let mut task = Task::new("cas-a4b1".into(), "QA evidence".into());
        std::fs::create_dir_all(scoped.join("github-issues")).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("LEDGER.md"), "historical evidence").unwrap();
        assert_eq!(task_qa_artifacts_dir(&cas_root, &base, &task), legacy);
        std::fs::write(scoped.join("LEDGER.md"), "new evidence").unwrap();
        assert_eq!(task_qa_artifacts_dir(&cas_root, &base, &task), scoped);
        std::fs::create_dir_all(legacy.join("qa")).unwrap();
        std::fs::write(legacy.join("qa/bundle.json"), "historical bundle").unwrap();
        task.notes = format!("qa-bundle: {}", legacy.join("qa/bundle.json").display());
        assert_eq!(task_qa_artifacts_dir(&cas_root, &base, &task), legacy);
        task.notes = format!("qa-bundle: {}", scoped.join("qa/bundle.json").display());
        assert_eq!(task_qa_artifacts_dir(&cas_root, &base, &task), scoped);
        std::fs::write(legacy.join("LEDGER.md"), "").unwrap();
        let ctx = EvidenceContext {
            task_id: &task.id,
            task_artifacts_dir: &legacy,
            repo: temp.path(),
            delivered_head: "",
            notes: &task.notes,
            deployed_origins: &[],
        };
        let error = run_close_gate_with_write_dir(&ctx, EvidenceTier::Ledger { terminal_qa: false }, &["demo".into()], &[], &scoped).unwrap_err();
        let (problem, next) = error.split_once("Next: ").unwrap();
        assert!(problem.contains(legacy.to_str().unwrap()), "{error}");
        assert!(next.contains(scoped.to_str().unwrap()), "{error}");
        assert!(!next.contains(legacy.to_str().unwrap()), "{error}");

    }

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
        let task_a = commit_file(&repo, "hub-web/dist/app.css", "body { color: red }\n");
        // Squash merge: the remote main carries A's change as a new commit.
        // It needs its own message: with commit_file's message (the path), the
        // same parent, tree and author inside one clock second, git computes
        // A's own id, A becomes an ancestor of origin/main, and the fixture is
        // no longer a squash merge (cas-e5ac: red in CI, empty branch diff).
        git(&repo, &["switch", "-q", "--detach", "main"]);
        // main predates A, so its checkout has no hub-web/dist yet.
        std::fs::create_dir_all(repo.join("hub-web/dist")).unwrap();
        std::fs::write(repo.join("hub-web/dist/app.css"), "body { color: red }\n").unwrap();
        git(&repo, &["add", "hub-web/dist/app.css"]);
        git(&repo, &["commit", "-q", "-m", "Task A (#1) (squashed)"]);
        let squashed = git(&repo, &["rev-parse", "HEAD"]);
        assert_ne!(squashed, task_a, "the squash commit is a new commit");
        git(
            &repo,
            &["update-ref", "refs/remotes/origin/main", &squashed],
        );
        let a_is_ancestor = Command::new("git")
            .args(["merge-base", "--is-ancestor", &task_a, "origin/main"])
            .current_dir(&repo)
            .status()
            .expect("run git")
            .success();
        assert!(
            !a_is_ancestor,
            "a squash merge leaves task A's commit outside origin/main's ancestry"
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
        let refusal = qa_evidence_close_gate(&cas_root, &task, &repo, "main", None)
            .expect_err("the branch-wide diff alone cannot tell A from B");
        assert!(refusal.contains("app.css"), "{refusal}");
        // An empty set from a selected delivery range is authoritative.
        // Callers pass None when attribution found no delivery range.
        qa_evidence_close_gate_for_paths(
            &cas_root,
            &task,
            &repo,
            "main",
            None,
            Some(Vec::<String>::new().as_slice()),
        )
        .expect("an attributed merge-only delivery has no UI change");
    }

    /// cas-e86b: the evidence gate reads the same shared reasons, so a
    /// fixture-only HTML diff needs no evidence while a product page beside
    /// it still needs the bundle.
    #[test]
    fn fixture_html_needs_no_evidence_but_product_html_needs_the_bundle_cas_e86b() {
        let qa = crate::config::QaConfig::default();
        let task = Task::new("cas-e86b-fixture".into(), "checker fixtures".into());
        let fixtures: Vec<String> = [
            "scripts/visual-qa.mjs",
            "scripts/visual-qa-fixtures/clip-box.html",
            "scripts/visual-qa-fixtures/clip-overflow.html",
        ]
        .map(String::from)
        .to_vec();
        let reasons = user_facing_reasons(&task, &qa, Some(&fixtures), &[]).reasons;
        assert_eq!(evidence_tier(&reasons, false), EvidenceTier::None, "{reasons:?}");

        let mut mixed = fixtures.clone();
        mixed.push("hub-web/src/styles.css".into());
        let reasons = user_facing_reasons(&task, &qa, Some(&mixed), &[]).reasons;
        assert_eq!(evidence_tier(&reasons, false), EvidenceTier::Bundle, "{reasons:?}");
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
