//! `verification action=qa_record|qa_waive|qa_status` (cas-619f).
//!
//! The independent QA verdict is recorded by the reviewer who claimed the
//! round — never the implementer (enforced in `cas_store::qa_pass_store`).
//! A rejection routes the parked delivery back to its implementer through
//! the same store transition the supervisor's `request_changes` uses, and
//! closes the QA work item either way.

use crate::mcp::tools::service::imports::*;
use crate::qa_pass::PreExistingIssue;
use cas_types::{Dependency, DependencyType, QaPass, QaVerdict, TaskStatus};

impl CasService {
    pub(super) async fn verification_qa_record(
        &self,
        req: VerificationRequest,
    ) -> Result<CallToolResult, McpError> {
        let task_id = required(req.task_id.as_deref(), "task_id (the delivery under review)")?;
        let verdict: QaVerdict = required(req.status.as_deref(), "status (approved or rejected)")?
            .parse()
            .map_err(|error: cas_types::TypeError| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
        let summary = required(req.summary.as_deref(), "summary")?;
        let ledger_path = required(req.ledger_path.as_deref(), "ledger_path (this round's LEDGER.md)")?;
        if !std::path::Path::new(ledger_path).is_file() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("qa_record: ledger_path {ledger_path} is not a file; write the ledger first"),
            ));
        }
        // cas-e371 (GH #1023 finding 1): issues marked pre-existing are the
        // page's backlog, not the delivery's defects. They never carry a
        // rejection on their own, and each one becomes a linked follow-up.
        let (mut pre_existing, delivery_issues) = crate::qa_pass::split_qa_issues(req.issues.as_deref())
            .map_err(|problem| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_record rejected: {problem}")))?;
        // cas-2849: a pre-existing issue takes its text from the ledger's
        // "F10 NORMAL: <text>" line when issues gave only its id; one with no
        // text anywhere is refused before anything is recorded.
        let ledger_text = std::fs::read_to_string(ledger_path).unwrap_or_default();
        crate::qa_pass::complete_pre_existing_issues(&mut pre_existing, &ledger_text)
            .map_err(|problem| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_record rejected: {problem}")))?;
        if let Some(refusal) =
            crate::qa_pass::rejection_scope_refusal(verdict, pre_existing.len(), delivery_issues)
        {
            return Err(Self::error(ErrorCode::INVALID_PARAMS, format!("qa_record rejected: {refusal}")));
        }
        let (reviewer, reviewer_id) = self.inner.qa_caller_identity()?;
        let cas_root = self.inner.cas_root.clone();
        let now = chrono::Utc::now();

        // Claim first (idempotent for the same reviewer). This is where the
        // no-self-review rule bites if the implementer tries to record it —
        // under its registered name or its session id.
        if let Some(open) = cas_store::latest_qa_pass(&cas_root, task_id, now)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
            && open.implementer_agent_id == reviewer_id
        {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("qa_record rejected: no self-review: {reviewer_id} implemented {task_id}"),
            ));
        }
        let claimed = cas_store::claim_qa_pass(&cas_root, task_id, &reviewer, now).map_err(|error| {
            Self::error(ErrorCode::INVALID_PARAMS, format!("qa_record rejected: {error}"))
        })?;
        // The verdict must be backed by the round's evidence bundle, built
        // against exactly the tip under review (cas-c3b8 contract v1).
        let bundle = crate::qa_pass::validate_round_bundle(std::path::Path::new(ledger_path), &claimed, verdict)
            .map_err(|reason| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_record rejected: {reason}")))?;
        if verdict == QaVerdict::Approved {
            let store = self.inner.open_task_store()?;
            let delivery = store.get(task_id).map_err(|e| Self::error(ErrorCode::INTERNAL_ERROR, e.to_string()))?;
            if crate::qa_evidence::journeys::affects_hub(&delivery.deliverables.files_changed)
                || crate::qa_evidence::journeys::recorded_selection(&delivery.notes, &claimed.bound_head).is_some()
            {
                let repo = crate::mcp::tools::core::task::lifecycle::close_ops::resolve_close_gate_repo_root(&cas_root)
                    .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e))?;
                let config = crate::config::Config::load(&cas_root)
                    .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e.to_string()))?;
                let roots = crate::config::resolved_factory_artifact_paths(&cas_root, config.factory().artifacts_root.as_deref());
                let manifest = bundle.canonicalize().map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e.to_string()))?;
                let artifacts = roots.task_dirs(task_id).into_iter()
                    .find(|dir| dir.canonicalize().is_ok_and(|root| manifest.starts_with(root)))
                    .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "QA journey evidence escapes the owning task artifacts"))?;
                let ctx = crate::qa_evidence::EvidenceContext {
                    task_id, task_artifacts_dir: &artifacts, repo: &repo,
                    delivered_head: &claimed.bound_head, notes: &delivery.notes, deployed_origins: &[],
                };
                crate::qa_evidence::journeys::check_round_journeys(&ctx, &manifest, &delivery.notes, &delivery.deliverables.files_changed)
                    .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_record rejected: {}. Next: {}", e.problem, e.command)))?;
            }
        }
        let pass = cas_store::resolve_qa_pass(
            &cas_root,
            task_id,
            &reviewer,
            verdict,
            summary,
            req.issues.as_deref(),
            ledger_path,
            now,
        )
        .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_record rejected: {error}")))?;
        // cas-2ee2: the verdict turns the GitHub required check green or red.
        crate::qa_pass::github_gate::publish_pass_status(&cas_root, &pass);

        // Cite the round's bundle on the delivery the same way an
        // implementer cites its own (`note_type=platform_proof`).
        if let Ok(store) = self.inner.open_task_store() {
            let note = format!(
                "[{}] 🧪 PLATFORM_PROOF qa-bundle: {} (independent QA round {}, {})",
                now.format("%Y-%m-%d %H:%M"),
                bundle.display(),
                pass.round,
                pass.state,
            );
            if let Err(error) = store.append_note(task_id, &note) {
                tracing::warn!(task_id = %task_id, error = %error, "cas-619f: bundle citation not recorded");
            }
        }
        let follow_ups = self.file_pre_existing_follow_ups(task_id, &pass, &pre_existing, ledger_path);
        let qa_task_note = self.close_qa_task(&pass, verdict, summary);
        let routing = match verdict {
            QaVerdict::Approved => self.announce_qa_pass(&pass),
            QaVerdict::Rejected => self.route_qa_rejection(&pass, &reviewer, summary),
        };
        Ok(Self::success(format!(
            "Independent QA {} recorded for {task_id} @{} (pass {}, round {}).\n{routing}{qa_task_note}{follow_ups}",
            match verdict {
                QaVerdict::Approved => "APPROVAL",
                QaVerdict::Rejected => "REJECTION",
            },
            pass.head8(),
            pass.id,
            pass.round,
        )))
    }

    /// cas-74284 / cas-9ffa: `verification action=qa_request task_id=<delivery>
    /// summary=<reason>`. Supervisor-only. Opens an independent QA round for a
    /// parked delivery the park did not judge user-facing (for example a
    /// hub-web change parked without a demo_statement, whose demo_statement
    /// the delivery-proof scope lock no longer lets anyone add). From then on
    /// every merge gate waits for that round's verdict. An explicit head_sha
    /// identifies pushed Open/InProgress corrections through the WorkTarget;
    /// requesting QA does not park or close that delivery.
    pub(super) async fn verification_qa_request(
        &self,
        req: VerificationRequest,
    ) -> Result<CallToolResult, McpError> {
        if !crate::harness_policy::is_supervisor_from_env() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "qa_request is supervisor-only; a worker's close dispatches QA for a user-facing delivery itself",
            ));
        }
        let task_id = required(req.task_id.as_deref(), "task_id (the delivery to review)")?;
        let reason = required(req.summary.as_deref(), "summary (why this delivery needs independent QA)")?;
        let supervisor = if req.head_sha.is_some() {
            // Recovering an explicit receipt can open review of an unparked
            // delivery, so require the registered factory supervisor role.
            self.inner.resolve_live_supervisor_authority()
                .map_err(|_| Self::error(ErrorCode::INVALID_PARAMS,
                    "qa_request requires a live registered supervisor"))?.id
        } else {
            // Preserve the existing parked-request contract for a standalone
            // Standard session running in supervisor mode. This does not grant
            // workers, dead sessions, or explicit receipt recovery authority.
            let id = self.inner.get_registered_agent_id_read_only()?;
            let caller = self.inner.open_agent_store()?.get(&id)
                .map_err(|_| Self::error(ErrorCode::INVALID_PARAMS,
                    "qa_request requires a live registered supervisor"))?;
            if !caller.is_alive() || !matches!(caller.role,
                cas_types::AgentRole::Supervisor | cas_types::AgentRole::Standard)
            {
                return Err(Self::error(ErrorCode::INVALID_PARAMS,
                    "qa_request requires a live registered supervisor"));
            }
            caller.id
        };
        let task = self
            .inner
            .open_task_store()?
            .get(task_id)
            .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, format!("Task not found: {error}")))?;
        // cas-624f: past the escalation (max_rounds rejections) this request
        // is the supervisor's fix plan, and it opens exactly one more round.
        // Log it as the override it is.
        let rejected_rounds = cas_store::list_qa_passes(&self.inner.cas_root, task_id)
            .unwrap_or_default()
            .iter()
            .filter(|pass| pass.state == cas_types::QaPassState::Failed)
            .count() as u32;
        let max_rounds = crate::config::Config::load(&self.inner.cas_root)
            .map(|config| config.qa().max_rounds)
            .unwrap_or(3)
            .max(1);
        let dispatch = self
            .inner
            .request_independent_qa_at_receipt(&task, reason.trim(), req.head_sha.as_deref())
            .map_err(|why| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_request rejected: {why}")))?;
        let note = if rejected_rounds >= max_rounds {
            format!(
                "[{}] ✅ DECISION Independent QA fix plan: supervisor {supervisor} opened round {} past the \
                 escalation after {rejected_rounds} rejected rounds (qa.max_rounds {max_rounds}). A further \
                 rejection escalates again. Fix plan: {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                rejected_rounds + 1,
                reason.trim(),
            )
        } else {
            format!(
                "[{}] ✅ DECISION Independent QA requested by supervisor {supervisor}. Reason: {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                reason.trim(),
            )
        };
        if let Err(error) = self.inner.open_task_store()?.append_note(task_id, &note) {
            tracing::warn!(task_id = %task_id, error = %error, "cas-74284: qa_request decision note not recorded");
        }
        Ok(Self::success(format!(
            "Independent QA requested for {task_id}.{dispatch}"
        )))
    }

    pub(super) async fn verification_qa_waive(
        &self,
        req: VerificationRequest,
    ) -> Result<CallToolResult, McpError> {
        if !crate::harness_policy::is_supervisor_from_env() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "qa_waive is supervisor-only; a reviewer records a verdict with qa_record",
            ));
        }
        let task_id = required(req.task_id.as_deref(), "task_id")?;
        let reason = required(req.summary.as_deref(), "summary (the waiver reason)")?;
        let supervisor = self.inner.get_agent_id()?;
        let task = self
            .inner
            .open_task_store()?
            .get(task_id)
            .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, format!("Task not found: {error}")))?;
        let implementer = task
            .assignee
            .clone()
            .map(|assigned| {
                self.inner
                    .open_agent_store()
                    .ok()
                    .and_then(|store| {
                        crate::mcp::tools::core::task::resolve_agent_identity(
                            store.as_ref(),
                            &assigned,
                        )
                    })
                    .map(|agent| agent.name)
                    .unwrap_or(assigned)
            })
            .or_else(|| {
                task.deliverables
                    .parked_branch
                    .as_deref()
                    .and_then(|branch| branch.strip_prefix("factory/"))
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_default();
        let branch = task
            .deliverables
            .parked_branch
            .clone()
            .unwrap_or_else(|| format!("factory/{implementer}"));
        // cas-5c38 (GH #999): a no-code task delivers no tip; any round an
        // earlier close opened for it is withdrawn rather than waived.
        if crate::qa_pass::is_no_code(&task) && task.deliverables.factory_branch_anchor.is_none() {
            return self.withdraw_no_code_qa(task_id, &supervisor, reason);
        }
        // cas-5c38 (GH #999): a delivery that never parked has no anchor.
        // Waive the tip the open (or latest) round was bound to, then any
        // commit Cassy recorded for the delivery.
        // cas-6c75 (GH #1048, #1078): worktree_merge checks the branch's
        // current tip. After a rebase the waiver binds to that tip when the
        // open round is bound to it or it is a rebased copy of the recorded
        // one, not to the stale pre-rebase anchor no merge can accept.
        let passes = cas_store::list_qa_passes(&self.inner.cas_root, task_id).unwrap_or_default();
        let repo = self
            .inner
            .cas_root
            .parent()
            .unwrap_or(&self.inner.cas_root)
            .to_path_buf();
        let current_tip = crate::qa_pass::branch_tip(&repo, &branch);
        let target = task
            .deliverables
            .work_target
            .as_ref()
            .map(|target| target.target_branch.clone())
            .filter(|target| !target.trim().is_empty())
            .unwrap_or_else(|| "main".to_string());
        let chosen = if let Some(requested) = req.head_sha.as_deref() {
            let pushed = crate::qa_pass::pushed_branch_tip(&repo, &branch).ok_or_else(|| {
                Self::error(ErrorCode::INVALID_PARAMS, format!(
                    "qa_waive: cannot validate head_sha: {branch} has no readable pushed tip on origin; push the delivery branch and retry"
                ))
            })?;
            if requested.trim() != pushed {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("qa_waive: head_sha must equal {branch}'s pushed tip {pushed}"),
                ));
            }
            Some(crate::qa_pass::WaiverHead {
                head: pushed,
                advanced_from: None,
                why: None,
            })
        } else {
            crate::qa_pass::waiver_head(&task, &passes, current_tip.as_deref(), |recorded, tip| {
                crate::qa_pass::is_rebased_copy(&repo, recorded, tip, &target)
            })
        };
        let Some(chosen) = chosen else {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "qa_waive: {task_id} has no delivered commit on record to bind a waiver to: it never \
                     parked, no QA round was opened, and no merge commit is recorded. For a pushed \
                     delivery, pass head_sha=<full pushed branch SHA>. If it was merged \
                     before close, close it with supervisor_override=true, a reason and \
                     commit_receipt=<merged sha>; the QA gate records the waiver against that commit."
                ),
            ));
        };
        let pass = cas_store::waive_qa_pass(
            &self.inner.cas_root,
            task_id,
            &supervisor,
            &implementer,
            &branch,
            &chosen.head,
            reason,
            chrono::Utc::now(),
        )
        .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_waive rejected: {error}")))?;
        // What the receipt adds: why a newer tip was chosen, or that the
        // branch has moved past the waived tip so the merge will still refuse.
        let short = |sha: &str| sha[..sha.len().min(8)].to_string();
        let binding = match (&chosen.advanced_from, chosen.why) {
            (Some(previous), Some(why)) => format!(
                " It binds to {branch}'s current tip, not the recorded @{}: {why}.",
                short(previous)
            ),
            _ => match current_tip.as_deref() {
                Some(tip) if tip != chosen.head => format!(
                    " Note: {branch} is now at @{}, which this waiver does not cover (it adds or changes work); \
                     worktree_merge will refuse until a round covers that tip.",
                    short(tip)
                ),
                _ => String::new(),
            },
        };
        // cas-2ee2: a waiver satisfies the GitHub required check too, and its
        // status description carries the logged reason.
        crate::qa_pass::github_gate::publish_pass_status(&self.inner.cas_root, &pass);
        // Same shape as `task action=notes note_type=decision`, so the waiver
        // reads as a decision in every note view.
        let note = format!(
            "[{}] ✅ DECISION Independent QA waived by supervisor {supervisor} for @{} (pass {}).{binding} Reason: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            pass.head8(),
            pass.id,
            reason.trim(),
        );
        if let Err(error) = self.inner.open_task_store()?.append_note(task_id, &note) {
            return Err(Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("qa_waive recorded pass {} but the decision note failed: {error}", pass.id),
            ));
        }
        Ok(Self::success(format!(
            "Independent QA waived for {task_id} @{} (pass {}).{binding} The waiver is logged on the task; merge and close accept this exact tip only.",
            pass.head8(),
            pass.id
        )))
    }

    /// cas-5c38: a no-code task has no tip to bind a waiver to, and the QA
    /// gate no longer applies to it. Withdraw any round a previous close
    /// opened, so nothing is left pending, and log the decision.
    fn withdraw_no_code_qa(
        &self,
        task_id: &str,
        supervisor: &str,
        reason: &str,
    ) -> Result<CallToolResult, McpError> {
        let withdrawn = cas_store::withdraw_open_qa_pass(
            &self.inner.cas_root,
            task_id,
            &format!("no-code task, waived by supervisor {supervisor}: {}", reason.trim()),
            true,
            chrono::Utc::now(),
        )
        .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_waive rejected: {error}")))?;
        let closed = withdrawn
            .as_ref()
            .map(|pass| self.close_withdrawn_qa_task(pass, reason))
            .unwrap_or_default();
        let note = format!(
            "[{}] ✅ DECISION Independent QA waived by supervisor {supervisor} for no-code task{}. Reason: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            withdrawn
                .as_ref()
                .map(|pass| format!(" (withdrew round {} pass {})", pass.round, pass.id))
                .unwrap_or_default(),
            reason.trim(),
        );
        self.inner
            .open_task_store()?
            .append_note(task_id, &note)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, format!("qa_waive note failed: {error}")))?;
        Ok(Self::success(format!(
            "Independent QA waived for no-code task {task_id}: it has no delivery tip, and the QA gate does \
             not apply to no-code tasks.{}{closed}",
            withdrawn
                .map(|pass| format!(" Open round {} (pass {}) withdrawn.", pass.round, pass.id))
                .unwrap_or_default(),
        )))
    }

    /// Close the QA work item of a withdrawn round so no reviewer picks it up.
    pub(crate) fn close_withdrawn_qa_task(&self, pass: &QaPass, reason: &str) -> String {
        self.inner.cancel_withdrawn_qa_task(pass, reason)
    }

    pub(super) async fn verification_qa_status(
        &self,
        req: VerificationRequest,
    ) -> Result<CallToolResult, McpError> {
        let task_id = required(req.task_id.as_deref(), "task_id")?;
        let _ = cas_store::latest_qa_pass(&self.inner.cas_root, task_id, chrono::Utc::now());
        let passes = cas_store::list_qa_passes(&self.inner.cas_root, task_id)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        if passes.is_empty() {
            return Ok(Self::success(format!(
                "No independent QA pass recorded for {task_id}."
            )));
        }
        let mut out = format!("Independent QA passes for {task_id} (newest first):\n");
        for pass in &passes {
            out.push_str(&format!(
                "- {} round {} @{} {} reviewer={} qa_task={} ledger={}{}\n",
                pass.id,
                pass.round,
                pass.head8(),
                pass.state,
                pass.reviewer_agent_id.as_deref().unwrap_or("-"),
                pass.qa_task_id.as_deref().unwrap_or("-"),
                pass.ledger_path.as_deref().unwrap_or("-"),
                pass.summary
                    .as_deref()
                    .map(|summary| format!("\n    {summary}"))
                    .unwrap_or_default(),
            ));
        }
        Ok(Self::success(out))
    }

    /// cas-e371: file one follow-up task per pre-existing issue, linked to
    /// the delivery (`related`), and note the ids on the delivery so the
    /// finding stays discoverable after the verdict closes the round.
    fn file_pre_existing_follow_ups(
        &self,
        delivery_id: &str,
        pass: &QaPass,
        issues: &[PreExistingIssue],
        ledger_path: &str,
    ) -> String {
        if issues.is_empty() {
            return String::new();
        }
        let Ok(store) = self.inner.open_task_store() else {
            return format!(
                "\n{} pre-existing issue(s) were not filed as follow-ups (task store unavailable); they are in {ledger_path}.",
                issues.len()
            );
        };
        let Ok(delivery) = store.get(delivery_id) else {
            return format!(
                "\n{} pre-existing issue(s) were not filed as follow-ups ({delivery_id} not found); they are in {ledger_path}.",
                issues.len()
            );
        };
        let mut filed = Vec::new();
        let mut failed = Vec::new();
        // cas-1980: follow-ups join the delivery's epic, so their closes
        // target its branch instead of main.
        let epic = match crate::qa_pass::follow_up_epic(store.as_ref(), &delivery.id) {
            Ok(epic) => epic,
            Err(error) => {
                return format!(
                    "\n{} pre-existing issue(s) were not filed as follow-ups (parent epic lookup failed: {error}); they are in {ledger_path}.",
                    issues.len()
                );
            }
        };
        let mut tracked = Vec::new();
        for issue in issues {
            // cas-2849: a finding that names its existing follow-up is linked,
            // not filed again.
            if let Some(existing) =
                crate::qa_pass::tracked_follow_up(issue, delivery_id, |id| store.get(id).is_ok())
            {
                let related =
                    Dependency::new(existing.clone(), delivery.id.clone(), DependencyType::Related);
                let _ = store.add_dependency(&related);
                let label = if issue.id.is_empty() { issue.problem.as_str() } else { issue.id.as_str() };
                tracked.push(format!("{label} by {existing}"));
                continue;
            }
            let id = match store.generate_id() {
                Ok(id) => id,
                Err(error) => {
                    failed.push(format!("{} ({error})", issue.problem));
                    continue;
                }
            };
            let task =
                crate::qa_pass::follow_up_task(&id, &delivery, pass, issue, ledger_path, epic.as_ref());
            if let Err(error) = store.create_atomic(
                &task,
                &[],
                epic.as_ref().map(|epic| epic.id.as_str()),
                Some("cas-qa-record"),
            ) {
                failed.push(format!("{} ({error})", issue.problem));
                continue;
            }
            let related = Dependency::new(id.clone(), delivery.id.clone(), DependencyType::Related);
            if let Err(error) = store.add_dependency(&related) {
                tracing::warn!(follow_up = %id, delivery = %delivery.id, error = %error, "cas-e371: related link not recorded");
            }
            filed.push(id);
        }
        let mut out = String::new();
        if !filed.is_empty() {
            let note = format!(
                "[{}] Independent QA round {} recorded {} pre-existing issue(s) as follow-ups: {} (not counted against this delivery; ledger {ledger_path})",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                pass.round,
                filed.len(),
                filed.join(", "),
            );
            if let Err(error) = store.append_note(delivery_id, &note) {
                tracing::warn!(task_id = %delivery_id, error = %error, "cas-e371: follow-up note not recorded");
            }
            out.push_str(&format!(
                "\nPre-existing follow-ups filed (related to {delivery_id}): {}.",
                filed.join(", ")
            ));
        }
        if !tracked.is_empty() {
            out.push_str(&format!(
                "\nAlready tracked, not filed again: {}.",
                tracked.join("; ")
            ));
        }
        if !failed.is_empty() {
            out.push_str(&format!(
                "\nNot filed, still in the ledger: {}.",
                failed.join("; ")
            ));
        }
        out
    }

    fn close_qa_task(&self, pass: &QaPass, verdict: QaVerdict, summary: &str) -> String {
        let Some(qa_task_id) = pass.qa_task_id.as_deref() else {
            return String::new();
        };
        let Ok(store) = self.inner.open_task_store() else {
            return format!("\nQA task {qa_task_id} could not be closed (task store unavailable).");
        };
        let Ok(mut qa_task) = store.get(qa_task_id) else {
            return format!("\nQA task {qa_task_id} not found; nothing to close.");
        };
        if qa_task.status == TaskStatus::Closed {
            return String::new();
        }
        let now = chrono::Utc::now();
        qa_task.status = TaskStatus::Closed;
        qa_task.closed_at = Some(now);
        qa_task.close_reason = Some(format!(
            "Independent QA {} for {} @{}: {}",
            match verdict {
                QaVerdict::Approved => "approved",
                QaVerdict::Rejected => "rejected",
            },
            pass.task_id,
            pass.head8(),
            summary.trim()
        ));
        qa_task.external_ref = pass.ledger_path.clone().or(qa_task.external_ref);
        qa_task.pending_verification = false;
        match store.update(&qa_task) {
            Ok(_) => format!("\nQA task {qa_task_id} closed with the verdict."),
            Err(error) => format!("\nQA task {qa_task_id} could not be closed: {error}"),
        }
    }

    fn announce_qa_pass(&self, pass: &QaPass) -> String {
        let body = format!(
            "Independent QA passed for {} @{} (pass {}, reviewer {}). Ledger: {}. It may merge now.",
            pass.task_id,
            pass.head8(),
            pass.id,
            pass.reviewer_agent_id.as_deref().unwrap_or("-"),
            pass.ledger_path.as_deref().unwrap_or("-"),
        );
        self.enqueue_qa_notice("supervisor", &format!("qa-verdict:{}", pass.id), &body);
        "The supervisor was told it may merge this exact tip.".to_string()
    }

    fn route_qa_rejection(&self, pass: &QaPass, reviewer: &str, summary: &str) -> String {
        let reason = format!(
            "Independent QA round {} by {reviewer} rejected {} @{}: {} Ledger with evidence: {}. \
             Fix the findings, push, and close again; the next park opens round {}.",
            pass.round,
            pass.task_id,
            pass.head8(),
            summary.trim(),
            pass.ledger_path.as_deref().unwrap_or("-"),
            pass.round + 1,
        );
        let task_status = self
            .inner
            .open_task_store()
            .ok()
            .and_then(|store| store.get(&pass.task_id).ok())
            .map(|task| task.status);
        let routed = if task_status == Some(TaskStatus::AwaitingMerge) {
            match cas_store::request_changes_for_parked_delivery(
                &self.inner.cas_root,
                &pass.task_id,
                reviewer,
                &reason,
            ) {
                Ok(_) => format!(
                    "{} is back to Open with its assignee kept; request_changes recorded the ledger.",
                    pass.task_id
                ),
                Err(error) => format!(
                    "request_changes could not reopen {}: {error}. The supervisor must decline it manually.",
                    pass.task_id
                ),
            }
        } else {
            format!(
                "{} is not parked (status {}); the rejection stands and merge/close stay blocked for @{}.",
                pass.task_id,
                task_status.map(|status| status.to_string()).unwrap_or_else(|| "unknown".into()),
                pass.head8()
            )
        };
        if let Some(owner) = self.inner.open_task_store().ok()
            .and_then(|store| store.get(&pass.task_id).ok())
            .filter(|task| !task.is_terminal())
            .and_then(|task| task.assignee)
        {
            let target = self.inner.open_agent_store().ok()
                .and_then(|store| store.get(&owner).ok())
                .map(|agent| agent.name)
                .unwrap_or(owner);
            self.enqueue_qa_notice(
                &target,
                &format!("qa-verdict:{}:implementer", pass.id),
                &reason,
            );
        }
        self.enqueue_qa_notice(
            "supervisor",
            &format!("qa-verdict:{}", pass.id),
            &format!("{reason}\n{routed}"),
        );
        routed
    }

    fn enqueue_qa_notice(&self, target: &str, source: &str, body: &str) {
        let Ok(queue) = crate::store::open_prompt_queue_store(&self.inner.cas_root) else {
            return;
        };
        let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
        if let Err(error) = queue.enqueue_idempotent(
            source,
            target,
            body,
            factory_session.as_deref(),
            Some(&body.chars().take(120).collect::<String>()),
            Some(cas_store::NotificationPriority::High),
            source,
            Some(&cas_store::QueueOrigin::Daemon),
        ) {
            tracing::warn!(target = %target, error = %error, "cas-619f: QA verdict notice not queued");
        }
    }
}

fn required<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, McpError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CasService::error(ErrorCode::INVALID_PARAMS, format!("{name} is required")))
}

