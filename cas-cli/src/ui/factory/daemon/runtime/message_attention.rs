//! Read-only alarm eligibility, shared by the sender and supervisor alarm lanes.
//! Acting on work retires its alarm without inventing a per-message read receipt.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MessageAttention {
    Needed,
    Satisfied,
    Unknown,
}

fn informational_approval(row: &cas_store::QueuedPrompt) -> bool {
    let text = row.prompt.trim().to_ascii_lowercase();
    // These are supervisor progress receipts, not instructions to re-close now.
    (text.starts_with("review pass") || text.starts_with("approved"))
        && (text.contains("holding")
            || text.contains("i will merge")
            || text.contains("i'll merge")
            || text.contains("i merge")
            || text.contains("awaiting ci"))
        && ![
            "re-close now",
            "reclose now",
            "action=close",
            "action=start",
            "please ",
            "rebase ",
            "run ",
            "fix ",
            "implement ",
            "resume ",
            "stop ",
            "start ",
        ]
        .iter()
        .any(|instruction| text.contains(instruction))
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
    /// One store snapshot per scan, one transcript read per recipient, and one
    /// local Git decision per distinct merge request. Never fetch on the loop.
    pub(super) fn message_attention_decisions(
        &self,
        queue: &dyn cas_store::PromptQueueStore,
        rows: &[cas_store::QueuedPrompt],
    ) -> HashMap<i64, MessageAttention> {
        if rows.is_empty() {
            return HashMap::new();
        }
        let agents = match open_agent_store(self.app.cas_dir())
            .and_then(|store| store.list(None).map_err(crate::error::CasError::from))
        {
            Ok(agents) => agents,
            Err(error) => {
                tracing::debug!(%error, "message attention registry unavailable; defer alarms");
                return HashMap::new();
            }
        };
        let events = crate::store::open_event_store(self.app.cas_dir()).ok();
        let tasks = crate::store::open_task_store_local(self.app.cas_dir()).ok();
        let mut task_cache = HashMap::<String, Option<cas_types::Task>>::new();
        let mut turn_cache = HashMap::<String, Option<chrono::DateTime<chrono::Utc>>>::new();
        let mut merge_cache = HashMap::<String, bool>::new();
        let mut decisions = HashMap::new();
        for row in rows {
            if self.dead_workers.contains(&row.target) {
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
                // Missing registry state is not proof of either shutdown or read.
                decisions.insert(row.id, MessageAttention::Unknown);
                continue;
            };
            if recipient.status == cas_types::AgentStatus::Shutdown {
                decisions.insert(row.id, MessageAttention::Satisfied);
                continue;
            }
            let report = match queue.message_delivery_report(row.id) {
                Ok(Some(report)) => report,
                _ => {
                    decisions.insert(row.id, MessageAttention::Unknown);
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
            let aliases = crate::harness_policy::inbox_aliases(
                &recipient.name,
                recipient.role == cas_types::AgentRole::Supervisor,
            );
            let mut activity = RecipientActivity::Absent;
            for alias in aliases {
                match queue.has_message_from_since(&alias, &self.session_name, row.created_at) {
                    Ok(true) => {
                        activity = RecipientActivity::Found;
                        break;
                    }
                    Ok(false) => {}
                    Err(_) => activity = RecipientActivity::Unknown,
                }
            }
            if let Some(events) = events.as_ref() {
                match events.has_event_since(
                    cas_types::EventType::TaskNoteAdded,
                    &recipient.id,
                    row.created_at,
                ) {
                    Ok(true) => activity = RecipientActivity::Found,
                    Ok(false) => {}
                    Err(_) if activity != RecipientActivity::Found => {
                        activity = RecipientActivity::Unknown
                    }
                    Err(_) => {}
                }
            } else if activity != RecipientActivity::Found {
                activity = RecipientActivity::Unknown;
            }
            if activity == RecipientActivity::Found {
                decisions.insert(row.id, MessageAttention::Satisfied);
                continue;
            }
            let last_turn = turn_cache.entry(recipient.id.clone()).or_insert_with(|| {
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
                let observed =
                    crate::mcp::tools::service::harness_observation::latest_turn_observations(
                        &path, cli,
                    );
                observed
                    .wake
                    .into_iter()
                    .chain(observed.completion)
                    .map(|observation| observation.at)
                    .max()
            });
            if last_turn.is_some_and(|at| at > row.created_at) {
                decisions.insert(row.id, MessageAttention::Satisfied);
                continue;
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
                            matches!(
                                task.status,
                                cas_types::TaskStatus::Closed | cas_types::TaskStatus::Cancelled
                            )
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
            let from_supervisor = agents.iter().any(|agent| {
                agent.factory_session.as_deref() == Some(self.session_name.as_str())
                    && agent.role == cas_types::AgentRole::Supervisor
                    && (agent.name == row.source || row.source == "supervisor")
            });
            if from_supervisor
                && recipient.role == cas_types::AgentRole::Worker
                && informational_approval(row)
            {
                decisions.insert(row.id, MessageAttention::Satisfied);
                continue;
            }
            decisions.insert(
                row.id,
                if activity == RecipientActivity::Unknown {
                    MessageAttention::Unknown
                } else {
                    MessageAttention::Needed
                },
            );
        }
        decisions
    }
}
