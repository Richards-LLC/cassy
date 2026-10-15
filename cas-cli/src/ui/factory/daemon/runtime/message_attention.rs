//! Read-only alarm eligibility, shared by the sender and supervisor alarm lanes.
//! Acting on work retires its alarm without inventing a per-message read receipt.
use super::*;

/// Red-first stub (cas-1be0).
const GH_1196_RED_STUB: bool = true;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MessageAttention {
    Needed,
    Satisfied,
}

fn close_instruction_tasks(prompt: &str) -> Vec<String> {
    let text = prompt.to_ascii_lowercase();
    if !(text.contains("re-close") || text.contains("reclose") || text.contains("action=close")) {
        return Vec::new();
    }
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .filter(|token| token.starts_with("cas-") && token.len() > 4)
        .map(str::to_owned)
        .collect()
}

impl FactoryDaemon {
    /// Snapshot stores once and resolve each recipient transcript once per scan.
    /// All action evidence is bound to the row. Never fetch on the loop.
    pub(super) fn message_attention_decisions(
        &self,
        queue: &dyn cas_store::PromptQueueStore,
        rows: &[cas_store::QueuedPrompt],
    ) -> HashMap<i64, MessageAttention> {
        if rows.is_empty() {
            return HashMap::new();
        }
        let agents = open_agent_store(self.app.cas_dir())
            .and_then(|store| store.list(None).map_err(crate::error::CasError::from))
            .unwrap_or_default();
        // A bounded snapshot may miss evidence, which conservatively leaves the
        // alarm eligible; missing/unreadable evidence must never silence it.
        let notes = crate::store::open_event_store(self.app.cas_dir())
            .ok()
            .and_then(|store| {
                store
                    .list_since(rows.iter().map(|r| r.created_at).min()?, 2000)
                    .ok()
            })
            .unwrap_or_default();
        let tasks = crate::store::open_task_store_local(self.app.cas_dir()).ok();
        let mut task_cache = HashMap::<String, Option<cas_types::Task>>::new();
        let mut transcript_cache = HashMap::new();
        let mut merge_cache = HashMap::<String, bool>::new();
        let mut decisions = HashMap::new();
        for row in rows {
            if worker_recipient_shut_down(
                &row.target,
                &self.dead_workers,
                self.app.worker_names(),
                &agents,
            ) {
                decisions.insert(row.id, MessageAttention::Satisfied);
                continue;
            }
            let report = match queue.message_delivery_report(row.id) {
                Ok(Some(report)) => report,
                _ => {
                    decisions.insert(row.id, MessageAttention::Needed);
                    continue;
                }
            };
            if report.confirmed_at.is_some()
                || report
                    .recipient_receipt
                    .is_some_and(cas_store::SurfacingSource::is_surfaced)
                || matches!(
                    report.stage,
                    cas_store::DeliveryStage::Dropped
                        | cas_store::DeliveryStage::Abandoned
                        | cas_store::DeliveryStage::Suppressed
                )
            {
                decisions.insert(row.id, MessageAttention::Satisfied);
                continue;
            }
            let recipient = agents
                .iter()
                .filter(|agent| {
                    agent.factory_session.as_deref() == Some(self.session_name.as_str())
                        && (agent.name.eq_ignore_ascii_case(&row.target)
                            || (row.target == "supervisor"
                                && agent.role == cas_types::AgentRole::Supervisor))
                })
                .max_by_key(|agent| agent.registered_at);
            let Some(recipient) = recipient else {
                // Missing registry state is not proof of shutdown or read.
                decisions.insert(row.id, MessageAttention::Needed);
                continue;
            };
            if recipient.status == cas_types::AgentStatus::Shutdown {
                decisions.insert(row.id, MessageAttention::Satisfied);
                continue;
            }
            let cited_note = notes.iter().any(|event| {
                event.event_type == cas_types::EventType::TaskNoteAdded
                    && event.session_id.as_deref() == Some(recipient.id.as_str())
                    && event.created_at > row.created_at
                    && (crate::mcp::tools::core::task::note_notification_ids(&event.summary)
                        .contains(&row.id)
                        || event
                            .metadata
                            .as_ref()
                            .and_then(|meta| meta.get("notification_ids"))
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|ids| ids.iter().any(|id| id.as_i64() == Some(row.id))))
            });
            if cited_note {
                decisions.insert(row.id, MessageAttention::Satisfied);
                continue;
            }
            let transcript = transcript_cache
                .entry(recipient.id.clone())
                .or_insert_with(|| {
                    let mut recipient = recipient.clone();
                    if recipient.role == cas_types::AgentRole::Supervisor
                        && let Some(cli) = recipient.metadata.get("supervisor_cli").cloned()
                    {
                        recipient.metadata.insert("worker_cli".into(), cli);
                    }
                    let cli =
                        crate::mcp::tools::service::factory_ops::worker_cli_from_agent(&recipient);
                    let path =
                        crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(
                            self.app.cas_dir(),
                            &recipient,
                        )?;
                    Some((path, cli))
                });
            if let Some((path, cli)) = transcript.as_ref() {
                // Grok completion alone cannot correlate a message with a turn.
                if matches!(
                    cli,
                    cas_mux::SupervisorCli::Claude | cas_mux::SupervisorCli::Codex
                ) {
                    let observed = crate::mcp::tools::service::harness_observation::observations_after_delivery(
                        path, *cli, row.created_at, &row.prompt,
                    );
                    if observed.wake.is_some() || observed.reaction.is_some() {
                        decisions.insert(row.id, MessageAttention::Satisfied);
                        continue;
                    }
                }
            }
            let envelope = crate::prompt_revalidation::parse_merge_request_envelope(&row.prompt);
            let mut task_ids = close_instruction_tasks(&row.prompt);
            if let Some(envelope) = envelope.as_ref() {
                task_ids.push(envelope.task_id.clone());
            }
            if let Some(id) = crate::prompt_revalidation::assignment_solicited_task_id(&row.prompt)
            {
                task_ids.push(id);
            }
            let terminal = !task_ids.is_empty()
                && task_ids.iter().all(|id| {
                    task_cache
                        .entry(id.clone())
                        .or_insert_with(|| tasks.as_ref()?.get(id).ok())
                        .as_ref()
                        .is_some_and(|task| {
                            let ended_after_row = task
                                .closed_at
                                .unwrap_or(task.updated_at)
                                > row.created_at;
                            // GH #1196: a cancelled task asks nothing more of
                            // anyone; a closed one must also carry the merge.
                            (!GH_1196_RED_STUB
                                && task.status == cas_types::TaskStatus::Cancelled
                                && ended_after_row)
                                || (task.status == cas_types::TaskStatus::Closed
                                    && task.closed_at.is_some_and(|at| at > row.created_at)
                                    && (task.deliverables.merge_commit.is_some()
                                        || task.deliverables.delivery_pr_merge_commit.is_some()
                                        || task.deliverables.delivery_pr_merged_at.is_some()))
                        })
                });
            if terminal {
                decisions.insert(row.id, MessageAttention::Satisfied);
                continue;
            }
            if let Some(envelope) = envelope.as_ref() {
                let key = format!(
                    "{}:{}:{}",
                    envelope.task_id, envelope.branch_tip, envelope.target_branch
                );
                let merged = merge_cache.entry(key).or_insert_with(|| {
                    let task = task_cache
                        .entry(envelope.task_id.clone())
                        .or_insert_with(|| tasks.as_ref()?.get(&envelope.task_id).ok());
                    let root = self.app.cas_dir().parent().unwrap_or(self.app.cas_dir());
                    matches!(
                        crate::prompt_revalidation::queued_merge_request_decision(
                            root,
                            task.as_ref(),
                            envelope
                        ),
                        crate::prompt_revalidation::MergeRequestDecision::AlreadyIntegrated { .. }
                    )
                });
                if *merged {
                    decisions.insert(row.id, MessageAttention::Satisfied);
                    continue;
                }
            }
            // Explicit replies are acknowledged by message_send at commit.
            // There is no typed informational signal for supervisor→worker
            // sends yet; approval-like prose must remain eligible for alarms.
            decisions.insert(row.id, MessageAttention::Needed);
        }
        decisions
    }
}
