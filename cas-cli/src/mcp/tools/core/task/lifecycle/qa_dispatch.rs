//! Independent QA dispatch at the merge park (cas-619f).
//!
//! When a user-facing delivery parks for merge, open (or re-find) its QA
//! round, create the QA work item a different agent will start, and wake the
//! supervisor. Every step is best-effort relative to the park itself: a
//! failure here is reported in the refusal text and never loses the park,
//! and the merge/close gates still refuse until a verdict exists.

use std::path::Path;

use crate::mcp::tools::core::imports::*;
use crate::prompt_revalidation::QaDeliveryLocation;
use crate::qa_pass::{
    QA_PASS_LABEL, changed_paths_for_delivery, delivery_eligibility, qa_task_description,
    qa_task_title, round_dir,
};
use cas_store::{NewQaPass, QaPassOpen};
use cas_types::{Dependency, DependencyType, QaPass};

/// Outcome of the cas-619f close backstop.
pub(crate) enum QaCloseGate {
    /// No independent QA is owed (or a satisfying round already covers it).
    Clear,
    /// The close must wait; the text is the refusal.
    Refuse(String),
    /// A supervisor override waived the pass; the text is the decision note.
    Waived(String),
}

/// Dispatch carries the satisfying verdict separately from its presentation,
/// so close cannot prepend a QA-required refusal to a satisfied round.
struct QaDispatchStatus {
    text: String,
    satisfied: Option<QaPass>,
    /// cas-7877: no review is owed; an unreviewed round for an earlier tip
    /// was withdrawn instead of being re-dispatched.
    withdrawn: bool,
}

impl QaDispatchStatus {
    fn required(text: String) -> Self {
        Self {
            text,
            satisfied: None,
            withdrawn: false,
        }
    }
}

fn is_ancestor(repo: &Path, commit: &str, target: &str) -> bool {
    std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", commit, target])
        .current_dir(repo)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The target ref to classify a delivery against (cas-3760, GH #1066): the
/// fresher of the local branch and `origin/<target>`. A stale local target
/// puts the merge-base before commits the delivery already absorbed, so an
/// unrelated `.scss` change on the target counted as this task's and marked a
/// CI-only delivery user-facing. The close gate reads origin the same way
/// (`live_target_ref`); a local target ahead of origin (merged here, not yet
/// pushed) is kept, so neither side's newer history is lost.
pub(crate) fn freshest_target_ref(repo: &Path, target_branch: &str) -> String {
    let origin = format!("origin/{target_branch}");
    let Some(origin_sha) = super::close_ops::resolve_branch_sha(repo, &origin) else {
        return target_branch.to_string();
    };
    match super::close_ops::resolve_branch_sha(repo, target_branch) {
        Some(local_sha) if is_ancestor(repo, &origin_sha, &local_sha) => target_branch.to_string(),
        _ => origin,
    }
}

fn close_delivery_location<'a>(repo: &Path, head: &str, target: &'a str) -> QaDeliveryLocation<'a> {
    // The merge gate accepts an origin target that is ahead of this checkout's
    // local ref. Use the same integration evidence for the supervisor text.
    let on_target = is_ancestor(repo, head, target)
        || (!target.starts_with("origin/") && is_ancestor(repo, head, &format!("origin/{target}")));
    if on_target {
        QaDeliveryLocation::ContainedIn(target)
    } else {
        QaDeliveryLocation::UnmergedFrom(target)
    }
}

fn resolve_commit(repo: &Path, reference: &str) -> Option<String> {
    let reference = reference.trim();
    if reference.is_empty() || reference.starts_with('-') {
        return None;
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
}

/// The repository's trunk when `commit` is already on it.
fn trunk_containing(repo: &Path, commit: &str) -> Option<String> {
    let trunk = crate::mcp::tools::core::task::repo_context::resolve_default_branch(repo).ok()?;
    is_ancestor(repo, commit, &trunk).then_some(trunk)
}

fn patch_id_between(repo: &Path, base: &str, head: &str) -> Option<String> {
    use std::io::Write;
    let diff = std::process::Command::new("git")
        .args([
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            base,
            head,
            "--",
        ])
        .current_dir(repo)
        .output()
        .ok()?;
    if !diff.status.success() || diff.stdout.is_empty() {
        return None;
    }
    // File-fed input avoids blocking on Git's pipe buffers for large diffs.
    let mut input = tempfile::NamedTempFile::new().ok()?;
    input.write_all(&diff.stdout).ok()?;
    let output = std::process::Command::new("git")
        .args(["patch-id", "--stable"])
        .current_dir(repo)
        .stdin(std::fs::File::open(input.path()).ok()?)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let id = String::from_utf8(output.stdout)
        .ok()?
        .split_whitespace()
        .next()?
        .to_string();
    (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(id)
}

/// A rewritten delivery retains the QA identity of the reviewed tip. Prove
/// its aggregate delivered files in an integrated receipt (or current target),
/// allowing unrelated target files but never unresolved or empty Git evidence.
fn reviewed_tip_carried_by(repo: &Path, reviewed: &str, integrated: &str, target: &str) -> bool {
    if !is_ancestor(repo, integrated, target) {
        return false;
    }
    let output = std::process::Command::new("git")
        .args(["merge-base", reviewed, target])
        .current_dir(repo)
        .output();
    let base = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => return false,
    };
    let output = std::process::Command::new("git")
        .args([
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            &base,
            reviewed,
            "--",
        ])
        .current_dir(repo)
        .output();
    let paths = match output {
        Ok(output) if output.status.success() => output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8(path.to_vec()))
            .collect::<Result<Vec<_>, _>>(),
        _ => return false,
    };
    let Ok(paths) = paths else {
        return false;
    };
    if paths.is_empty() {
        return false;
    }
    let matches = std::process::Command::new("git")
        .args([
            "diff",
            "--quiet",
            "--no-ext-diff",
            "--no-textconv",
            reviewed,
            integrated,
            "--",
        ])
        .args(&paths)
        .current_dir(repo)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if matches {
        return true;
    }
    let Some(parent) = resolve_commit(repo, &format!("{integrated}^1")) else {
        return false;
    };
    // Compare S itself, not any older target commit which may since have
    // been changed or reverted. Unknown/empty patches never match.
    match (
        patch_id_between(repo, &base, reviewed),
        patch_id_between(repo, &parent, integrated),
    ) {
        (Some(reviewed_patch), Some(integrated_patch)) => reviewed_patch == integrated_patch,
        _ => false,
    }
}

/// cas-624f: rejected (failed) rounds on record for a delivery.
pub(crate) fn failed_qa_rounds(cas_root: &Path, task_id: &str) -> u32 {
    cas_store::list_qa_passes(cas_root, task_id)
        .unwrap_or_default()
        .iter()
        .filter(|pass| pass.state == cas_types::QaPassState::Failed)
        .count() as u32
}

fn qa_pass_covers_integrated_delivery(
    repo: &Path,
    pass: &QaPass,
    head: Option<&str>,
    receipt: Option<&str>,
    target: &str,
) -> bool {
    if !pass.state.satisfies_gate() {
        return false;
    }
    let Some(reviewed) = resolve_commit(repo, &pass.bound_head) else {
        return false;
    };
    let targets = [target.to_string(), format!("origin/{target}")];
    // Ordinary merges retain ancestry. Preserve that evidence and also
    // consult origin when the worker's local target has not advanced.
    if targets
        .iter()
        .any(|target| is_ancestor(repo, &reviewed, target))
    {
        return true;
    }
    let head = head.and_then(|head| resolve_commit(repo, head));
    let resolved_receipt = receipt.and_then(|receipt| resolve_commit(repo, receipt));
    if receipt.is_some() && resolved_receipt.is_none() {
        return false;
    }
    let receipt = resolved_receipt;
    // A recorded newer pre-merge tip needs its own verdict. A squash receipt
    // can carry the old reviewed identity when it is the delivery being closed.
    if head.as_deref() != Some(reviewed.as_str()) && !(head.is_some() && head == receipt) {
        return false;
    }
    targets.iter().any(|target| {
        let Some(target_tip) = resolve_commit(repo, target) else {
            return false;
        };
        let integrated = receipt.as_deref().unwrap_or(&target_tip);
        reviewed_tip_carried_by(repo, &reviewed, integrated, &target_tip)
    })
}

