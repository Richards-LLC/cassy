//! Independent QA dispatch at the merge park (cas-619f).
//!
//! When a user-facing delivery parks for merge, open (or re-find) its QA
//! round, create the QA work item a different agent will start, and wake the
//! supervisor. Every step is best-effort relative to the park itself: a
//! failure here is reported in the refusal text and never loses the park,
//! and the merge/close gates still refuse until a verdict exists.

use std::path::Path;

use crate::mcp::tools::core::imports::*;
use crate::qa_pass::{
    QA_PASS_LABEL, changed_paths_for_delivery, delivery_eligibility, qa_task_description,
    qa_task_title, round_dir,
};
use cas_store::{NewQaPass, QaPassOpen};
use cas_types::{Dependency, DependencyType, QaPass};

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
        let claimed = cas_store::claim_qa_pass(&self.cas_root, &pass.task_id, &name, chrono::Utc::now())
            .map_err(reject)?;
        Ok(Some(format!(
            "\nIndependent QA round {} claimed for {} @{} — you are the reviewer; deadline {}.",
            claimed.round,
            claimed.task_id,
            claimed.head8(),
            claimed.deadline_at.to_rfc3339()
        )))
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
        let branch = format!("factory/{implementer}");
        let changed = match changed_paths_for_delivery(repo, parent_branch, &branch) {
            Ok(paths) => Some(paths),
            Err(error) => {
                tracing::warn!(task_id = %task.id, error = %error, "cas-619f: delivery diff unavailable; eligibility uses the demo_statement only");
                None
            }
        };
        self.independent_qa_for_paths(task, repo, parent_branch, head, changed)
    }

    /// Shared tail of the park and the close backstop: decide eligibility
    /// from a known (or unknown) change set, then open the round.
    fn independent_qa_for_paths(
        &self,
        task: &Task,
        repo: &Path,
        parent_branch: &str,
        head: Option<&str>,
        changed: Option<Vec<String>>,
    ) -> Option<String> {
        let config = crate::config::Config::load(&self.cas_root).ok()?;
        let qa = config.qa();
        let implementer = task.assignee.as_deref()?;
        let branch = format!("factory/{implementer}");
        let journeys = changed
            .as_deref()
            .map(|paths| crate::qa_pass::catalog_journeys_for(repo, paths))
            .unwrap_or_default();
        let eligibility = delivery_eligibility(task, &qa, changed.as_deref(), &journeys);
        if !eligibility.is_eligible() {
            return None;
        }
        let reasons = eligibility.reasons.join(", ");
        let Some(head) = head else {
            return Some(format!(
                "\n\nINDEPENDENT QA REQUIRED ({reasons}), but the tip of {branch} could not be resolved, \
                 so no QA pass was dispatched. Push the branch and close again."
            ));
        };
        let now = chrono::Utc::now();
        let new = NewQaPass {
            task_id: &task.id,
            implementer_agent_id: implementer,
            branch: &branch,
            bound_head: head,
            deadline_at: now + chrono::Duration::minutes(i64::from(qa.pass_timeout_mins.max(1))),
            max_rounds: qa.max_rounds,
        };
        let outcome = match cas_store::open_qa_pass(&self.cas_root, &new, now) {
            Ok(outcome) => outcome,
            Err(error) => {
                tracing::error!(task_id = %task.id, error = %error, "cas-619f: QA pass could not be opened");
                return Some(format!(
                    "\n\nINDEPENDENT QA REQUIRED ({reasons}), but Cassy could not open the pass: {error}. \
                     The merge stays blocked until a pass records a verdict for {head}."
                ));
            }
        };
        Some(match outcome {
            QaPassOpen::Dispatched(pass) => {
                self.materialize_qa_round(task, pass, &reasons, parent_branch, &config)
            }
            QaPassOpen::AlreadyOpen(pass) => {
                if pass.qa_task_id.is_none() {
                    // A previous park opened the round but crashed before
                    // its work item existed; finish the job.
                    self.materialize_qa_round(task, pass, &reasons, parent_branch, &config)
                } else {
                    format!(
                        "\n\nINDEPENDENT QA PENDING: pass {} (round {}) for {} is {}{}; QA task {}. \
                         The merge waits for its verdict.",
                        pass.id,
                        pass.round,
                        pass.head8(),
                        pass.state,
                        pass.reviewer_agent_id
                            .as_deref()
                            .map(|reviewer| format!(" by {reviewer}"))
                            .unwrap_or_default(),
                        pass.qa_task_id.as_deref().unwrap_or("-"),
                    )
                }
            }
            QaPassOpen::AlreadySatisfied(pass) => format!(
                "\n\nINDEPENDENT QA {}: pass {} covers {}. Ready for the supervisor to merge.",
                if pass.state == cas_types::QaPassState::Waived {
                    "WAIVED"
                } else {
                    "PASSED"
                },
                pass.id,
                pass.head8(),
            ),
            QaPassOpen::Escalate {
                failed_rounds,
                latest,
            } => {
                self.escalate_qa_rounds(task, failed_rounds, &latest);
                format!(
                    "\n\nINDEPENDENT QA ESCALATED: {failed_rounds} rejected rounds (latest {}). \
                     Cassy will not open another round; the supervisor decides (fix plan, waiver, or cancel).",
                    latest.id
                )
            }
        })
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

    /// cas-619f close backstop: after a merge, an independently reviewed tip
    /// must be contained in the target branch. Returns the refusal when not,
    /// dispatching a round first if none is open so the task can progress.
    pub(crate) fn independent_qa_close_refusal(
        &self,
        task: &Task,
        repo: &Path,
        target_branch: &str,
    ) -> Option<String> {
        let config = crate::config::Config::load(&self.cas_root).ok()?;
        let qa = config.qa();
        if !qa.independent_pass || task.assignee.is_none() {
            return None;
        }
        let passes = cas_store::list_qa_passes(&self.cas_root, &task.id).unwrap_or_default();
        let covered = passes.iter().any(|pass| {
            pass.state.satisfies_gate()
                && std::process::Command::new("git")
                    .args(["merge-base", "--is-ancestor", &pass.bound_head, target_branch])
                    .current_dir(repo)
                    .status()
                    .is_ok_and(|status| status.success())
        });
        if covered {
            return None;
        }
        let head = task
            .deliverables
            .factory_branch_anchor
            .clone()
            .or_else(|| {
                task.assignee.as_deref().and_then(|name| {
                    super::close_ops::resolve_branch_sha(repo, &format!("factory/{name}"))
                })
            });
        // Judge from what the delivery actually integrated, so a
        // docs/test/CI-only change closes freely even when it merged before
        // it ever parked through the gate.
        let changed = head
            .as_deref()
            .and_then(|head| crate::qa_pass::integrated_paths(repo, head, target_branch));
        if passes.is_empty() {
            let journeys = changed
                .as_deref()
                .map(|paths| crate::qa_pass::catalog_journeys_for(repo, paths))
                .unwrap_or_default();
            if !delivery_eligibility(task, &qa, changed.as_deref(), &journeys).is_eligible() {
                return None;
            }
        } else if !crate::qa_pass::gate_applies(task, &qa, &passes) {
            return None;
        }
        let dispatch = self
            .independent_qa_for_paths(task, repo, target_branch, head.as_deref(), changed)
            .unwrap_or_default();
        Some(format!(
            "INDEPENDENT QA REQUIRED: {} is user-facing and no passed or waived QA round covers a tip \
             merged into {target_branch}. The close waits for the reviewer's verdict (or a logged \
             supervisor waiver: verification action=qa_waive task_id={}).{dispatch}",
            task.id, task.id,
        ))
    }

    fn materialize_qa_round(
        &self,
        task: &Task,
        pass: QaPass,
        reasons: &str,
        parent_branch: &str,
        config: &crate::config::Config,
    ) -> String {
        let artifacts_root =
            crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
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
             A different agent reviews the running build; the merge waits for its verdict, and a rejection \
             returns this task to you with the ledger at {}/LEDGER.md.",
            pass.id,
            pass.round,
            pass.head8(),
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
                 Cassy stopped opening rounds. Decide: a fix plan with the implementer, a logged waiver \
                 (verification action=qa_waive), or cancel.",
                task = task.id,
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
