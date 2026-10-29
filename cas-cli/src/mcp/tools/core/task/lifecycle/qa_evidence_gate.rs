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
/// worker's next task). A deleted lane still uses its durable anchor before
/// falling back to the repository HEAD.
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
    // The durable branch remains the delivery after a lease release clears
    // the assignee; the closing checkout's HEAD may be the target itself.
    let branch = task.assignee.as_deref().map(|assignee| {
        super::close_ops::close_measured_factory_branch(repo, task, assignee)
    }).or_else(|| super::close_ops::task_delivery_branch(repo, task));
    if let Some(tip) = branch.as_deref().and_then(|branch| resolve_branch_sha(repo, branch))
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
    task.deliverables.factory_branch_anchor.as_deref().and_then(rev)
        .or_else(|| rev("HEAD"))
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
    // Older generated configs stored filename guesses such as *render*.
    // Those match PDF/HTML renderers and server logs too (GH #1212). Upgrade
    // only the two known complete default lists; custom paths are declarations
    // of terminal surfaces and retain their configured meaning.
    let old_defaults = [
        "**/ui/**",
        "**/tui/**",
        "**/*render*",
        "**/*output*",
        "**/*theme*",
        "**/*progress*",
    ];
    let old_default_list = old_defaults
        .iter()
        .all(|glob| qa.terminal_render_paths.iter().any(|path| path == glob))
        && qa
            .terminal_render_paths
            .iter()
            .all(|path| path == "**/cli/**" || old_defaults.contains(&path.as_str()));
    let render_paths = if old_default_list {
        std::borrow::Cow::Owned(crate::config::default_terminal_render_paths())
    } else {
        std::borrow::Cow::Borrowed(&qa.terminal_render_paths)
    };
    let required_output = [
        "**/cli/**".to_string(),
        "**/cas-mux/src/render.rs".to_string(),
        "**/cas-mux/src/pane/style.rs".to_string(),
        "**/cas-mux/src/pane/snapshot.rs".to_string(),
    ];
    paths
        .iter()
        .filter(|path| !is_non_surface_path(path))
        .any(|path| {
            let one = std::slice::from_ref(path);
            // Existing config files can retain the older default output globs,
            // without **/cli/**. A blanket cas-mux interaction glob likewise
            // must not hide dedicated drawing/styles behind an input exemption.
            // pane/mod.rs is mixed input/geometry code: its live interaction
            // still needs the real-build ledger rather than a stdout capture.
            first_user_facing_path(one, &required_output).is_some()
                || (first_user_facing_path(one, &render_paths).is_some()
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
    qa_evidence_close_gate_for_paths(cas_root, task, repo, target_branch, commit_receipt, None, None)
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

/// A landed rewrite may reuse the passed round's own cited bundle only when
/// its complete delivery diff is identical. Ordinary PASS never replaces the
/// implementer's evidence, and a citation alone grants no approval.
pub(super) fn approved_receipt_bundle(
    cas_root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    receipt: Option<&str>,
) -> Option<(cas_types::QaPass, PathBuf, String)> {
    use crate::bounded_process::{Deadline, run_command};
    use crate::git_evidence::resolve_ref_commit_sha;
    use std::time::Duration;

    let deadline = Deadline::after(Duration::from_secs(5));
    let git = |args: &[&str]| {
        run_command(
            Command::new("git").current_dir(repo).args(args),
            deadline,
            Duration::from_secs(1),
        )
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
    };
    let receipt = resolve_ref_commit_sha(repo, receipt?)?;
    let target =
        resolve_ref_commit_sha(repo, &super::qa_dispatch::freshest_target_ref(repo, target))?;
    git(&["merge-base", "--is-ancestor", &receipt, &target])?;
    let start = task.deliverables.factory_branch_start.as_ref()?;
    if task.deliverables.work_target.as_ref() != Some(&start.work_target) {
        return None;
    }
    let base = resolve_ref_commit_sha(repo, &start.base_sha)?;
    let config = crate::config::Config::load(cas_root).ok()?;
    let paths = crate::config::resolved_factory_artifact_paths(
        cas_root,
        config.factory().artifacts_root.as_deref(),
    );
    let passes = cas_store::satisfying_qa_passes(cas_root, &task.id).ok()?;
    let diff = |from: &str, to: &str| {
        git(&[
            "diff",
            "--binary",
            "--full-index",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-relative",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            from,
            to,
            "--",
        ])
    };
    passes.into_iter().find_map(|pass| {
        if pass.state != cas_types::QaPassState::Passed || pass.branch != start.branch {
            return None;
        }
        let reviewed = resolve_ref_commit_sha(repo, &pass.bound_head)?;
        if task.deliverables.factory_branch_anchor.as_deref().is_some_and(|anchor| {
            resolve_ref_commit_sha(repo, anchor)
                .is_none_or(|anchor| anchor != reviewed && anchor != receipt)
        }) {
            return None;
        }
        if reviewed == receipt
            || git(&["merge-base", "--is-ancestor", &reviewed, &receipt]).is_some()
        {
            return None;
        }
        git(&["merge-base", "--is-ancestor", &base, &reviewed])?;
        let history = String::from_utf8(git(&[
            "rev-list",
            "--parents",
            &format!("{base}..{reviewed}"),
        ])?)
        .ok()?;
        let count = history.lines().count();
        if count == 0
            || count > 200
            || history
                .lines()
                .any(|line| line.split_whitespace().count() != 2)
        {
            return None;
        }
        let reviewed_diff = diff(&base, &reviewed)?;
        if reviewed_diff.is_empty() {
            return None;
        }
        let ledger = Path::new(pass.ledger_path.as_deref()?);
        if !ledger.is_file() || std::fs::metadata(ledger).ok()?.len() == 0 {
            return None;
        }
        let bundle =
            crate::qa_pass::validate_round_bundle(ledger, &pass, cas_types::QaVerdict::Approved)
                .ok()?
                .canonicalize()
                .ok()?;
        if !paths.task_dirs(&task.id).iter().any(|dir| {
            crate::config::canonical_factory_task_artifact_dir(&paths.base, dir)
                .is_some_and(|root| bundle.starts_with(&root))
        }) {
            return None;
        }
        let cited = task
            .notes
            .lines()
            .filter(|line| line.contains("PLATFORM_PROOF"))
            .filter_map(|line| {
                line.split_once(crate::qa_evidence::REVIEW_BUNDLE_CITATION)
                    .or_else(|| line.split_once(crate::qa_evidence::BUNDLE_CITATION))
            })
            .filter_map(|(_, rest)| rest.split_whitespace().next())
            .any(|raw| {
                let path = raw.trim_matches(|ch: char| "(),[]{}<>\"'`;*.".contains(ch));
                crate::qa_evidence::expand_home(path)
                    .canonicalize()
                    .ok()
                    .as_ref()
                    == Some(&bundle)
            });
        if !cited {
            return None;
        }
        // Match the aggregate, never just the last cherry-pick. Trying the
        // bounded parent chain also permits a squash of those same commits.
        let parents = String::from_utf8(git(&[
            "rev-list",
            "--first-parent",
            &format!("--max-count={}", count + 1),
            &receipt,
        ])?)
        .ok()?;
        let receipt_base = parents
            .lines()
            .skip(1)
            .find(|candidate| {
                diff(candidate, &receipt).is_some_and(|patch| patch == reviewed_diff)
            })?
            .to_string();
        Some((pass, bundle, receipt_base))
    })
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
    recorded_base: Option<&str>,
) -> Result<Vec<String>, String> {
    let Ok(config) = crate::config::Config::load(cas_root) else {
        return Ok(Vec::new());
    };
    let qa = config.qa();
    let batch_mode = qa.mode == crate::config::QaMode::Batch;
    let deferred = cas_store::has_qa_batch_deferral(cas_root, &task.id)
        .map_err(|e| format!("QA EVIDENCE REJECTED: cannot read batch deferral: {e}"))?;
    if (!qa.evidence_gate
        || task.task_type == TaskType::Epic
        || task.execution_note.as_deref() == Some("no-code")) && !deferred
    {
        return Ok(Vec::new());
    }
    let Some(head) = delivered_head(task, repo, target_branch, commit_receipt) else {
        if deferred {
            return Err("QA EVIDENCE REJECTED: deferred batch delivery has no resolvable head; restore its delivery anchor and record a batch_receipt".into());
        }
        return Ok(Vec::new());
    };
    let carried_bundle = approved_receipt_bundle(cas_root, task, repo, target_branch, commit_receipt);
    let range = carried_bundle.as_ref().map(|(_, _, base)| (base.clone(), head.clone())).or_else(|| (!target_branch.is_empty())
        .then(|| delivery_range(repo, &head, &live_target_ref(repo, target_branch), recorded_base))
        .flatten());
    // An empty attributed set is authoritative: a merge-only or formatting-
    // only task delivered no reviewable UI, even if the branch's merge diff
    // contains UI files from the other parent (GH #1037).
    let changed = if carried_bundle.is_some() {
        // Exact aggregate equivalence supplies the delivery boundary even
        // when legacy attribution can recover only the final cherry-pick.
        range
            .as_ref()
            .and_then(|(from, to)| range_paths(repo, from, to))
    } else {
        match attributed_paths {
            Some(paths) => Some(paths.to_vec()),
            None => range
                .as_ref()
                .and_then(|(from, to)| range_paths(repo, from, to)),
        }
    };
    let journeys = changed
        .as_deref()
        .map(|paths| catalog_journeys_for(repo, Some(&head), paths))
        .unwrap_or_default();
    let reasons = user_facing_reasons(task, &qa, changed.as_deref(), &journeys).reasons;
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
        capture_configured: qa.capture.as_ref().is_some_and(|capture| capture.is_configured().unwrap_or(true)),
        rebind_scope: changed.as_deref().map(|files| (&qa, files)),
    };
    let tier = evidence_tier(&reasons, terminal_render);
    let carried_bundle = carried_bundle.filter(|_| tier == EvidenceTier::Bundle);
    let batch_receipt = if batch_mode || deferred {
        crate::qa_batch::satisfying_receipt(cas_root, task, repo, target_branch, &head)
            .map_err(|e| format!("QA EVIDENCE REJECTED: cannot read batch receipts: {e}"))?
    } else { None };
    let integrated = (batch_mode || deferred).then(|| crate::qa_batch::integration_state(repo, &head, target_branch)).flatten();
    let covering_pass = if deferred {
        crate::qa_batch::covering_pass(cas_root, task, repo, target_branch, &head)
            .map_err(|e| format!("QA EVIDENCE REJECTED: cannot read covering QA pass: {e}"))?
    } else { None };
    if deferred && integrated != Some(false) && batch_receipt.is_none() && covering_pass.is_none() {
        return Err(crate::qa_batch::debt_refusal(&task.id));
    }
    // Only proven unmerged code may defer. Unknown target/ancestry retains the normal gate.
    let batch_defer = batch_mode && integrated == Some(false) && !matches!(tier, EvidenceTier::None);
    // Optional citations alone do not upgrade None/Ledger proof requirements.
    // A declared comparison still applies when batch approval replaces a bundle.
    let pair_coverage = if tier != EvidenceTier::Bundle || batch_receipt.is_some() || batch_defer || covering_pass.is_some() || carried_bundle.is_some() {
        crate::qa_evidence::validate_cited_pair_coverage(&ctx)
    } else {
        Ok(false)
    };
    // A supervisor waiver covers implementer evidence at this exact delivered
    // commit, including bundles (GH #1199). Independent QA PASS and ancestor
    // waivers do not do so; skip/focus markers still run below.
    // None-tier evidence needs a waiver only when declared coverage failed.
    let waiver = if tier != EvidenceTier::None || pair_coverage.is_err() {
        cas_store::satisfying_qa_passes(cas_root, &task.id)
            .map_err(|err| format!("QA EVIDENCE REJECTED: cannot read QA waivers: {err}"))?
            .into_iter()
            .find(|pass| pass.bound_head == head && pass.state == cas_types::QaPassState::Waived)
    } else {
        None
    };
    let restored = if waiver.is_none() && carried_bundle.is_none()
        && (tier == EvidenceTier::Bundle || pair_coverage.is_err()) {
        crate::artifacts::qa::restore(cas_root, task, &notes, &artifacts_dir)
            .map_err(|error| format!("QA EVIDENCE REJECTED: cannot restore published bundle: {error}"))?
    } else { None };
    let resolved_notes = match restored {
        Some(bundle) => format!("{notes}\nqa-bundle: {}", bundle.display()),
        None => notes.to_string(),
    };
    let ctx = EvidenceContext { notes: &resolved_notes, ..ctx };
    let pair_coverage = if tier != EvidenceTier::Bundle || batch_receipt.is_some() || batch_defer || covering_pass.is_some() || carried_bundle.is_some() {
        crate::qa_evidence::validate_cited_pair_coverage(&ctx)
    } else { pair_coverage };
    // Batch deferral/approval can replace full proof, but cannot silently drop
    // cells from a declared comparison. Only the authenticated exact-tip waiver
    // above (or the close handler's recorded supervisor override) waives it.
    if waiver.is_none() {
        pair_coverage.map_err(|refusal| {
            format!("QA EVIDENCE REJECTED: {}. Next: {}", refusal.problem, refusal.command)
        })?;
    }
    let mut pass = run_close_gate_with_write_dir(
        &ctx,
        if waiver.is_some() || batch_receipt.is_some() || batch_defer || covering_pass.is_some() || carried_bundle.is_some() {
            EvidenceTier::None
        } else {
            tier
        },
        &reasons,
        &markers,
        &paths.task_dirs(&task.id)[0],
    )?;
    if let Some((round, bundle, _)) = carried_bundle {
        pass.notes.push(format!(
            "QA evidence bundle accepted for identical landed delivery: pass={} reviewed={} receipt={} bundle={}",
            round.id, round.bound_head, head, bundle.display()
        ));
    }
    if let Some(qa_pass) = covering_pass {
        pass.notes.push(format!("QA batch debt satisfied by passed independent QA: pass={} head={}", qa_pass.id, qa_pass.bound_head));
    }
    if batch_defer {
        cas_store::defer_qa_batch_evidence(cas_root, &task.id, &head)
            .map_err(|e| format!("QA EVIDENCE REJECTED: cannot record batch deferral: {e}"))?;
        pass.notes.push(format!("QA batch evidence deferred: task={} head={head}; staging receipt required before close", task.id));
    }
    if let Some(receipt) = batch_receipt {
        pass.notes.push(format!("QA batch evidence PASS: receipt={} staging={} ledger={} task={} anchor={}",
            receipt.id, receipt.staging_sha, receipt.ledger_artifact_id, task.id,
            receipt.task_heads.get(&task.id).map(String::as_str).unwrap_or("")));
    }
    if let Some(waiver) = waiver {
        let evidence = match tier {
            EvidenceTier::Bundle => "bundle",
            EvidenceTier::Ledger { .. } => "ledger",
            EvidenceTier::None => "pair coverage",
        };
        pass.notes.push(format!(
            "QA evidence {evidence} waived: head={} waiver={} supervisor={} reason={}",
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
        assert!(!requires_terminal_qa(&["src/theme.rs".into()], &qa));
        qa.terminal_render_paths.push("src/theme.rs".into());
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

    #[test]
    fn terminal_surface_defaults_and_declared_paths_cas_2de9() {
        let old = vec![
            "**/ui/**",
            "**/tui/**",
            "**/*render*",
            "**/*output*",
            "**/*theme*",
            "**/*progress*",
        ];
        let mut with_cli = old.clone();
        with_cli.push("**/cli/**");
        for configured in [
            crate::config::default_terminal_render_paths(),
            old.into_iter().map(String::from).collect(),
            with_cli.into_iter().map(String::from).collect(),
        ] {
            let mut qa = crate::config::QaConfig::default();
            qa.terminal_render_paths = configured;
            for path in [
                "src/contracts/pdf.renderer.ts",
                "src/services/output.service.ts",
                "src/theme.rs",
                "src/progress.rs",
                "src/service.ts",
                "tests/cli/status.rs",
            ] {
                assert!(!requires_terminal_qa(&[path.into()], &qa), "{path}: {qa:?}");
            }
            for path in [
                "src/cli/status.rs",
                "src/tui/status.rs",
                "src/ui/status.rs",
                "src/doctor.rs",
                "src/doctor/output.rs",
                "src/terminal/report.py",
                "crates/cas-mux/src/render.rs",
                "crates/cas-mux/src/pane/style.rs",
                "crates/cas-mux/src/pane/snapshot.rs",
                "crates/ghostty_vt/src/lib.rs",
                "crates/ghostty_vt_sys/src/lib.rs",
                "crates/cas-diffs/src/widget.rs",
                "crates/cas-diffs/src/highlight.rs",
            ] {
                assert!(requires_terminal_qa(&[path.into()], &qa), "{path}: {qa:?}");
            }
            for input in [
                "crates/cas-mux/src/input_stream.rs",
                "crates/cas-mux/src/pty.rs",
                "crates/cas-mux/src/pane/mod.rs",
            ] {
                assert!(!requires_terminal_qa(&[input.into()], &qa));
            }
            qa.terminal_render_paths = vec!["src/shared/status.rs".into()];
            assert!(requires_terminal_qa(&["src/shared/status.rs".into()], &qa));
            assert!(!requires_terminal_qa(
                &["src/contracts/pdf.renderer.ts".into()],
                &qa
            ));
            qa.terminal_render_paths.clear();
            assert!(!requires_terminal_qa(&["src/cli/status.rs".into()], &qa));
            assert!(!requires_terminal_qa(
                &["crates/cas-mux/src/render.rs".into()],
                &qa
            ));
        }
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
            deployed_origins: &[], capture_configured: false, rebind_scope: None,
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

        // A deleted lane must not replace its durable delivery with the
        // closing checkout's unrelated HEAD (cas-420af).
        task.deliverables.factory_branch_anchor = Some(rust_only.clone());
        git(&repo, &["switch", "-q", "main"]);
        git(&repo, &["branch", "-D", "factory/worker"]);
        assert_eq!(delivered_head(&task, &repo, "main", None), Some(rust_only));
    }

    fn merged_receipt_fixture_gh1181(first: &str, later: Option<&str>) -> (tempfile::TempDir, String) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        commit_file(&repo, "README.md", "base\n");
        git(&repo, &["switch", "-q", "-c", "factory/worker"]);
        commit_file(&repo, first, "first delivery\n");
        git(&repo, &["switch", "-q", "main"]);
        git(&repo, &["merge", "-q", "--no-ff", "factory/worker", "-m", "Merge first PR"]);
        let receipt = git(&repo, &["rev-parse", "HEAD"]);
        if let Some(later) = later {
            git(&repo, &["switch", "-q", "-c", "later-pr"]);
            commit_file(&repo, later, "later delivery\n");
            git(&repo, &["switch", "-q", "main"]);
            git(&repo, &["merge", "-q", "--no-ff", "later-pr", "-m", "Merge later PR"]);
        }
        std::fs::create_dir_all(temp.path().join(".cas")).unwrap();
        (temp, receipt)
    }

    #[test]
    fn backend_merge_receipt_does_not_require_qa_for_later_ui_merge_gh1181() {
        let (temp, receipt) = merged_receipt_fixture_gh1181(
            "src/service.ts", Some("hub-web/dist/CoachReportPreview.vue"),
        );
        let repo = temp.path().join("repo");
        let mut task = Task::new("cas-8103".into(), "Backend delivery".into());
        task.assignee = Some("worker".into());
        qa_evidence_close_gate(&temp.path().join(".cas"), &task, &repo, "main", Some(&receipt))
            .expect("the earlier backend merge must not own the later UI merge");
        let (from, to) = delivery_range(&repo, &receipt, "main", None).unwrap();
        assert_eq!(to, receipt);
        assert_eq!(range_paths(&repo, &from, &to), Some(vec!["src/service.ts".into()]));
    }

    #[test]
    fn ui_merge_receipt_keeps_its_own_qa_requirement_after_later_backend_merge_gh1181() {
        for later in [None, Some("src/service.ts")] {
            let (temp, receipt) = merged_receipt_fixture_gh1181("hub-web/dist/own.vue", later);
            let repo = temp.path().join("repo");
            let mut task = Task::new("cas-8104".into(), "UI delivery".into());
            task.assignee = Some("worker".into());
            let refusal = qa_evidence_close_gate(&temp.path().join(".cas"), &task, &repo, "main", Some(&receipt))
                .expect_err("the receipt's own UI still requires QA, even at the current target");
            assert!(refusal.contains("own.vue"), "{refusal}");
            assert!(!refusal.contains("src/service.ts"), "{refusal}");
        }
    }

    #[test]
    fn worker_tip_merge_uses_its_integration_merge_range_gh1181() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        commit_file(&repo, "README.md", "base\n");
        git(&repo, &["switch", "-q", "-c", "factory/worker"]);
        commit_file(&repo, "src/service.ts", "backend delivery\n");
        let own_commit = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["switch", "-q", "main"]);
        commit_file(&repo, "hub-web/dist/epic.vue", "incoming epic UI\n");
        git(&repo, &["switch", "-q", "factory/worker"]);
        git(&repo, &["merge", "-q", "--no-ff", "main", "-m", "Sync epic into worker"]);
        let worker_tip = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["switch", "-q", "main"]);
        git(&repo, &["merge", "-q", "--no-ff", "factory/worker", "-m", "Integrate worker delivery"]);
        let integration = git(&repo, &["rev-parse", "HEAD"]);
        std::fs::create_dir_all(temp.path().join(".cas")).unwrap();
        let mut task = Task::new("cas-8105".into(), "Backend delivery after sync".into());
        task.assignee = Some("worker".into());
        // cas-cca7: measured from the pre-sync anchor (the task's last own
        // commit), the first ancestry-path merge is the lane's own target
        // sync, whose first-parent diff is the incoming epic UI. Both heads
        // must resolve to the integration merge and the task-only paths.
        for head in [&worker_tip, &own_commit] {
            qa_evidence_close_gate(&temp.path().join(".cas"), &task, &repo, "main", Some(head))
                .expect("a worker sync merge must not adopt the epic's UI as its own delivery");
            let (from, to) = delivery_range(&repo, head, "main", None).unwrap();
            assert_eq!(to, integration, "head {head}");
            assert_eq!(range_paths(&repo, &from, &to), Some(vec!["src/service.ts".into()]));
        }
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
            None,
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
            None,
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
    fn api_paths_use_http_evidence_without_visual_bundle_cas_40d5() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        git(repo, &["init", "-q", "-b", "main"]);
        commit_file(repo, "seed.txt", "seed");
        git(repo, &["checkout", "-q", "-b", "factory/api"]);
        std::fs::create_dir_all(repo.join("lib")).unwrap();
        commit_file(repo, "lib/Server.tsx", "export const status = 400;");
        let head = resolve_branch_sha(repo, "HEAD").unwrap();
        let mut config = crate::config::Config::default();
        config.set("qa.api_paths", "app/api/**, lib/**").unwrap();
        assert_eq!(config.get("qa.api_paths").as_deref(), Some("app/api/**,lib/**"));
        assert!(config.list().contains(&("qa.api_paths".into(), "app/api/**,lib/**".into())));
        let cas = crate::store::init_cas_dir(repo).unwrap();
        config.save(&cas).unwrap();
        assert_eq!(crate::config::Config::load(&cas).unwrap().get("qa.api_paths"), config.get("qa.api_paths"));
        let qa = config.qa();
        let task = Task::new("cas-api".into(), "API status change".into());
        let mut paths = vec!["app/api/route.ts".into(), "lib/Server.tsx".into()];
        let reasons = user_facing_reasons(&task, &qa, Some(&paths), &["broad-app-catalog".into()]).reasons;
        let tier = evidence_tier(&reasons, false);
        assert_eq!(tier, EvidenceTier::Ledger { terminal_qa: false });
        assert!(!reasons.is_empty(), "API paths must trigger QA without a demo");
        let artifacts = repo.join("artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        let transcript = format!(
            "head_sha: {head}\norigin: http://127.0.0.1:3000\n\
             | A1 | invalid input | 400 | 400 | PASS | real-build | inline | |\n\
             request: POST http://127.0.0.1:3000/api/widgets\nstatus: 400\nbody: {{\"error\":\"invalid input\"}}\n"
        );
        std::fs::write(artifacts.join("LEDGER.md"), &transcript).unwrap();
        let ctx = EvidenceContext {
            task_id: &task.id, task_artifacts_dir: &artifacts, repo,
            delivered_head: &head, notes: "", deployed_origins: &[], capture_configured: false, rebind_scope: None,
        };
        let missing = run_close_gate_with_write_dir(&ctx, tier, &reasons, &[], &artifacts)
            .expect_err("the contract comparison is required, even when it has no differences");
        assert!(missing.contains("contracts.diff"), "{missing}");
        std::fs::write(artifacts.join("contracts.diff"), "").unwrap();
        run_close_gate_with_write_dir(&ctx, tier, &reasons, &[], &artifacts)
            .expect("two API artifacts suffice; no screenshots or bundle manifest");
        crate::store::open_task_store_local(&cas).unwrap().add(&task).unwrap();
        let round: cas_types::QaPass = serde_json::from_value(serde_json::json!({
            "id": "qp-api", "task_id": task.id, "round": 1, "implementer_agent_id": "worker",
            "branch": "factory/api", "bound_head": head, "state": "claimed",
            "requested_at": chrono::Utc::now(), "deadline_at": chrono::Utc::now(),
        })).unwrap();
        let approved = crate::qa_pass::validate_round_evidence(&cas, &artifacts.join("LEDGER.md"), &round, cas_types::QaVerdict::Approved).unwrap();
        assert_eq!(approved.0, "api-evidence:");
        std::fs::write(artifacts.join("LEDGER.md"), transcript.replace("| PASS |", "| FAIL |")).unwrap();
        crate::qa_pass::validate_round_evidence(&cas, &artifacts.join("LEDGER.md"), &round, cas_types::QaVerdict::Rejected)
            .expect("an API defect can be rejected without a passing capture or visual bundle");
        std::fs::write(artifacts.join("LEDGER.md"), transcript.replace("status: 400", "status: missing")).unwrap();
        assert!(run_close_gate_with_write_dir(&ctx, tier, &reasons, &[], &artifacts).is_err());
        paths.push("app/page.tsx".into());
        let mixed = user_facing_reasons(&task, &qa, Some(&paths), &[]).reasons;
        assert_eq!(evidence_tier(&mixed, false), EvidenceTier::Bundle);
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