fn qa_delivery_not_proven(task: &Task, pass: &QaPass, target: &str) -> String {
    format!(
        "INDEPENDENT QA {}: task {} pass {} covers reviewed tip @{}, but Cassy cannot prove that delivery's content on {target}. No new QA round is needed for that reviewed tip. Refresh the target refs or provide commit_receipt=<integrated-delivery-sha>; changed delivery content needs its own round.",
        if pass.state == cas_types::QaPassState::Waived {
            "WAIVED"
        } else {
            "PASSED"
        },
        task.id,
        pass.id,
        pass.head8(),
    )
}

impl CasCore {
    /// The caller's QA identity: its registered name (what `task.assignee`
    /// and therefore `implementer_agent_id` hold) plus its session id. The
    /// no-self-review comparison must see both spellings.
    pub(crate) fn qa_caller_identity(&self) -> Result<(String, String), McpError> {
        let id = self.get_agent_id()?;
        let name = self
            .open_agent_store()
            .ok()
            .and_then(|store| store.get(&id).ok())
            .map(|agent| agent.name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| id.clone());
        Ok((name, id))
    }

    /// cas-619f: starting a QA work item claims its round for the caller,
    /// and refuses the delivery's implementer (under either identity).
    pub(crate) fn claim_qa_round_on_start(&self, qa_task: &Task) -> Result<Option<String>, McpError> {
        if !qa_task.labels.iter().any(|label| label == QA_PASS_LABEL) {
            return Ok(None);
        }
        let (name, id) = self.qa_caller_identity()?;
        let reject = |error: cas_store::StoreError| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Cannot start QA task {}: {error}", qa_task.id)),
            data: None,
        };
        // Check the session id too: an assignee recorded as an id must not
        // slip past a name-only comparison.
        cas_store::assert_may_review_qa_task(&self.cas_root, &qa_task.id, &id).map_err(reject)?;
        let Some(pass) =
            cas_store::assert_may_review_qa_task(&self.cas_root, &qa_task.id, &name).map_err(reject)?
        else {
            return Ok(None);
        };
        if !pass.state.is_active() {
            return Ok(Some(format!(
                "\nQA round {} is {} — nothing to review; ask the supervisor for the current round.",
                pass.id, pass.state
            )));
        }
        // cas-d5c1 (GH #1023 finding 6): check the reviewer's credentials,
        // env files and test-account capacity before the round is claimed,
        // so a setup gap is a clear blocker instead of a stalled round.
        let preflight = self.qa_reviewer_preflight(qa_task, &pass);
        if let Some(report) = preflight.as_ref().filter(|report| !report.is_ready()) {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "QA PREFLIGHT BLOCKED: {} was not started and round {} for {} @{} was not \
                     claimed, because this reviewer's environment is not ready:\n{}\n\
                     Tell the supervisor: `{}coordination action=message target=supervisor \
                     blocker=true summary=\"QA preflight blocked {}\" message=\"...\"` with the \
                     lines above, then start {} again once they are fixed.",
                    qa_task.id,
                    pass.round,
                    pass.task_id,
                    pass.head8(),
                    report.render(),
                    crate::mcp::tools::core::guidance::caller_prefix(),
                    qa_task.id,
                    qa_task.id,
                )),
                data: None,
            });
        }
        let claimed = cas_store::claim_qa_pass(&self.cas_root, &pass.task_id, &name, chrono::Utc::now())
            .map_err(reject)?;
        let preflight_note = preflight
            .map(|report| format!("\nQA preflight:\n{}", report.render()))
            .unwrap_or_default();
        Ok(Some(format!(
            "\nIndependent QA round {} claimed for {} @{} — you are the reviewer; deadline {}.{preflight_note}",
            claimed.round,
            claimed.task_id,
            claimed.head8(),
            claimed.deadline_at.to_rfc3339()
        )))
    }

    /// cas-d5c1: the project's QA preflight for this reviewer, or `None` when
    /// the project declares none. The report is also recorded on the QA work
    /// item; it holds no secret values (see `qa_pass::preflight`).
    fn qa_reviewer_preflight(
        &self,
        qa_task: &Task,
        pass: &QaPass,
    ) -> Option<crate::qa_pass::preflight::PreflightReport> {
        let config = crate::config::Config::load(&self.cas_root).ok()?;
        let qa = config.qa();
        if !crate::qa_pass::preflight::is_configured(&qa) {
            return None;
        }
        let repo_root = super::close_ops::resolve_close_gate_repo_root(&self.cas_root)
            .unwrap_or_else(|_| {
                self.cas_root
                    .parent()
                    .unwrap_or(&self.cas_root)
                    .to_path_buf()
            });
        let report = crate::qa_pass::preflight::run(
            &qa,
            &repo_root,
            crate::qa_pass::preflight::PreflightContext {
                delivery_task: &pass.task_id,
                qa_task: &qa_task.id,
                head: &pass.bound_head,
            },
        )?;
        if let Ok(store) = self.open_task_store() {
            let note = format!(
                "[{}] QA preflight for round {} @{}: {}\n{}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                pass.round,
                pass.head8(),
                if report.is_ready() {
                    "ready"
                } else {
                    "BLOCKED"
                },
                report.render()
            );
            let _ = store.append_note(&qa_task.id, &note);
        }
        Some(report)
    }

    /// Open the independent QA round for a delivery that just parked (or
    /// re-parked) for merge. Returns the text to append to the MERGE
    /// REQUIRED refusal, or `None` when the delivery is not user-facing.
    pub(crate) fn dispatch_independent_qa(
        &self,
        task: &Task,
        repo: &Path,
        parent_branch: &str,
        head: Option<&str>,
    ) -> Option<String> {
        let config = crate::config::Config::load(&self.cas_root).ok()?;
        let qa = config.qa();
        if !qa.independent_pass {
            return None;
        }
        let implementer = task.assignee.as_deref()?;
        let branch = super::close_ops::close_measured_factory_branch(repo, task, implementer);
        // The close gate has already selected this task's delivery tip. A
        // worker's older lane may contain unrelated UI changes (GH #1040).
        let delivery_ref = head.unwrap_or(&branch);
        let changed = match changed_paths_for_delivery(
            repo,
            &freshest_target_ref(repo, parent_branch),
            delivery_ref,
        ) {
            Ok(paths) => Some(paths),
            Err(error) => {
                tracing::warn!(task_id = %task.id, error = %error, "cas-619f: delivery diff unavailable; eligibility uses the demo_statement only");
                None
            }
        };
        self.independent_qa_for_paths(
            task,
            repo,
            parent_branch,
            &branch,
            head,
            changed,
            QaDeliveryLocation::ParkedForMerge,
            None,
        )
        .map(|status| status.text)
    }

    /// cas-74284: a supervisor asks for an independent QA round on a parked
    /// delivery that Cassy did not judge user-facing at the park (no
    /// demo_statement, no surface path or journey in its diff). Opens the
    /// round for the parked tip exactly as the park would, with the
    /// supervisor's reason recorded as the eligibility reason, so the merge
    /// gates wait for a verdict from then on. Returns the dispatch text, or
    /// why no round could be opened.
    pub(crate) fn request_independent_qa(
        &self,
        task: &Task,
        reason: &str,
    ) -> Result<String, String> {
        let config = crate::config::Config::load(&self.cas_root)
            .map_err(|error| format!("could not load config: {error}"))?;
        if !config.qa().independent_pass {
            return Err(
                "qa.independent_pass is off for this project, so no merge gate would wait for the round"
                    .to_string(),
            );
        }
        if task.status != TaskStatus::AwaitingMerge {
            return Err(format!(
                "{} is {:?}, not parked awaiting merge. A round binds a parked delivery tip;                  the worker's close parks it (and dispatches QA itself when the diff is user-facing).",
                task.id, task.status
            ));
        }
        let implementer = task.assignee.as_deref().ok_or_else(|| {
            format!(
                "{} has no assignee, so there is no implementer to keep out of the review",
                task.id
            )
        })?;
        // Resolve the delivery's repository and target the way the close
        // does: the task's (or its epic's) work target, else this Cassy
        // root's repository and the epic branch or trunk.
        let parent_epic = self
            .open_task_store()
            .ok()
            .and_then(|store| store.get_parent_epic(&task.id).ok().flatten());
        let (repo_root, target_branch) =
            match super::close_ops::effective_close_work_target(task, parent_epic.as_ref()) {
                Some(target) => {
                    let context =
                        crate::mcp::tools::core::task::repo_context::resolve_repo_context(
                            &self.cas_root,
                            &target,
                        )?;
                    (context.repo_root, context.target_branch)
                }
                None => {
                    let repo_root = super::close_ops::resolve_close_gate_repo_root(&self.cas_root)
                        .unwrap_or_else(|_| {
                            self.cas_root
                                .parent()
                                .unwrap_or(&self.cas_root)
                                .to_path_buf()
                        });
                    let target_branch =
                        match parent_epic.as_ref().and_then(|epic| epic.branch.clone()) {
                            Some(branch) => branch,
                            None => {
                                crate::mcp::tools::core::task::repo_context::resolve_default_branch(
                                    &repo_root,
                                )?
                            }
                        };
                    (repo_root, target_branch)
                }
            };
        let branch = super::close_ops::close_measured_factory_branch(&repo_root, task, implementer);
        let head = task
            .deliverables
            .factory_branch_anchor
            .clone()
            .or_else(|| super::close_ops::resolve_branch_sha(&repo_root, &branch))
            .ok_or_else(|| {
                format!(
                    "the parked tip of {} could not be resolved ({branch} does not resolve and no anchor is recorded)",
                    task.id
                )
            })?;
        let changed = changed_paths_for_delivery(
            &repo_root,
            &freshest_target_ref(&repo_root, &target_branch),
            &head,
        )
        .ok();
        self.independent_qa_for_paths(
            task,
            &repo_root,
            &target_branch,
            &branch,
            Some(&head),
            changed,
            QaDeliveryLocation::ParkedForMerge,
            Some(reason),
        )
        .map(|status| status.text)
        .ok_or_else(|| format!("Cassy could not open a round for {} @{head}", task.id))
    }

    /// Shared tail of the park and the close backstop: decide eligibility
    /// from a known (or unknown) change set, then open the round.
    ///
    /// The location is measured at dispatch: reaching the close backstop
    /// does not prove the delivered tip was merged or that it never parked.
    fn independent_qa_for_paths(
        &self,
        task: &Task,
        repo: &Path,
        parent_branch: &str,
        branch: &str,
        head: Option<&str>,
        changed: Option<Vec<String>>,
        location: QaDeliveryLocation<'_>,
        requested: Option<&str>,
    ) -> Option<QaDispatchStatus> {
        let config = crate::config::Config::load(&self.cas_root).ok()?;
        let qa = config.qa();
        let implementer = task.assignee.as_deref()?;
        // cas-7877: the close backstop reaches here for a delivery that is not
        // integrated yet, with no change set. Judging it from the demo alone
        // re-dispatched a stale round as a "re-review" for a backend-only tip.
        // Measure the tip's own diff against the fresh target instead.
        let changed = changed.or_else(|| {
            head.and_then(|head| {
                changed_paths_for_delivery(repo, &freshest_target_ref(repo, parent_branch), head)
                    .ok()
            })
        });
        let journeys = changed
            .as_deref()
            .map(|paths| crate::qa_pass::catalog_journeys_for(repo, paths))
            .unwrap_or_default();
        let mut eligibility = delivery_eligibility(task, &qa, changed.as_deref(), &journeys);
        if let Some(reason) = requested.map(str::trim).filter(|reason| !reason.is_empty()) {
            // cas-74284: the supervisor's request is the reason on record,
            // whatever the diff alone would have said.
            eligibility
                .reasons
                .push(format!("requested by supervisor: {reason}"));
        } else if !eligibility.is_eligible() {
            // GH #1001 (cas-627c): once a round is on record the delivery is
            // gated (`gate_applies`), so a re-park must open the next round
            // even when this park's diff alone looks non-user-facing: a
            // test-only fix for a rejection, or a target that moved to a
            // branch already holding the reviewed change. Deciding afresh
            // left the merge refused with no round a reviewer could record.
            let prior = cas_store::list_qa_passes(&self.cas_root, &task.id).unwrap_or_default();
            // cas-7877: rounds that never reached a verdict were opened for an
            // earlier tip that looked user-facing (a lane stacked on another
            // task's UI commit, say). They are not a review this tip owes.
            // Withdraw the open one instead of re-dispatching it as a
            // "re-review". A recorded verdict (passed, failed, waived) keeps
            // the GH #1001 rule: the next park is reviewed again.
            if changed.is_some()
                && !prior.is_empty()
                && !prior.iter().any(|pass| {
                    !pass.is_withdrawn()
                        && matches!(
                            pass.state,
                            cas_types::QaPassState::Passed
                                | cas_types::QaPassState::Failed
                                | cas_types::QaPassState::Waived
                        )
                })
            {
                return self.withdraw_unreviewed_qa_round(task, repo, head);
            }
            if !crate::qa_pass::gate_applies(task, &qa, &prior) {
                // cas-2ee2: a required GitHub check must still turn green for
                // a delivery that needs no independent QA.
                if let Some(head) = head {
                    crate::qa_pass::github_gate::publish_not_required(&self.cas_root, repo, head);
                }
                return None;
            }
            let latest = prior.iter().find(|pass| !pass.is_withdrawn())?;
            eligibility.reasons.push(format!(
                "re-review after round {} ({})",
                latest.round, latest.state
            ));
        }
        let reasons = eligibility.reasons.join(", ");
        let Some(head) = head else {
            return Some(QaDispatchStatus::required(format!(
                "\n\nINDEPENDENT QA REQUIRED ({reasons}), but the tip of {branch} could not be resolved, \
                 so no QA pass was dispatched. Push the branch and close again."
            )));
        };
        let now = chrono::Utc::now();
        // cas-624f: after `max_rounds` rejections Cassy escalates instead of
        // opening another round, and the escalation offers "a fix plan with
        // the implementer". The supervisor's explicit qa_request (with that
        // plan as its reason) is the executable form of that option: it opens
        // exactly one more round. A worker's close stays capped, so a further
        // rejection escalates again.
        let max_rounds = if requested.is_some_and(|reason| !reason.trim().is_empty()) {
            qa.max_rounds.max(failed_qa_rounds(&self.cas_root, &task.id) + 1)
        } else {
            qa.max_rounds
        };
        let new = NewQaPass {
            task_id: &task.id,
            implementer_agent_id: implementer,
            branch,
            bound_head: head,
            deadline_at: now + chrono::Duration::minutes(i64::from(qa.pass_timeout_mins.max(1))),
            max_rounds,
        };
        // cas-ce39: a new tip retires the round open for the old one. Report
        // which, so its work item is cancelled and a reviewer who had claimed
        // it is told to stop, instead of reviewing a dead head.
        let opened = cas_store::open_qa_pass_reporting_superseded(&self.cas_root, &new, now);
        let (outcome, superseded) = match opened {
            Ok(outcome) => outcome,
            Err(error) => {
                tracing::error!(task_id = %task.id, error = %error, "cas-619f: QA pass could not be opened");
                return Some(QaDispatchStatus::required(format!(
                    "\n\nINDEPENDENT QA REQUIRED ({reasons}), but Cassy could not open the pass: {error}. \
                     The merge stays blocked until a pass records a verdict for {head}."
                )));
            }
        };
        let new_round_id = match &outcome {
            QaPassOpen::Dispatched(pass) => Some(pass.id.clone()),
            _ => None,
        };
        let satisfied = match &outcome {
            QaPassOpen::AlreadySatisfied(pass) => Some(pass.clone()),
            _ => None,
        };
        let mut status = match outcome {
            QaPassOpen::Dispatched(pass) => {
                self.materialize_qa_round(task, pass, &reasons, parent_branch, &config, location)
            }
            QaPassOpen::AlreadyOpen(pass) => {
                if pass.qa_task_id.is_none() {
                    // A previous park opened the round but crashed before
                    // its work item existed; finish the job.
                    self.materialize_qa_round(
                        task,
                        pass,
                        &reasons,
                        parent_branch,
                        &config,
                        location,
                    )
                } else {
                    format!(
                        "\n\nINDEPENDENT QA PENDING: pass {} (round {}) for {} is {}{}; QA task {}. \
                         The {} waits for its verdict.",
                        pass.id,
                        pass.round,
                        pass.head8(),
                        pass.state,
                        pass.reviewer_agent_id
                            .as_deref()
                            .map(|reviewer| format!(" by {reviewer}"))
                            .unwrap_or_default(),
                        pass.qa_task_id.as_deref().unwrap_or("-"),
                        location.gate(),
                    )
                }
            }
            QaPassOpen::AlreadySatisfied(pass) => format!(
                "\n\nINDEPENDENT QA {}: pass {} covers {}. Ready for the supervisor to {}.",
                if pass.state == cas_types::QaPassState::Waived {
                    "WAIVED"
                } else {
                    "PASSED"
                },
                pass.id,
                pass.head8(),
                location.gate(),
            ),
            QaPassOpen::Escalate {
                failed_rounds,
                latest,
            } => {
                self.escalate_qa_rounds(task, failed_rounds, &latest);
                format!(
                    "\n\nINDEPENDENT QA ESCALATED: {failed_rounds} rejected rounds (latest {}). \
                     Cassy will not open another round by itself; the supervisor decides: a fix plan \
                     (`{prefix}verification action=qa_request task_id={task} summary=\"<fix plan>\"` opens \
                     one more round), a waiver, or cancel.",
                    latest.id,
                    prefix = crate::mcp::tools::core::guidance::supervisor_prefix(),
                    task = task.id,
                )
            }
        };
        if let Some(retired) = superseded {
            // Read the new round back after materialization so its QA task
            // id (linked just above) is known.
            let next = new_round_id.as_deref().and_then(|id| {
                cas_store::list_qa_passes(&self.cas_root, &task.id)
                    .ok()?
                    .into_iter()
                    .find(|pass| pass.id == id)
            });
            status.push_str(&self.retire_superseded_qa_round(&retired, next.as_ref()));
        }
        // cas-2ee2: mirror the round covering this head onto GitHub.
        if let Some(current) = cas_store::list_qa_passes(&self.cas_root, &task.id)
            .unwrap_or_default()
            .into_iter()
            .find(|pass| pass.bound_head == head && !pass.is_withdrawn())
        {
            crate::qa_pass::github_gate::publish_pass_status(&self.cas_root, &current);
        }
        Some(QaDispatchStatus {
            text: status,
            satisfied,
            withdrawn: false,
        })
    }

    /// cas-7877: the re-parked tip is not user-facing and no earlier round
    /// reached a verdict. Withdraw the round still open for an earlier tip,
    /// cancel its work item (telling a reviewer who claimed it to stop), and
    /// report that no review is owed. `None` when there was nothing open.
    fn withdraw_unreviewed_qa_round(
        &self,
        task: &Task,
        repo: &Path,
        head: Option<&str>,
    ) -> Option<QaDispatchStatus> {
        if let Some(head) = head {
            crate::qa_pass::github_gate::publish_not_required(&self.cas_root, repo, head);
        }
        let tip = head.map(|head| head.get(..8).unwrap_or(head)).unwrap_or("this tip");
        let reason = format!(
            "re-parked at {tip}, which has no user-facing change; the round was opened for an earlier tip and never reviewed"
        );
        let pass = match cas_store::withdraw_open_qa_pass(
            &self.cas_root,
            &task.id,
            &reason,
            true,
            chrono::Utc::now(),
        ) {
            Ok(Some(pass)) => pass,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(task_id = %task.id, error = %error, "cas-7877: unreviewed QA round could not be withdrawn");
                return Some(QaDispatchStatus::required(format!(
                    "\n\nINDEPENDENT QA: this tip is not user-facing, but the round open for an earlier tip \
                     could not be withdrawn ({error}). A supervisor can qa_waive it."
                )));
            }
        };
        let cancelled = self.cancel_withdrawn_qa_task(&pass, &reason);
        let notified = pass
            .reviewer_agent_id
            .as_deref()
            .map(|reviewer| {
                self.notify_superseded_reviewer(
                    &pass,
                    reviewer,
                    "no new round: the re-parked tip has no user-facing change",
                )
            })
            .unwrap_or_default();
        Some(QaDispatchStatus {
            text: format!(
                "\n\nINDEPENDENT QA WITHDRAWN: round {} (pass {}) for {} was opened for an earlier tip and \
                 never reviewed; {tip} has no user-facing change, so no review is owed.{cancelled}{notified}",
                pass.round,
                pass.id,
                pass.head8(),
            ),
            satisfied: None,
            withdrawn: true,
        })
    }

    /// cas-ce39: a re-park at a new tip superseded `retired`. Cancel its QA
    /// work item (pointing at the new one) and, when a reviewer had claimed
    /// it, message that reviewer with the new round, so the old review stops
    /// instead of producing a verdict for a dead head. Returns the sentence
    /// appended to the park response.
    fn retire_superseded_qa_round(&self, retired: &QaPass, next: Option<&QaPass>) -> String {
        let next_desc = match next {
            Some(next) => format!(
                "round {} (pass {}, QA task {}) for {}",
                next.round,
                next.id,
                next.qa_task_id.as_deref().unwrap_or("-"),
                next.head8()
            ),
            None => "no new round was opened (see above)".to_string(),
        };
        let reason = format!(
            "Independent QA round {} for {} @{} superseded: the delivery re-parked at a new tip; \
             the review continues as {next_desc}.",
            retired.round,
            retired.task_id,
            retired.head8()
        );
        let cancelled = self.cancel_qa_task_with_reason(
            retired,
            &reason,
            next.and_then(|next| next.qa_task_id.clone()),
        );
        let (held_by, notified) = match retired.reviewer_agent_id.as_deref() {
            Some(reviewer) => (
                format!("claimed by {reviewer}"),
                self.notify_superseded_reviewer(retired, reviewer, &next_desc),
            ),
            None => ("pending, unclaimed".to_string(), String::new()),
        };
        format!(
            "\n\nSUPERSEDED: QA round {} (pass {}, {held_by}) for {} no longer binds this delivery; \
             the review continues as {next_desc}.{cancelled}{notified}",
            retired.round,
            retired.id,
            retired.head8(),
        )
    }

    /// Tell the reviewer of a superseded round to stop (cas-ce39).
    fn notify_superseded_reviewer(&self, retired: &QaPass, reviewer: &str, next_desc: &str) -> String {
        let queue = match crate::store::open_prompt_queue_store(&self.cas_root) {
            Ok(queue) => queue,
            Err(error) => {
                return format!(
                    " Reviewer {reviewer} could NOT be told (prompt queue unavailable: {error}); message them."
                );
            }
        };
        let body = format!(
            "STOP reviewing {task} @{old}: independent QA round {round} (pass {pass}) is superseded \
             because the implementer re-parked the delivery at a new tip. Do not record a verdict for \
             the old tip. Its QA task is cancelled. The review continues as {next_desc}; the \
             supervisor assigns it.",
            task = retired.task_id,
            old = retired.head8(),
            round = retired.round,
            pass = retired.id,
        );
        let source = format!(
            "{}superseded:{}",
            super::supervisor_push::QA_DISPATCH_SOURCE_PREFIX,
            retired.id
        );
        let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
        match queue.enqueue_idempotent(
            &source,
            reviewer,
            &body,
            factory_session.as_deref(),
            Some(&format!("QA round superseded: {}", retired.task_id)),
            Some(cas_store::NotificationPriority::High),
            &source,
            Some(&cas_store::QueueOrigin::Daemon),
        ) {
            Ok(_) => format!(" Reviewer {reviewer} told to stop."),
            Err(error) => {
                tracing::warn!(pass_id = %retired.id, error = %error, "cas-ce39: superseded-round notice not queued");
                format!(" Reviewer {reviewer} could NOT be told ({error}); message them.")
            }
        }
    }

    /// cas-619f merge gate for `worktree_merge task_id=…`: the refusal text
    /// when the delivery's current branch tip lacks a passed/waived round.
    pub(crate) fn independent_qa_merge_refusal(
        &self,
        task_id: &str,
        merge_id: &str,
        repo: &Path,
    ) -> Option<String> {
        let config = crate::config::Config::load(&self.cas_root).ok()?;
        let qa = config.qa();
        let task = self.open_task_store().ok()?.get(task_id).ok()?;
        let passes = cas_store::list_qa_passes(&self.cas_root, task_id).unwrap_or_default();
        if !crate::qa_pass::gate_applies(&task, &qa, &passes) {
            return None;
        }
        let branch = if merge_id.starts_with("factory/") {
            merge_id.to_string()
        } else {
            task.deliverables
                .parked_branch
                .clone()
                .or_else(|| task.assignee.as_deref().map(|name| format!("factory/{name}")))
                .unwrap_or_else(|| format!("factory/{merge_id}"))
        };
        let Some(head) = super::close_ops::resolve_branch_sha(repo, &branch) else {
            return Some(format!(
                "INDEPENDENT QA REQUIRED before {task_id} merges, but {branch} could not be resolved in {}; \
                 Cassy cannot prove which tip was reviewed.",
                repo.display()
            ));
        };
        crate::qa_pass::merge_gate(&task, &qa, &passes, &head).err()
    }

    /// cas-619f close backstop: an independently reviewed tip must be
    /// contained in the target branch. Returns the refusal when not,
    /// dispatching a round first if none is open so the task can progress.
    ///
    /// cas-5c38 (GH #999): a delivery merged before it was ever closed (the
    /// domdms cas-0019 shape) used to deadlock here. The backstop dispatched
    /// a review for code already on trunk, and `qa_waive` then refused for
    /// lack of a parked tip. Now:
    /// - the delivered commit is the parked anchor, the close's
    ///   `commit_receipt`, or the implementer's branch tip only when that tip
    ///   is itself merged;
    /// - a live supervisor's override with a reason records a waiver against
    ///   that commit;
    /// - code already on trunk is never sent for review.
    pub(crate) fn independent_qa_close_gate(
        &self,
        task: &Task,
        repo: &Path,
        target_branch: &str,
        commit_receipt: Option<&str>,
        supervisor_waiver: Option<&str>,
    ) -> QaCloseGate {
        let Ok(config) = crate::config::Config::load(&self.cas_root) else {
            return QaCloseGate::Clear;
        };
        let qa = config.qa();
        let Some(implementer) = task.assignee.as_deref() else {
            return QaCloseGate::Clear;
        };
        if !qa.independent_pass {
            return QaCloseGate::Clear;
        }
        let passes = cas_store::list_qa_passes(&self.cas_root, &task.id).unwrap_or_default();
        let branch = task
            .deliverables
            .parked_branch
            .clone()
            .unwrap_or_else(|| format!("factory/{implementer}"));
        // A commit the task itself stands behind: the parked anchor or the
        // close's receipt.
        let recorded_head = task
            .deliverables
            .factory_branch_anchor
            .clone()
            .or_else(|| commit_receipt.and_then(|receipt| resolve_commit(repo, receipt)));
        let head = recorded_head.clone().or_else(|| {
            // The live branch tip is the delivery only while it is itself
            // merged: months later it carries unrelated work.
            super::close_ops::resolve_branch_sha(repo, &branch)
                .filter(|tip| is_ancestor(repo, tip, target_branch))
        });
        if passes.iter().any(|pass| {
            qa_pass_covers_integrated_delivery(
                repo,
                pass,
                head.as_deref(),
                commit_receipt,
                target_branch,
            )
        }) {
            return QaCloseGate::Clear;
        }
        // Judge from what the delivery actually integrated, so a
        // docs/test/CI-only change closes freely even when it merged before
        // it ever parked through the gate.
        let changed = head
            .as_deref()
            .and_then(|head| crate::qa_pass::integrated_paths(repo, head, target_branch));
        // cas-2387: a no-code task owes no review only while it delivered no
        // user-facing code. Then any round an earlier close opened for it is
        // withdrawn, so it cannot hold the close.
        // Only a recorded delivery commit is evidence of the task's own code.
        // A no-code task's branch tip usually sits on its base, and the
        // first merge above that base is someone else's change.
        let own_changed = recorded_head.as_ref().and(changed.as_deref());
        if crate::qa_pass::no_code_without_surface(task, own_changed) {
            let reason = "no-code task delivered no user-facing code";
            if let Ok(Some(pass)) = cas_store::withdraw_open_qa_pass(
                &self.cas_root,
                &task.id,
                reason,
                true,
                chrono::Utc::now(),
            ) {
                self.cancel_withdrawn_qa_task(&pass, reason);
            }
            return QaCloseGate::Clear;
        }
        if passes.iter().all(|pass| pass.is_withdrawn()) {
            let journeys = changed
                .as_deref()
                .map(|paths| crate::qa_pass::catalog_journeys_for(repo, paths))
                .unwrap_or_default();
            if !delivery_eligibility(task, &qa, changed.as_deref(), &journeys).is_eligible() {
                return QaCloseGate::Clear;
            }
        } else if !crate::qa_pass::gate_applies(task, &qa, &passes) {
            return QaCloseGate::Clear;
        }

        if let Some(reason) = supervisor_waiver.map(str::trim).filter(|r| !r.is_empty()) {
            let Some(head) = head.as_deref() else {
                return QaCloseGate::Waived(format!(
                    "✅ DECISION Independent QA waived by supervisor override at close; no delivered \
                     commit resolved to bind it to (pass commit_receipt to record one). Reason: {reason}"
                ));
            };
            let supervisor = self
                .get_agent_id()
                .unwrap_or_else(|_| "supervisor".to_string());
            return match cas_store::waive_qa_pass(
                &self.cas_root,
                &task.id,
                &supervisor,
                implementer,
                &branch,
                head,
                reason,
                chrono::Utc::now(),
            ) {
                Ok(pass) => QaCloseGate::Waived(format!(
                    "✅ DECISION Independent QA waived by supervisor {supervisor} at close for @{} \
                     (pass {}), already merged into {target_branch}. Reason: {reason}",
                    pass.head8(),
                    pass.id,
                )),
                Err(error) => QaCloseGate::Refuse(format!(
                    "INDEPENDENT QA REQUIRED: {} is user-facing and the supervisor override could not \
                     record its waiver: {error}",
                    task.id
                )),
            };
        }

        // Review coverage and integration proof are separate facts. A failed
        // integration proof must not claim that an existing verdict is missing.
        if let Some(pass) = passes.iter().find(|pass| {
            pass.state.satisfies_gate() && head.as_deref() == Some(pass.bound_head.as_str())
        }) {
            return QaCloseGate::Refuse(qa_delivery_not_proven(task, pass, target_branch));
        }

        let remedy = "A live supervisor closes it with supervisor_override=true, a reason and \
             commit_receipt=<merged sha>; the waiver is recorded against that commit";
        let Some(head) = head else {
            return QaCloseGate::Refuse(unresolved_delivery_refusal(
                &task.id,
                target_branch,
                &branch,
                super::close_ops::resolve_branch_sha(repo, &branch).as_deref(),
                remedy,
            ));
        };
        if let Some(trunk) = trunk_containing(repo, &head) {
            return QaCloseGate::Refuse(format!(
                "INDEPENDENT QA REQUIRED: {} is user-facing and its delivery @{} is already on trunk \
                 {trunk} with no passed or waived QA round. Cassy does not dispatch a review of code \
                 that is already on trunk, so no QA pass was opened. {remedy}.",
                task.id,
                &head[..head.len().min(8)],
            ));
        }
        let location = close_delivery_location(repo, &head, target_branch);
        let dispatch = self.independent_qa_for_paths(
            task,
            repo,
            target_branch,
            &branch,
            Some(&head),
            changed,
            location,
            None,
        );
        if dispatch.as_ref().is_some_and(|status| status.withdrawn) {
            // cas-7877: the only round was an unreviewed one for an earlier,
            // user-facing-looking tip; the delivered change owes no review.
            return QaCloseGate::Clear;
        }
        if let Some(pass) = dispatch
            .as_ref()
            .and_then(|status| status.satisfied.as_ref())
        {
            return if qa_pass_covers_integrated_delivery(
                repo,
                pass,
                Some(&head),
                commit_receipt,
                target_branch,
            ) {
                QaCloseGate::Clear
            } else {
                QaCloseGate::Refuse(qa_delivery_not_proven(task, pass, target_branch))
            };
        }
        let dispatch = dispatch.map(|status| status.text).unwrap_or_default();
        QaCloseGate::Refuse(format!(
            "INDEPENDENT QA REQUIRED: {} is user-facing and no passed or waived QA round covers \
             its delivered tip @{} for {target_branch}. The {} waits for the reviewer's verdict \
             (or a logged supervisor waiver: `{}verification action=qa_waive task_id={} summary=\"...\"`).{dispatch}",
            task.id,
            &head[..head.len().min(8)],
            location.gate(),
            crate::mcp::tools::core::guidance::supervisor_prefix(),
            task.id,
        ))
    }

    fn materialize_qa_round(
        &self,
        task: &Task,
        pass: QaPass,
        reasons: &str,
        parent_branch: &str,
        config: &crate::config::Config,
        location: QaDeliveryLocation<'_>,
    ) -> String {
        let artifacts_root =
            crate::config::project_factory_artifacts_root(&self.cas_root, &crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref()));
        let ledger_dir = round_dir(&artifacts_root, &pass);
        let qa_task_id = match self.create_qa_task(task, &pass, reasons, &ledger_dir, parent_branch) {
            Ok(id) => id,
            Err(error) => {
                tracing::error!(task_id = %task.id, pass_id = %pass.id, error = %error, "cas-619f: QA task not created");
                return format!(
                    "\n\nINDEPENDENT QA REQUIRED: pass {} opened for {}, but its QA task could not be created ({error}). \
                     Closing again retries; the merge stays blocked.",
                    pass.id,
                    pass.head8()
                );
            }
        };
        if let Err(error) = cas_store::set_qa_task(&self.cas_root, &pass.id, &qa_task_id) {
            tracing::error!(task_id = %task.id, pass_id = %pass.id, error = %error, "cas-619f: QA task not linked to its pass");
        }
        let mut handoff = "queued for the supervisor".to_string();
        match crate::store::open_prompt_queue_store(&self.cas_root) {
            Ok(queue) => {
                if let Err(error) = super::supervisor_push::emit_qa_dispatch_handoff(
                    queue.as_ref(),
                    &pass.id,
                    &task.id,
                    &qa_task_id,
                    pass.round,
                    &pass.bound_head,
                    pass.deadline_at,
                    &pass.implementer_agent_id,
                    reasons,
                    location,
                ) {
                    tracing::warn!(task_id = %task.id, error = %error, "cas-619f: QA handoff not queued");
                    handoff = "NOT queued — message the supervisor".to_string();
                }
            }
            Err(error) => {
                tracing::warn!(task_id = %task.id, error = %error, "cas-619f: prompt queue unavailable for QA handoff");
                handoff = "NOT queued — message the supervisor".to_string();
            }
        }
        format!(
            "\n\nINDEPENDENT QA DISPATCHED ({reasons}): pass {} round {} for {}; QA task {qa_task_id} ({handoff}). \
             A different agent reviews the running build; the {} waits for its verdict, and a rejection \
             returns this task to you with the ledger at {}/LEDGER.md.",
            pass.id,
            pass.round,
            pass.head8(),
            location.gate(),
            ledger_dir.display(),
        )
    }

    fn create_qa_task(
        &self,
        delivery: &Task,
        pass: &QaPass,
        reasons: &str,
        ledger_dir: &Path,
        parent_branch: &str,
    ) -> Result<String, String> {
        let task_store = self
            .open_task_store()
            .map_err(|error| format!("task store unavailable: {}", error.message))?;
        let id = task_store
            .generate_id()
            .map_err(|error| format!("id generation failed: {error}"))?;
        let epic = task_store
            .get_parent_epic(&delivery.id)
            .map_err(|error| format!("parent epic lookup failed: {error}"))?
            .map(|epic| epic.id);
        let reasons_list: Vec<String> = reasons.split(", ").map(ToOwned::to_owned).collect();
        let mut qa_task = Task::new(id.clone(), qa_task_title(delivery, pass));
        qa_task.scope = crate::types::Scope::Project;
        qa_task.origin_project = delivery.origin_project.clone();
        qa_task.description =
            qa_task_description(delivery, pass, &reasons_list, ledger_dir, parent_branch);
        qa_task.acceptance_criteria = "LEDGER.md with journeys, correctness and polish sections; \
             every finding cites a trace action and a screenshot; desktop+phone, light+dark \
             captures; visual-qa.mjs --strict output; verdict recorded with verification action=qa_record."
            .to_string();
        qa_task.execution_note = Some("no-code".to_string());
        qa_task.labels = vec![QA_PASS_LABEL.to_string()];
        qa_task.priority = delivery.priority;
        qa_task.external_ref = Some(format!("{}/LEDGER.md", ledger_dir.display()));
        task_store
            .create_atomic(&qa_task, &[], epic.as_deref(), Some("cas-qa-dispatch"))
            .map_err(|error| format!("create failed: {error}"))?;
        let related = Dependency::new(id.clone(), delivery.id.clone(), DependencyType::Related);
        if let Err(error) = task_store.add_dependency(&related) {
            tracing::warn!(qa_task = %id, delivery = %delivery.id, error = %error, "cas-619f: related link not recorded");
        }
        Ok(id)
    }

    fn escalate_qa_rounds(&self, task: &Task, failed_rounds: u32, latest: &QaPass) {
        let Ok(queue) = crate::store::open_prompt_queue_store(&self.cas_root) else {
            return;
        };
        let body = crate::prompt_revalidation::attach_blocker_envelope(
            &format!(
                "Independent QA rejected {task} {failed_rounds} times (latest pass {pass}, ledger {ledger}). \
                 Cassy stopped opening rounds. Decide: a fix plan with the implementer (once it is \
                 pushed and parked, `{prefix}verification action=qa_request task_id={task} \
                 summary=\"<fix plan>\"` opens one more round), a logged waiver \
                 (`{prefix}verification action=qa_waive task_id={task} summary=\"...\"`), or cancel.",
                task = task.id,
                prefix = crate::mcp::tools::core::guidance::supervisor_prefix(),
                pass = latest.id,
                ledger = latest.ledger_path.as_deref().unwrap_or("-"),
            ),
            task.assignee.as_deref().unwrap_or("worker"),
            Some(&task.id),
        );
        let source = format!(
            "{}escalate:{}",
            super::supervisor_push::QA_DISPATCH_SOURCE_PREFIX,
            latest.id
        );
        let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
        if let Err(error) = queue.enqueue_idempotent(
            &source,
            "supervisor",
            &body,
            factory_session.as_deref(),
            Some(&format!("Independent QA escalated: {}", task.id)),
            Some(cas_store::NotificationPriority::High),
            &source,
            Some(&cas_store::QueueOrigin::Daemon),
        ) {
            tracing::warn!(task_id = %task.id, error = %error, "cas-619f: QA escalation not queued");
        }
    }
}

