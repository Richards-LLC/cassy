//! `verification action=qa_record|qa_waive|qa_status` (cas-619f).
//!
//! The independent QA verdict is recorded by the reviewer who claimed the
//! round — never the implementer (enforced in `cas_store::qa_pass_store`).
//! A rejection routes the parked delivery back to its implementer through
//! the same store transition the supervisor's `request_changes` uses, and
//! closes the QA work item either way.

use crate::mcp::tools::service::imports::*;
use cas_types::{QaPass, QaVerdict, TaskStatus};

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
        let bundle = crate::qa_pass::validate_round_bundle(std::path::Path::new(ledger_path), &claimed)
            .map_err(|reason| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_record rejected: {reason}")))?;
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
        let qa_task_note = self.close_qa_task(&pass, verdict, summary);
        let routing = match verdict {
            QaVerdict::Approved => self.announce_qa_pass(&pass),
            QaVerdict::Rejected => self.route_qa_rejection(&pass, &reviewer, summary),
        };
        Ok(Self::success(format!(
            "Independent QA {} recorded for {task_id} @{} (pass {}, round {}).\n{routing}{qa_task_note}",
            match verdict {
                QaVerdict::Approved => "APPROVAL",
                QaVerdict::Rejected => "REJECTION",
            },
            pass.head8(),
            pass.id,
            pass.round,
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
        let passes = cas_store::list_qa_passes(&self.inner.cas_root, task_id).unwrap_or_default();
        let head = task
            .deliverables
            .factory_branch_anchor
            .clone()
            .or_else(|| {
                passes
                    .iter()
                    .find(|pass| !pass.is_withdrawn())
                    .map(|pass| pass.bound_head.clone())
            })
            .or_else(|| task.deliverables.delivery_pr_merge_commit.clone())
            .or_else(|| task.deliverables.merge_commit.clone())
            .or_else(|| task.deliverables.commit_hash.clone())
            .filter(|head| !head.trim().is_empty());
        let Some(head) = head else {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "qa_waive: {task_id} has no delivered commit on record to bind a waiver to: it never \
                     parked, no QA round was opened, and no merge commit is recorded. If it was merged \
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
            &head,
            reason,
            chrono::Utc::now(),
        )
        .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, format!("qa_waive rejected: {error}")))?;
        // Same shape as `task action=notes note_type=decision`, so the waiver
        // reads as a decision in every note view.
        let note = format!(
            "[{}] ✅ DECISION Independent QA waived by supervisor {supervisor} for @{} (pass {}). Reason: {}",
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
            "Independent QA waived for {task_id} @{} (pass {}). The waiver is logged on the task; merge and close accept this exact tip only.",
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
        self.enqueue_qa_notice(
            &pass.implementer_agent_id,
            &format!("qa-verdict:{}:implementer", pass.id),
            &reason,
        );
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