impl CasCore {
    /// Cancel the QA work item of a withdrawn round (cas-5c38) so no
    /// reviewer is spawned for a pass that no longer binds the delivery.
    pub(crate) fn cancel_withdrawn_qa_task(&self, pass: &QaPass, reason: &str) -> String {
        let close_reason = format!(
            "Independent QA round {} for {} @{} withdrawn: {}",
            pass.round,
            pass.task_id,
            pass.head8(),
            reason.trim()
        );
        self.cancel_qa_task_with_reason(pass, &close_reason, None)
    }

    /// Cancel a round's QA work item with a full close reason, pointing at
    /// the work item that replaces it when there is one (cas-ce39).
    pub(crate) fn cancel_qa_task_with_reason(
        &self,
        pass: &QaPass,
        close_reason: &str,
        superseded_by: Option<String>,
    ) -> String {
        let Some(qa_task_id) = pass.qa_task_id.as_deref() else {
            return String::new();
        };
        let Ok(store) = self.open_task_store() else {
            return format!(" QA task {qa_task_id} could not be closed (task store unavailable).");
        };
        let Ok(mut qa_task) = store.get(qa_task_id) else {
            return String::new();
        };
        if qa_task.is_terminal() {
            return String::new();
        }
        let now = chrono::Utc::now();
        qa_task.status = TaskStatus::Cancelled;
        qa_task.closed_at = Some(now);
        qa_task.terminal_outcome =
            Some(cas_types::TaskTerminalOutcome::Cancelled { superseded_by });
        qa_task.close_reason = Some(close_reason.to_string());
        qa_task.pending_verification = false;
        match store.update(&qa_task) {
            Ok(_) => format!(" QA task {qa_task_id} cancelled."),
            Err(error) => format!(" QA task {qa_task_id} could not be cancelled: {error}"),
        }
    }
}

/// The refusal for a user-facing close whose delivering commit cannot be
/// resolved: no parked anchor, no receipt, and a branch tip that is not on the
/// target (cas-08f9). The wording must never claim a merge the target does
/// not contain: an unmerged branch tip is sent back to park for merge and QA
/// with its tip as the receipt, and only a missing branch is described as an
/// out-of-band merge.
pub(crate) fn unresolved_delivery_refusal(
    task_id: &str,
    target_branch: &str,
    branch: &str,
    branch_tip: Option<&str>,
    remedy: &str,
) -> String {
    match branch_tip {
        Some(tip) => format!(
            "INDEPENDENT QA REQUIRED: {task_id} is user-facing and has no QA round, and Cassy \
             cannot tell which commit delivered it: it is not parked, and {branch} @{} is not \
             contained in {target_branch}, so it is not merged. If that tip is this task's \
             delivery, close again with commit_receipt={tip}: it parks for merge and opens the \
             QA round. If the delivery already merged another way, close with \
             commit_receipt=<merged sha>. {remedy}.",
            &tip[..tip.len().min(8)],
        ),
        None => format!(
            "INDEPENDENT QA REQUIRED: {task_id} is user-facing and has no QA round, and Cassy \
             cannot tell which commit delivered it: it never parked and {branch} does not \
             resolve here. If it merged into {target_branch}, close with \
             commit_receipt=<merged sha>. {remedy}."
        ),
    }
}

#[cfg(test)]
mod delivery_location_tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn close_dispatch_location_follows_target_ancestry_gh_1026() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(repo)
                .env("GIT_AUTHOR_NAME", "CAS Test")
                .env("GIT_AUTHOR_EMAIL", "cas@example.test")
                .env("GIT_COMMITTER_NAME", "CAS Test")
                .env("GIT_COMMITTER_EMAIL", "cas@example.test")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-q", "-b", "epic/ui"]);
        git(&["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&["checkout", "-q", "-b", "factory/worker"]);
        git(&["commit", "-q", "--allow-empty", "-m", "delivery"]);
        let head = git(&["rev-parse", "HEAD"]);

        assert!(matches!(
            close_delivery_location(repo, &head, "epic/ui"),
            QaDeliveryLocation::UnmergedFrom("epic/ui")
        ));
        // The remote target may contain the delivery while this checkout's
        // local epic ref is stale.
        git(&["update-ref", "refs/remotes/origin/epic/ui", &head]);
        assert!(matches!(
            close_delivery_location(repo, &head, "epic/ui"),
            QaDeliveryLocation::ContainedIn("epic/ui")
        ));
        git(&["checkout", "-q", "epic/ui"]);
        git(&["merge", "-q", "--no-ff", "-m", "merge", "factory/worker"]);
        assert!(matches!(
            close_delivery_location(repo, &head, "epic/ui"),
            QaDeliveryLocation::ContainedIn("epic/ui")
        ));
    }
}

#[cfg(test)]
mod squash_close_tests {
    use super::*;
    use crate::store::{
        open_agent_store, open_rule_store, open_skill_store, open_store, open_task_store,
    };
    use crate::test_support::TestEnvGuard;
    use cas_types::{Agent, AgentRole, QaPassState, QaVerdict, TaskRisk, TaskStatus};
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "CAS Test")
            .env("GIT_AUTHOR_EMAIL", "cas@example.test")
            .env("GIT_COMMITTER_NAME", "CAS Test")
            .env("GIT_COMMITTER_EMAIL", "cas@example.test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn squash_fixture(
        env: &mut TestEnvGuard,
        state: QaPassState,
    ) -> (tempfile::TempDir, CasCore, Task, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let cas_dir = repo.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        env.set("CAS_ROOT", &cas_dir);
        env.set("XDG_CONFIG_HOME", env.home().join(".config"));
        std::fs::write(
            cas_dir.join("config.toml"),
            "[verification]\nenabled=false\n[qa]\nevidence_gate=false\nindependent_pass=true\n",
        )
        .unwrap();
        open_store(&cas_dir).unwrap().init().unwrap();
        open_rule_store(&cas_dir).unwrap().init().unwrap();
        open_skill_store(&cas_dir).unwrap().init().unwrap();
        let tasks = open_task_store(&cas_dir).unwrap();
        tasks.init().unwrap();
        let agents = open_agent_store(&cas_dir).unwrap();
        agents.init().unwrap();
        agents
            .register(&Agent::new_with_role(
                "test-worker-session".into(),
                "worker".into(),
                AgentRole::Worker,
            ))
            .unwrap();
        let core = CasCore::with_daemon(cas_dir.clone(), None, None);
        core.set_agent_id_for_testing("test-worker-session".into());
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "seed\n").unwrap();
        git(repo, &["add", "README.md"]);
        git(repo, &["commit", "-q", "-m", "seed"]);
        let mut task = Task::new("cas-ui01".into(), "Composer spacing".into());
        task.assignee = Some("worker".into());
        task.risk = vec![TaskRisk::None];
        task.demo_statement = "Open composer and see even spacing".into();
        git(repo, &["checkout", "-q", "-b", "factory/worker"]);
        std::fs::create_dir_all(repo.join("web")).unwrap();
        std::fs::write(repo.join("web/composer.css"), ".composer{gap:8px}\n").unwrap();
        git(repo, &["add", "web/composer.css"]);
        git(repo, &["commit", "-q", "-m", "feat(cas-ui01): spacing"]);
        std::fs::write(repo.join("web/composer.js"), "export const ready = true;\n").unwrap();
        git(repo, &["add", "web/composer.js"]);
        git(repo, &["commit", "-q", "-m", "feat(cas-ui01): ready"]);
        let tip = git(repo, &["rev-parse", "HEAD"]);
        task.status = TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(tip.clone());
        task.deliverables.parked_branch = Some("factory/worker".into());
        tasks.add(&task).unwrap();
        let now = chrono::Utc::now();
        match state {
            QaPassState::Waived => {
                cas_store::waive_qa_pass(
                    &cas_dir,
                    &task.id,
                    "supervisor",
                    "worker",
                    "factory/worker",
                    &tip,
                    "Reviewed approved delivery",
                    now,
                )
                .unwrap();
            }
            QaPassState::Passed => {
                cas_store::open_qa_pass(
                    &cas_dir,
                    &NewQaPass {
                        task_id: &task.id,
                        implementer_agent_id: "worker",
                        branch: "factory/worker",
                        bound_head: &tip,
                        deadline_at: now + chrono::Duration::minutes(30),
                        max_rounds: 3,
                    },
                    now,
                )
                .unwrap();
                cas_store::claim_qa_pass(&cas_dir, &task.id, "reviewer", now).unwrap();
                cas_store::resolve_qa_pass(
                    &cas_dir,
                    &task.id,
                    "reviewer",
                    QaVerdict::Approved,
                    "Independent review passed",
                    None,
                    "/fixture/LEDGER.md",
                    now,
                )
                .unwrap();
            }
            _ => panic!("fixture needs a satisfying round"),
        }
        git(repo, &["checkout", "-q", "main"]);
        // Target has unrelated changes: equality is over delivered files,
        // rather than the entire target tree.
        std::fs::write(repo.join("other.txt"), "another task\n").unwrap();
        git(repo, &["add", "other.txt"]);
        git(repo, &["commit", "-q", "-m", "other task"]);
        git(repo, &["merge", "-q", "--squash", "factory/worker"]);
        git(repo, &["commit", "-q", "-m", "squash(cas-ui01): composer"]);
        let squash = git(repo, &["rev-parse", "HEAD"]);
        assert!(
            !is_ancestor(repo, &tip, "main"),
            "fixture must rewrite commit identity"
        );
        git(repo, &["checkout", "-q", "factory/worker"]);
        (dir, core, task, squash)
    }

    async fn worker_close_after_squash(state: QaPassState) {
        let mut env = TestEnvGuard::temp_home();
        let (dir, core, task, squash) = squash_fixture(&mut env, state);
        let request = TaskCloseRequest {
            id: task.id.clone(),
            reason: Some("Squash delivery landed".into()),
            commit_receipt: Some(squash),
            supervisor_override: None,
            stranded_branch_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
        };
        let result = core.cas_task_close(Parameters(request)).await.unwrap();
        let text = result
            .content
            .into_iter()
            .filter_map(|content| match content.raw {
                rmcp::model::RawContent::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            open_task_store(&dir.path().join(".cas"))
                .unwrap()
                .get(&task.id)
                .unwrap()
                .status,
            TaskStatus::Closed,
            "{text}"
        );
        assert!(!text.contains("INDEPENDENT QA REQUIRED"), "{text}");
        let passes = cas_store::list_qa_passes(&dir.path().join(".cas"), &task.id).unwrap();
        assert_eq!(passes.len(), 1, "no new round or waiver is needed");
        assert_eq!(passes[0].state, state);
        assert_eq!(
            passes[0].bound_head,
            task.deliverables.factory_branch_anchor.unwrap()
        );
    }

    #[tokio::test]
    async fn worker_squash_close_honors_waived_round_cas_fe7b() {
        worker_close_after_squash(QaPassState::Waived).await;
    }

    #[tokio::test]
    async fn worker_squash_close_honors_passed_round_cas_fe7b() {
        worker_close_after_squash(QaPassState::Passed).await;
    }
    #[test]
    fn squash_qa_does_not_clear_changed_delivery_content_cas_fe7b() {
        for state in [QaPassState::Passed, QaPassState::Waived] {
            let mut env = TestEnvGuard::temp_home();
            let (dir, core, task, _squash) = squash_fixture(&mut env, state);
            let repo = dir.path();
            git(repo, &["checkout", "-q", "main"]);
            // The last pre-squash commit added JS; changing the earlier CSS
            // must also fail aggregate delivery coverage.
            std::fs::write(repo.join("web/composer.css"), ".composer{gap:2px}\n").unwrap();
            git(repo, &["add", "web/composer.css"]);
            git(repo, &["commit", "-q", "-m", "changed integration"]);
            let wrong = git(repo, &["rev-parse", "HEAD"]);
            match core.independent_qa_close_gate(&task, repo, "main", Some(&wrong), None) {
                QaCloseGate::Refuse(text) => {
                    assert!(
                        text.contains("cannot prove") && !text.contains("INDEPENDENT QA REQUIRED"),
                        "{text}"
                    );
                    assert!(!text.contains("Ready for the supervisor"), "{text}");
                }
                _ => panic!("a verdict for T cannot prove changed delivered files"),
            }
            assert_eq!(
                cas_store::list_qa_passes(&repo.join(".cas"), &task.id)
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[test]
    fn squash_qa_uses_origin_when_local_target_is_stale_cas_fe7b() {
        let mut env = TestEnvGuard::temp_home();
        let (dir, core, task, squash) = squash_fixture(&mut env, QaPassState::Waived);
        let repo = dir.path();
        git(repo, &["update-ref", "refs/remotes/origin/main", &squash]);
        let base = git(repo, &["rev-parse", "main~2"]);
        git(repo, &["branch", "-f", "main", &base]);
        assert!(!is_ancestor(repo, &squash, "main"));
        assert!(matches!(
            core.independent_qa_close_gate(&task, repo, "main", Some(&squash), None),
            QaCloseGate::Clear
        ));
    }

    #[test]
    fn squash_qa_receipt_resolves_reviewed_tip_without_anchor_cas_fe7b() {
        let mut env = TestEnvGuard::temp_home();
        let (dir, core, mut task, squash) = squash_fixture(&mut env, QaPassState::Passed);
        task.deliverables.factory_branch_anchor = None;
        assert!(matches!(
            core.independent_qa_close_gate(&task, dir.path(), "main", Some(&squash), None),
            QaCloseGate::Clear
        ));
    }

    #[test]
    fn squash_qa_does_not_cover_a_newer_task_anchor_cas_fe7b() {
        let mut env = TestEnvGuard::temp_home();
        let (dir, core, mut task, squash) = squash_fixture(&mut env, QaPassState::Passed);
        let repo = dir.path();
        std::fs::write(repo.join("web/new.css"), "new unreviewed feature\n").unwrap();
        git(repo, &["add", "web/new.css"]);
        git(repo, &["commit", "-q", "-m", "new task delivery"]);
        task.deliverables.factory_branch_anchor = Some(git(repo, &["rev-parse", "HEAD"]));
        open_task_store(&repo.join(".cas"))
            .unwrap()
            .update(&task)
            .unwrap();
        match core.independent_qa_close_gate(&task, repo, "main", Some(&squash), None) {
            QaCloseGate::Refuse(text) => {
                assert!(text.contains("INDEPENDENT QA REQUIRED"), "{text}");
                assert!(
                    !text.contains("INDEPENDENT QA PASSED")
                        && !text.contains("INDEPENDENT QA WAIVED"),
                    "{text}"
                );
            }
            _ => panic!("old QA verdict cannot cover a newer recorded delivery"),
        }
    }

    #[test]
    fn squash_qa_rejects_missing_or_unintegrated_receipts_cas_fe7b() {
        let mut env = TestEnvGuard::temp_home();
        let (dir, core, task, _squash) = squash_fixture(&mut env, QaPassState::Waived);
        let repo = dir.path();
        let tree = git(repo, &["rev-parse", "factory/worker^{tree}"]);
        let base = git(repo, &["rev-parse", "factory/worker~2"]);
        let detached = git(
            repo,
            &["commit-tree", &tree, "-p", &base, "-m", "not integrated"],
        );
        for receipt in [
            detached.as_str(),
            "0000000000000000000000000000000000000000",
        ] {
            assert!(
                matches!(
                    core.independent_qa_close_gate(&task, repo, "main", Some(receipt), None),
                    QaCloseGate::Refuse(_)
                ),
                "receipt must identify integrated content: {receipt}"
            );
        }
    }

    #[test]
    fn squash_qa_accepts_patch_equivalent_reviewed_tip_cas_fe7b() {
        let mut env = TestEnvGuard::temp_home();
        let (dir, core, mut task, squash) = squash_fixture(&mut env, QaPassState::Passed);
        let repo = dir.path();
        // Collapse T into one reviewed aggregate patch, with different
        // whitespace from S. Tree identity fails; stable patch-id still holds.
        let base = git(repo, &["rev-parse", "factory/worker~2"]);
        git(repo, &["reset", "-q", "--soft", &base]);
        std::fs::write(
            repo.join("web/composer.js"),
            "export  const ready = true;\n",
        )
        .unwrap();
        git(repo, &["add", "web/composer.js"]);
        git(repo, &["commit", "-q", "-m", "reviewed aggregate delivery"]);
        let reviewed = git(repo, &["rev-parse", "HEAD"]);
        task.deliverables.factory_branch_anchor = Some(reviewed.clone());
        cas_store::waive_qa_pass(
            &repo.join(".cas"),
            &task.id,
            "supervisor",
            "worker",
            "factory/worker",
            &reviewed,
            "Reviewed formatting",
            chrono::Utc::now(),
        )
        .unwrap();
        assert_ne!(
            git(
                repo,
                &[
                    "diff",
                    "--name-only",
                    &reviewed,
                    &squash,
                    "--",
                    "web/composer.js"
                ]
            ),
            ""
        );
        assert!(matches!(
            core.independent_qa_close_gate(&task, repo, "main", Some(&squash), None),
            QaCloseGate::Clear
        ));
        git(repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("web/composer.css"), ".composer{gap:2px}\n").unwrap();
        git(repo, &["add", "web/composer.css"]);
        git(repo, &["commit", "-q", "-m", "changed delivery after squash"]);
        let changed = git(repo, &["rev-parse", "HEAD"]);
        assert!(matches!(core.independent_qa_close_gate(&task, repo, "main", Some(&changed), None), QaCloseGate::Refuse(_)), "a matching older patch must not prove the supplied changed receipt");
    }
}
