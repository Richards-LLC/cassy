use crate::mcp::tools::service::imports::*;
use crate::prompt_revalidation::{assignment_solicited_task_id, assignment_targets_terminal_task};

fn resolve_inbox_recipient(
    registered_name: Option<String>,
    environment_name: Option<String>,
) -> Option<String> {
    registered_name
        .or(environment_name)
        .filter(|name| !name.trim().is_empty())
}

/// Marker prefixed to any inbox row this poll is handing over for a second
/// time (cas-99d2, GH #127).
///
/// Recipients — human or agent — must be able to tell a repeat from a new
/// instruction mechanically, without reasoning about timestamps. The daemon's
/// teams-inbox writer already treats this exact token as the signal for an
/// *intentional* redelivery (see `daemon::runtime::teams`), so the same marker
/// means the same thing on both delivery channels.
pub(crate) const INBOX_REDELIVERY_MARKER: &str = "[redelivery]";

/// Durable provenance rendered with every queue-backed message (cas-4a27).
///
/// A plain sender prefix cannot distinguish a late spawn task brief from a
/// fresh supervisor reply. The queue already knows the facts a worker needs;
/// keep them visible at the final render boundary instead of asking an agent
/// to reconstruct them from prose and timing. Five minutes is an operational
/// freshness marker, not the 24-hour terminal quarantine TTL: a message can be
/// valid but still worth calling out as late to the recipient.
pub(crate) const MESSAGE_PROVENANCE_STALE_AFTER_SECS: i64 = 5 * 60;

pub(crate) fn queued_message_provenance_at(
    message: &cas_store::QueuedPrompt,
    observed_at: chrono::DateTime<chrono::Utc>,
) -> String {
    let origin = if message.source.eq_ignore_ascii_case("supervisor") {
        "supervisor-authored"
    } else if message.source.eq_ignore_ascii_case("viktor") {
        "viktor"
    } else if message.source.eq_ignore_ascii_case("director")
        && message
            .summary
            .as_deref()
            .is_some_and(|summary| summary.starts_with("Assigned task:"))
    {
        "spawn-boilerplate"
    } else if message.source.eq_ignore_ascii_case("director") {
        "director-generated"
    } else if message.source.starts_with("lifecycle-wake:") {
        "lifecycle-relay"
    } else {
        "agent-authored"
    };
    let delivery = if message.processed_at.is_some() {
        "replay"
    } else {
        "first-delivery"
    };
    let age_secs = (observed_at - message.created_at).num_seconds().max(0);
    let stale = age_secs >= MESSAGE_PROVENANCE_STALE_AFTER_SECS;
    format!(
        "CAS provenance: notification_id={} origin={} queued_at={} age_secs={} stale={} delivery={}",
        message.id,
        origin,
        message.created_at.to_rfc3339(),
        age_secs,
        stale,
        delivery,
    )
}

pub(crate) fn queued_message_provenance(message: &cas_store::QueuedPrompt) -> String {
    queued_message_provenance_at(message, chrono::Utc::now())
}

#[cfg(test)]
mod viktor_provenance_tests {
    use super::queued_message_provenance_at;
    use cas_store::{NotificationPriority, QueuedPrompt};

    #[test]
    fn viktor_queue_rows_keep_the_standard_provenance_envelope() {
        let row = QueuedPrompt {
            id: 73,
            source: "viktor".to_string(),
            target: "worker-1".to_string(),
            prompt: "reply".to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-08-18T20:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            processed_at: None,
            factory_session: Some("factory-1".to_string()),
            summary: Some("Viktor reply".to_string()),
            priority: NotificationPriority::High,
            acked_at: None,
            urgent: false,
        };

        let observed_at = chrono::DateTime::parse_from_rfc3339("2026-08-18T20:06:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            queued_message_provenance_at(&row, observed_at),
            "CAS provenance: notification_id=73 origin=viktor queued_at=2026-08-18T20:00:00+00:00 age_secs=360 stale=true delivery=first-delivery"
        );
    }

    #[test]
    fn provenance_marks_only_rows_past_the_operational_freshness_window() {
        let row = QueuedPrompt {
            id: 74,
            source: "director".to_string(),
            target: "worker-1".to_string(),
            prompt: "assignment".to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-08-18T20:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            processed_at: None,
            factory_session: Some("factory-1".to_string()),
            summary: Some("Assigned task: cas-test".to_string()),
            priority: NotificationPriority::Normal,
            acked_at: None,
            urgent: false,
        };
        let observed_at = row.created_at + chrono::Duration::seconds(299);
        let provenance = queued_message_provenance_at(&row, observed_at);
        assert!(provenance.contains("age_secs=299"));
        assert!(provenance.contains("stale=false"));
    }
}

/// cas-99d2 (GH #127): what to do with one row an inbox poll selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InboxRedelivery {
    /// Never handed to a transport before — render it plainly.
    FirstDelivery,
    /// Already transport-delivered once; render it, marked as a repeat.
    MarkRedelivery,
    /// Already transport-delivered AND the action it solicited demonstrably
    /// happened — re-rendering it verbatim would read as a fresh instruction.
    WithholdConsumed,
}

/// cas-99d2 (GH #127): decide how an inbox poll should render a selected row.
///
/// Pure so the reported shape is testable without a store, a daemon or a clock.
///
/// `already_transport_delivered` is the daemon's own handoff stamp. Note that
/// this is deliberately NOT a reason to drop the row outright: the whole point
/// of the polling API is to surface rows the transport wrote to a file nobody
/// read. What it does establish is that this is not the recipient's first look
/// at the content, which is exactly what the recipient needs told.
///
/// `solicited_action_observed` is real consumption evidence: the state
/// transition the message asked for is already in the store, attributed to this
/// recipient. For an assignment that means the named task has been started by
/// the addressed worker. When a message has both — it was delivered, and what
/// it asked for happened — handing it back verbatim 15 minutes later is a
/// fabricated instruction, which is the reported bug (notification 7112).
pub(crate) fn inbox_redelivery_decision(
    already_transport_delivered: bool,
    solicited_action_observed: bool,
) -> InboxRedelivery {
    if !already_transport_delivered {
        return InboxRedelivery::FirstDelivery;
    }
    if solicited_action_observed {
        return InboxRedelivery::WithholdConsumed;
    }
    InboxRedelivery::MarkRedelivery
}

/// Inbox polling is a delivery surface, not an authority to discard a relay.
/// A failed task read is uncertainty, so only a freshly-read task whose
/// lifecycle occurrence is positively stale may be withheld.
pub(crate) fn lifecycle_relay_is_stale_at_inbox_pop(
    prompt: &str,
    task: Option<&cas_types::Task>,
) -> bool {
    let Some(task) = task else {
        return false;
    };
    matches!(
        crate::prompt_revalidation::revalidate_lifecycle_prompt(
            prompt,
            task.status,
            task.updated_at,
        ),
        crate::prompt_revalidation::LifecyclePromptDecision::SuppressStale { .. }
    )
}

/// cas-7a01 (GH #155): render the wake evidence pair as a sentence.
///
/// The two fields answer different questions and the failure the issue reported
/// lives precisely in their disagreement, so an operator must not have to
/// derive it. `wake_attempt` is what Cassy did; `wake` is whether a turn
/// demonstrably carried the content. `nudge_fired` + `unobserved` is the
/// GH #155 signature and is called out by name — that combination used to be
/// completely invisible.
pub(crate) fn wake_attempt_narrative(
    attempt: cas_store::WakeAttempt,
    wake: cas_store::ObservationStatus,
) -> String {
    use cas_store::{ObservationStatus, WakeAttempt};
    let line = match (attempt, wake) {
        (_, ObservationStatus::Observed) => {
            "wake evidence: this message was injected into a turn on the recipient's side \
             (hook surfacing receipt)."
        }
        (WakeAttempt::Fired, ObservationStatus::Unobserved) => {
            "wake evidence: Cassy DID nudge this recipient's pane, and no turn is recorded as \
             having carried this message — the nudge landed but the harness surfaced nothing \
             (GH #155 signature)."
        }
        (WakeAttempt::Failed, ObservationStatus::Unobserved) => {
            "wake evidence: Cassy ATTEMPTED a wake and it FAILED (see wake_attempt_detail); the \
             recipient was never nudged for this message."
        }
        (WakeAttempt::NotAttempted, ObservationStatus::Unobserved) => {
            "wake evidence: Cassy never attempted a wake for this message — the idle gate \
             declined it, or the recipient's channel needs no nudge."
        }
    };
    format!("{line}\n")
}

/// cas-ac7e (GH #130): the operator-facing warning for a row that claims
/// `stage=delivered` with no recipient-side transport stamp.
///
/// A free function, not an inline condition, because this warning IS the fix's
/// visible surface: it is how an operator tells notification 7183's shape
/// (delivered per the writer, unrecorded per the recipient) from a genuinely
/// corroborated delivery. Three independent clauses have to be right and an
/// inline expression let all three be wrong silently.
///
/// `all_workers` is exempt: a broadcast's per-recipient transport is its
/// broadcast counts, so it legitimately has no single-recipient stamp.
pub(crate) fn recipient_transport_warning(
    stage: cas_store::DeliveryStage,
    target: &str,
    has_recipient_transport: bool,
) -> Option<&'static str> {
    if stage != cas_store::DeliveryStage::Delivered
        || target == "all_workers"
        || has_recipient_transport
    {
        return None;
    }
    Some(
        "recipient_transport: MISSING — this row reports stage=delivered with no \
         per-recipient transport stamp. Either it was delivered before Cassy \
         recorded them, or the stamp and the stage have diverged; treat the \
         delivery as unproven and re-send.\n",
    )
}

const CLAUDE_INTERRUPT_CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const CLAUDE_INTERRUPT_CONFIRM_POLL: std::time::Duration = std::time::Duration::from_millis(100);

fn interrupt_delivery_is_observed(report: &cas_store::MessageDeliveryReport) -> bool {
    matches!(
        report.stage,
        cas_store::DeliveryStage::Delivered | cas_store::DeliveryStage::Confirmed
    ) && report.delivered_at.is_some()
        && report.recipient_transport_at.is_some()
}

fn interrupt_delivery_failure(report: &cas_store::MessageDeliveryReport) -> Option<String> {
    report.stage.is_terminal_non_delivery().then(|| {
        format!(
            "delivery reached terminal stage {}{}",
            report.stage,
            report
                .pending_detail
                .as_deref()
                .map(|detail| format!(": {detail}"))
                .unwrap_or_default()
        )
    })
}

fn interrupt_unconfirmed_message(
    notification_id: i64,
    report: Option<&cas_store::MessageDeliveryReport>,
) -> String {
    let state = report.map_or_else(
        || "no delivery report was readable".to_string(),
        |report| {
            format!(
                "stage={}, wake_attempt={}, recipient_transport={}",
                report.stage,
                report.wake_attempt,
                if report.recipient_transport_at.is_some() {
                    "observed"
                } else {
                    "unobserved"
                }
            )
        },
    );
    format!(
        "Could not confirm Claude interrupt delivery for notification {notification_id} within {}s ({state}). The queue row remains durable for retry, but this call did not silently claim the teammate was interrupted; inspect `coordination action=message_status notification_id={notification_id}` before proceeding.",
        CLAUDE_INTERRUPT_CONFIRM_TIMEOUT.as_secs()
    )
}

async fn wait_for_claude_interrupt_delivery(
    queue: &dyn cas_store::PromptQueueStore,
    notification_id: i64,
) -> std::result::Result<(), String> {
    let deadline = tokio::time::Instant::now() + CLAUDE_INTERRUPT_CONFIRM_TIMEOUT;
    let mut last_report = None;
    loop {
        match queue.message_delivery_report(notification_id) {
            Ok(Some(report)) => {
                if interrupt_delivery_is_observed(&report) {
                    return Ok(());
                }
                if let Some(failure) = interrupt_delivery_failure(&report) {
                    return Err(format!(
                        "Could not interrupt Claude teammate for notification {notification_id}: {failure}"
                    ));
                }
                last_report = Some(report);
            }
            Ok(None) => {}
            Err(error) => {
                return Err(format!(
                    "Could not verify Claude interrupt notification {notification_id}: {error}"
                ));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(interrupt_unconfirmed_message(
                notification_id,
                last_report.as_ref(),
            ));
        }
        tokio::time::sleep(CLAUDE_INTERRUPT_CONFIRM_POLL).await;
    }
}

impl CasService {
    pub(in crate::mcp::tools::service) async fn message_send(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_prompt_queue_store;

        let target = req.target.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "target required (agent name, 'supervisor', or 'all_workers')",
            )
        })?;
        let mut message = req.message.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "message required — full message body goes in `message`. \
                 Example: mcp__cas__coordination action=message target=supervisor \
                 summary=\"task blocked\" message=\"cas-abc1 needs ...\"",
            )
        })?;
        let summary = req.summary.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "summary required — a short one-line preview shown in the UI. \
                 Example: summary=\"task blocked on verification\" (required alongside `message`).",
            )
        })?;

        let source = self
            .inner
            .get_agent_id()
            .unwrap_or_else(|_| "unknown".to_string());
        // When agent ID lookup fails but CAS_AGENT_NAME is set (factory mode),
        // resolve display_name from the env var so messages show the correct sender.
        let env_agent_name = std::env::var("CAS_AGENT_NAME").ok();
        let agent_from_store = {
            use crate::store::open_agent_store;
            open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.get(&source).ok())
        };
        // The registered row is the explicit identity for this MCP caller.
        // CAS_AGENT_ROLE is only a bootstrap fallback when no row can be
        // resolved; letting it win here can relabel a registered worker as a
        // supervisor when a supervisor-launched test or server shares a
        // process environment.
        let role = agent_from_store
            .as_ref()
            .map(|a| a.role.to_string())
            .or_else(|| std::env::var("CAS_AGENT_ROLE").ok())
            .unwrap_or_else(|| "primary".to_string());
        let factory_session = std::env::var("CAS_FACTORY_SESSION")
            .ok()
            .filter(|session| !session.trim().is_empty());

        let resolve_supervisor_name = || -> Option<String> {
            if let Ok(name) = std::env::var("CAS_SUPERVISOR_NAME") {
                if !name.trim().is_empty() {
                    return Some(name);
                }
            }
            use crate::store::open_agent_store;
            use cas_types::{AgentRole, AgentStatus};
            open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.list(None).ok())
                .and_then(|agents| {
                    agents
                        .into_iter()
                        .find(|a| {
                            a.role == AgentRole::Supervisor
                                && (a.status == AgentStatus::Active
                                    || a.status == AgentStatus::Idle)
                        })
                        .map(|a| a.name)
                })
        };

        let addressed_logical_supervisor = target.eq_ignore_ascii_case("supervisor");
        let mut peer_supervisor_copy = None;
        let resolved_target = if role == "worker" {
            if target == "supervisor" {
                resolve_supervisor_name().ok_or_else(|| {
                    Self::error(ErrorCode::INVALID_REQUEST,
                        "Cannot resolve 'supervisor' - no CAS_SUPERVISOR_NAME and no active supervisor agent found.")
                })?
            } else if target == "all_workers" {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "Workers cannot broadcast to all_workers",
                ));
            } else {
                let supervisor_name = resolve_supervisor_name();
                let named_supervisor_is_registered = supervisor_name.as_deref() == Some(&target)
                    && crate::store::open_agent_store(&self.inner.cas_root)
                        .ok()
                        .and_then(|store| store.list(None).ok())
                        .is_some_and(|agents| {
                            agents.iter().any(|agent| {
                                agent.role == cas_types::AgentRole::Supervisor
                                    && agent.name.eq_ignore_ascii_case(&target)
                            })
                        });
                if named_supervisor_is_registered {
                    target
                } else {
                    // cas-5068 / GH #335: peer contact is a narrow,
                    // supervisor-visible collision-warning lane, not general
                    // worker chat. Environment names alone must not grant
                    // cross-session access; both sides are typed store rows.
                    use crate::store::open_agent_store;
                    use cas_types::AgentRole;

                    let session = factory_session.as_deref().ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_REQUEST,
                            "Workers can only message their supervisor or a same-session registered peer; this caller has no named factory session. Use target='supervisor' instead",
                        )
                    })?;
                    let source_agent = agent_from_store.as_ref().filter(|agent| {
                        agent.role == AgentRole::Worker
                            && agent.factory_session.as_deref() == Some(session)
                    }).ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_REQUEST,
                            "Workers can only message their supervisor or a same-session registered peer; this caller is not a registered worker in the current factory session. Use target='supervisor' instead",
                        )
                    })?;
                    let agent_store = open_agent_store(&self.inner.cas_root).map_err(|error| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!("Failed to resolve peer message scope: {error}"),
                        )
                    })?;
                    let agents = agent_store.list(None).map_err(|error| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!("Failed to resolve peer message scope: {error}"),
                        )
                    })?;
                    let peer = agents.iter().find(|agent| {
                        agent.role == AgentRole::Worker
                            && agent.id != source_agent.id
                            && agent.name.eq_ignore_ascii_case(&target)
                            && agent.factory_session.as_deref() == Some(session)
                    }).ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_REQUEST,
                            format!(
                                "Workers can only message their supervisor or another registered worker in factory session '{session}'. Use target='supervisor' for '{}'",
                                supervisor_name.unwrap_or_else(|| "<supervisor>".to_string())
                            ),
                        )
                    })?;
                    let supervisor = agents.iter().find(|agent| {
                        agent.role == AgentRole::Supervisor
                            && agent.factory_session.as_deref() == Some(session)
                    }).ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_REQUEST,
                            "Peer messages require a registered supervisor in the same factory session; use target='supervisor' instead",
                        )
                    })?;
                    peer_supervisor_copy = Some(supervisor.name.clone());
                    peer.name.clone()
                }
            }
        } else {
            target
        };

        if role != "worker" && (resolved_target == "owner" || resolved_target.starts_with("inbox:"))
        {
            use crate::store::{
                NotificationPriority, open_agent_store, open_supervisor_queue_store,
            };
            use cas_types::AgentRole;
            use rusqlite::Connection;
            use std::collections::HashSet;

            let display_name = open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.get(&source).ok())
                .map(|agent| {
                    if agent.role == AgentRole::Supervisor {
                        "supervisor".to_string()
                    } else {
                        agent.name
                    }
                })
                .unwrap_or_else(|| source.clone());

            let inbox_id = if resolved_target == "owner" {
                "owner".to_string()
            } else {
                resolved_target
                    .strip_prefix("inbox:")
                    .unwrap_or("owner")
                    .to_string()
            };

            let engaged = (|| -> std::result::Result<bool, rusqlite::Error> {
                let agent_name = std::env::var("CAS_AGENT_NAME").unwrap_or_default();
                if agent_name.is_empty() {
                    return Ok(false);
                }

                let manager = crate::ui::factory::SessionManager::new();
                let sessions = manager
                    .list_sessions()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;

                let session = sessions
                    .into_iter()
                    .find(|session| session.metadata.supervisor.name == agent_name);

                let Some(session) = session else {
                    return Ok(false);
                };

                let mut targets: HashSet<String> = HashSet::new();
                targets.insert(session.metadata.supervisor.name.clone());
                targets.insert("all_workers".to_string());
                for worker in &session.metadata.workers {
                    targets.insert(worker.name.clone());
                }

                if targets.is_empty() {
                    return Ok(false);
                }

                let db_path = self.inner.cas_root.join("cas.db");
                let conn = Connection::open(&db_path)?;

                let mut target_vec: Vec<String> = targets.into_iter().collect();
                target_vec.sort();
                let placeholders = std::iter::repeat_n("?", target_vec.len())
                    .collect::<Vec<_>>()
                    .join(", ");

                let sql = format!(
                    "SELECT 1 FROM prompt_queue WHERE source = ? AND target IN ({placeholders}) LIMIT 1"
                );
                let mut stmt = conn.prepare(&sql)?;

                let mut params: Vec<Box<dyn rusqlite::ToSql>> =
                    Vec::with_capacity(1 + target_vec.len());
                params.push(Box::new("openclaw".to_string()));
                for target in target_vec {
                    params.push(Box::new(target));
                }

                let mut rows = stmt.query(rusqlite::params_from_iter(
                    params.iter().map(|param| param.as_ref()),
                ))?;
                Ok(rows.next()?.is_some())
            })()
            .unwrap_or(false);

            if !engaged {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "External inbox is not engaged for this session yet. Owner must message this factory session first (via OpenClaw) before agents can reply to 'owner'.",
                ));
            }

            let queue = open_supervisor_queue_store(&self.inner.cas_root).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open supervisor queue: {error}"),
                )
            })?;

            let payload = serde_json::json!({
                "schema_version": 1,
                "type": "message",
                "from": display_name,
                "message": message,
            })
            .to_string();

            let notification_id = queue
                .notify(&inbox_id, "message", &payload, NotificationPriority::Normal)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to queue external message: {error}"),
                    )
                })?;

            return Ok(Self::success(format!(
                "External message queued\n\nID: {}\nInbox: {}\nFrom: {} ({})\nMessage: {}",
                notification_id,
                inbox_id,
                display_name,
                role,
                truncate_str(&message, 100)
            )));
        }

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open message queue: {error}"),
            )
        })?;

        let display_name = {
            use crate::store::open_agent_store;
            use cas_types::AgentRole;
            let agent_store = open_agent_store(&self.inner.cas_root).ok();
            agent_store
                .and_then(|store| store.get(&source).ok())
                .map(|agent| {
                    if agent.role == AgentRole::Supervisor {
                        "supervisor".to_string()
                    } else {
                        agent.name
                    }
                })
                .or_else(|| env_agent_name.clone())
                .unwrap_or_else(|| source.clone())
        };

        // cas-4a27 (GH #334): reply inference intentionally requires a
        // recipient-surfacing receipt, so it cannot turn an unrelated later
        // message into proof that an escalation was read. A supervisor that
        // DID read an escalation needs a precise way to say so. Link this
        // reply to the durable notification explicitly, validate the two
        // endpoints, and render the reference on the worker-facing message.
        // This is strong evidence for exactly one row; the conservative
        // generic reply-inference rule below remains unchanged.
        let explicit_reply_to = req.in_reply_to;
        if let Some(notification_id) = explicit_reply_to {
            let prior = queue
                .message_delivery_report(notification_id)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to inspect in_reply_to message {notification_id}: {error}"),
                    )
                })?
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!("in_reply_to notification {notification_id} does not exist"),
                    )
                })?;
            let expected_prior_sources = [
                resolved_target.as_str(),
                if addressed_logical_supervisor {
                    "supervisor"
                } else {
                    ""
                },
            ];
            let expected_prior_targets = [
                display_name.as_str(),
                env_agent_name.as_deref().unwrap_or_default(),
                if role == "supervisor" { "supervisor" } else { "" },
            ];
            let source_matches = expected_prior_sources
                .iter()
                .filter(|name| !name.is_empty())
                .any(|name| prior.source.eq_ignore_ascii_case(name));
            let target_matches = expected_prior_targets
                .iter()
                .filter(|name| !name.is_empty())
                .any(|name| prior.target.eq_ignore_ascii_case(name));
            if !source_matches || !target_matches {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "in_reply_to notification {notification_id} is {} -> {}, not a direct message from {resolved_target} to this sender",
                        prior.source, prior.target
                    ),
                ));
            }
            message = format!(
                "[CAS reply: explicitly acknowledges notification_id={notification_id}]\n{message}"
            );
        }

        // cas-bc8c: a genuine merge request receives an immutable envelope so
        // transport can suppress it once its requested tip has landed. That
        // envelope is a message type, not a property of the sender's task
        // state: workers must still be able to report a blocker or rejected
        // close after their only parked branch lands (cas-89e1 / GH #328).
        // Therefore an unmarked worker message remains free-form even if it
        // names the one AwaitingMerge task; only an explicit merge request is
        // eligible for revalidation and suppression.
        if role == "worker" && req.merge_request.unwrap_or(false) {
            use crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha;
            use crate::mcp::tools::core::task::repo_context::{
                resolve_repo_context, resolve_repo_context_from_local_root,
            };
            use crate::prompt_revalidation::{
                MergeRequestDecision, MergeRequestEnvelope, attach_merge_request_envelope,
                merge_landed_guidance, revalidate_merge_request, select_unambiguous_merge_task,
            };
            use crate::store::open_task_store_local;
            use cas_types::TaskStatus;

            let merge_task = open_task_store_local(&self.inner.cas_root)
                .ok()
                .and_then(|store| {
                    let parked = store.list(Some(TaskStatus::AwaitingMerge)).ok()?;
                    select_unambiguous_merge_task(&parked, &display_name, req.task_id.as_deref())
                        .cloned()
                });

            if let Some(task) = merge_task
                && let Some(work_target) = task.deliverables.work_target.as_ref()
                && let Ok(repo) = {
                    match resolve_repo_context_from_local_root(&self.inner.cas_root, work_target) {
                        Ok(repo) => {
                            tracing::debug!(
                                task_id = %task.id,
                                resolution = "local_checkout",
                                repo_root = %repo.repo_root.display(),
                                git_common_dir = %repo.git_common_dir.display(),
                                "merge request revalidation selected explicit local checkout"
                            );
                            Ok(repo)
                        }
                        Err(local_error) => {
                            tracing::debug!(
                                task_id = %task.id,
                                resolution = "local_checkout_miss",
                                error = %local_error,
                                "merge request revalidation falling back to host registry"
                            );
                            match resolve_repo_context(&self.inner.cas_root, work_target) {
                                Ok(repo) => {
                                    tracing::debug!(
                                        task_id = %task.id,
                                        resolution = "host_registry_fallback",
                                        repo_root = %repo.repo_root.display(),
                                        git_common_dir = %repo.git_common_dir.display(),
                                        "merge request revalidation selected host registry checkout"
                                    );
                                    Ok(repo)
                                }
                                Err(error) => {
                                    tracing::debug!(
                                        task_id = %task.id,
                                        resolution = "host_registry_miss",
                                        error = %error,
                                        "merge request revalidation could not resolve checkout"
                                    );
                                    Err(error)
                                }
                            }
                        }
                    }
                }
            {
                let branch = crate::prompt_revalidation::merge_request_branch(Some(&task));
                // cas-b17c (GH #703): revalidate against the LIVE branch tip.
                // This used to prefer `factory_branch_anchor`, which records
                // the previous merge — so a commit pushed after that merge was
                // judged as the already-merged sha and the request was
                // suppressed with "Merge already landed". The anchor is kept
                // only as a reported datum so the drift is visible downstream.
                let recorded_anchor = task.deliverables.factory_branch_anchor.clone();
                if let Some(branch) = branch
                    && let Some(branch_tip) = crate::prompt_revalidation::resolve_live_branch_tip(
                        &repo.repo_root,
                        &branch,
                        recorded_anchor.as_deref(),
                    )
                {
                    match revalidate_merge_request(
                        &repo.repo_root,
                        &branch_tip,
                        &repo.target_branch,
                    ) {
                        MergeRequestDecision::AlreadyIntegrated { target_tip } => {
                            return Ok(Self::success(merge_landed_guidance(
                                &task.id,
                                &branch_tip,
                                &repo.target_branch,
                                &target_tip,
                            )));
                        }
                        MergeRequestDecision::Pending { target_tip } => {
                            message = attach_merge_request_envelope(
                                &message,
                                &MergeRequestEnvelope {
                                    task_id: task.id,
                                    // Live tip; the anchor rides along only
                                    // when it disagrees, so the supervisor
                                    // sees the drift instead of inferring it.
                                    anchor_tip: recorded_anchor
                                        .filter(|anchor| anchor != &branch_tip),
                                    branch_tip,
                                    target_branch: repo.target_branch,
                                    target_branch_tip: target_tip,
                                },
                            );
                        }
                        MergeRequestDecision::Unverifiable => {
                            tracing::warn!(
                                task_id = %task.id,
                                "cas-bc8c: merge request tips could not be verified; delivering free-form message unchanged"
                            );
                        }
                    }
                }
            }
        }

        // cas-6913: "Message queued" reads as delivery confirmation, but a
        // message addressed to a not-yet-registered worker name (the common
        // spawn-then-immediately-assign sequence) sits in the queue until
        // that name shows up in the agent store — the supervisor has no
        // signal this happened. Check registration state up front so the
        // response can say so honestly. `all_workers` is a broadcast, not a
        // single-target claim, so it's always reported as delivered framing.
        //
        // cas-73c8: `director` is a permanent team member (TeamsManager
        // init_team_config) and the source of inbound teammate messages, but
        // is not an agent_store registration. Treat it as always registered
        // so outbound replies after an inbound director message are not
        // reported as "not yet registered".
        let resolved_target_agent = {
            use crate::store::open_agent_store;
            open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.list(None).ok())
                .and_then(|agents| {
                    agents
                        .into_iter()
                        .find(|agent| agent.name.eq_ignore_ascii_case(&resolved_target))
                })
        };
        let target_is_registered = resolved_target == "all_workers"
            || resolved_target == "supervisor"
            || resolved_target.eq_ignore_ascii_case("director")
            || resolved_target_agent.is_some();

        let urgent = req.urgent.unwrap_or(false);
        // Urgent messages break the target's in-flight turn, so they must jump
        // the queue ahead of any backlog: force Critical priority when urgent
        // and no explicit priority was given.
        let priority = req.priority.as_deref().map(|p| match p {
            "critical" | "0" => cas_store::NotificationPriority::Critical,
            "high" | "1" => cas_store::NotificationPriority::High,
            _ => cas_store::NotificationPriority::Normal,
        });
        let priority = if urgent && priority.is_none() {
            Some(cas_store::NotificationPriority::Critical)
        } else {
            priority
        };

        if urgent && !target_is_registered {
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "Could not interrupt '{resolved_target}': target is not registered, so no live pane can be interrupted"
                ),
            ));
        }

        // cas-b269 review 2: halt fan-out is session-scoped, authorized by
        // AgentRole::Supervisor|Director (and display fallback), fail-closed
        // on store errors, generation-stamped, and all-or-none with enqueue
        // (compensate halt writes if enqueue fails).
        let mut halt_compensation: Vec<(String, std::collections::HashMap<String, String>)> =
            Vec::new();
        let mut halt_bindings: Vec<(String, u64)> = Vec::new();
        {
            use crate::mcp::tools::core::task::lifecycle::stale_close_guard::{
                HaltWorkerCandidate, apply_halt_metadata, halt_targets_for_urgent,
                is_merge_reclose_exempt_urgent, may_source_role_set_halt, may_source_set_halt,
                next_halt_generation, session_scoped_worker_names, should_persist_urgent_halt,
            };
            use crate::store::{open_agent_store, open_task_store};
            use cas_types::{AgentRole, TaskStatus};

            // Prefer typed role from agent store when available.
            let source_role_for_halt = agent_from_store
                .as_ref()
                .map(|a| a.role.to_string())
                .unwrap_or_else(|| role.clone());
            let source_authorized = agent_from_store
                .as_ref()
                .map(|a| may_source_role_set_halt(a.role))
                .unwrap_or_else(|| may_source_set_halt(&display_name, &source_role_for_halt));

            if urgent
                && (source_authorized || may_source_set_halt(&display_name, &source_role_for_halt))
            {
                let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!(
                            "Failed to open agent store for urgent halt (cas-b269, fail closed): {e}"
                        ),
                    )
                })?;
                let agents = agent_store.list(None).map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!(
                            "Failed to list agents for urgent halt (cas-b269, fail closed): {e}"
                        ),
                    )
                })?;

                let worker_candidates: Vec<HaltWorkerCandidate> = agents
                    .iter()
                    .filter(|a| a.role == AgentRole::Worker)
                    .map(|a| HaltWorkerCandidate {
                        name: a.name.clone(),
                        factory_session: a.factory_session.clone(),
                    })
                    .collect();
                let session_workers =
                    session_scoped_worker_names(&worker_candidates, factory_session.as_deref());

                // cas-126b: an urgent "MERGE DONE → re-close now" hand-off both
                // wakes the parked worker AND (before this guard) armed
                // halt_task_work — deadlocking the very re-close it asks for.
                // Skip the halt fan-out when this urgent send is close/verify
                // guidance that references, as a bounded token, a task that is
                // (a) currently AwaitingMerge AND (b) assigned to THIS urgent's
                // target worker. The assignee binding is a scope/authorization
                // gate: an urgent to worker B must not skip halt because its
                // text happens to name worker A's parked task. The exemption
                // only skips the halt flag — the message is still
                // enqueued+injected, and the factory-branch merge gate in
                // close_ops remains the sole authority on close success, so a
                // re-close sent before the merge is visible still rejects with
                // MERGE REQUIRED (never a false success). Fail closed — if the
                // task store can't be read, keep the original halt behavior.
                let reclose_exempt = match open_task_store(&self.inner.cas_root)
                    .ok()
                    .and_then(|ts| ts.list(Some(TaskStatus::AwaitingMerge)).ok())
                {
                    Some(tasks) => {
                        let target_awaiting_ids: Vec<String> = tasks
                            .into_iter()
                            .filter(|t| {
                                t.assignee
                                    .as_deref()
                                    .map(|a| a.eq_ignore_ascii_case(&resolved_target))
                                    .unwrap_or(false)
                            })
                            .map(|t| t.id)
                            .collect();
                        is_merge_reclose_exempt_urgent(&message, &target_awaiting_ids)
                    }
                    None => false,
                };

                if reclose_exempt {
                    tracing::debug!(
                        target = %resolved_target,
                        "cas-126b: skipping halt_task_work for merge-complete re-close urgent"
                    );
                } else if should_persist_urgent_halt(
                    urgent,
                    &display_name,
                    &source_role_for_halt,
                    &resolved_target,
                    &session_workers,
                ) {
                    let targets = halt_targets_for_urgent(&resolved_target, &session_workers);
                    let halt_generation = next_halt_generation();
                    for target_name in &targets {
                        // Match by name + session so same-name cross-session
                        // peers are not halted.
                        let Some(mut agent) = agents
                            .iter()
                            .find(|a| {
                                a.role == AgentRole::Worker
                                    && a.name.eq_ignore_ascii_case(target_name)
                                    && a.visible_to_factory_session(factory_session.as_deref())
                            })
                            .cloned()
                        else {
                            continue;
                        };
                        halt_compensation.push((agent.id.clone(), agent.metadata.clone()));
                        apply_halt_metadata(&mut agent.metadata, halt_generation);
                        if let Err(e) = agent_store.update(&agent) {
                            // Compensate any prior successful writes.
                            for (id, prev) in halt_compensation.drain(..) {
                                if let Ok(mut a) = agent_store.get(&id) {
                                    a.metadata = prev;
                                    let _ = agent_store.update(&a);
                                }
                            }
                            return Err(Self::error(
                                ErrorCode::INTERNAL_ERROR,
                                format!(
                                    "Failed to persist halt_task_work for {target_name} \
                                     before urgent enqueue (cas-b269): {e}"
                                ),
                            ));
                        }
                        halt_bindings.push((agent.id.clone(), halt_generation));
                    }
                }
            } else if urgent
                && !source_authorized
                && !may_source_set_halt(&display_name, &source_role_for_halt)
            {
                tracing::debug!(
                    source = %display_name,
                    role = %source_role_for_halt,
                    "cas-b269: ignoring halt for unauthorized source"
                );
            }
        }

        // cas-f9e8 telemetry: measure the wall-clock spent inside the DB
        // insert and log it alongside the caller-visible message id, so a
        // future investigator can bisect whether stalls live in send-side
        // persistence, daemon wake, daemon poll, or downstream inject. Logged
        // at debug so normal sessions stay quiet; enable via
        // `RUST_LOG=cas::coordination=debug`.
        let enqueue_started = std::time::Instant::now();
        let peer_copy_message = peer_supervisor_copy.as_ref().map(|_| {
            format!(
                "Peer worker message copy — from {display_name} to {resolved_target}.\n\n{message}"
            )
        });
        let (message_id, duplicate_suppressed, supervisor_copy_id) =
            if let Some(supervisor) = peer_supervisor_copy.as_deref() {
                let session = factory_session
                    .as_deref()
                    .expect("peer scope requires a factory session");
                match queue.enqueue_worker_peer_with_supervisor_copy(
                    &display_name,
                    &resolved_target,
                    supervisor,
                    &message,
                    peer_copy_message.as_deref().expect("peer copy text"),
                    session,
                    Some(summary.as_str()),
                    priority,
                    urgent,
                ) {
                    Ok(ids) => (ids.recipient_id, false, Some(ids.supervisor_copy_id)),
                    Err(error) => {
                        return Err(Self::error(
                            ErrorCode::INVALID_REQUEST,
                            format!("Could not queue scoped peer message: {error}"),
                        ));
                    }
                }
            } else {
                // cas-15f2: stamp the row with the RECIPIENT's session, not the
                // sender's, so the recipient's daemon and inbox can select it.
                let row_session = row_factory_session(
                    &resolved_target,
                    resolved_target_agent.as_ref(),
                    factory_session.as_deref(),
                );
                let enqueue_outcome = match queue.enqueue_urgent_with_outcome(
                    &display_name,
                    &resolved_target,
                    &message,
                    row_session.as_deref(),
                    Some(summary.as_str()),
                    priority,
                    urgent,
                ) {
                    Ok(id) => id,
                    Err(error) => {
                        // Compensate halt fan-out so we never leave halt without the
                        // corresponding urgent message (all-or-none).
                        if !halt_compensation.is_empty() {
                            use crate::store::open_agent_store;
                            if let Ok(agent_store) = open_agent_store(&self.inner.cas_root) {
                                for (id, prev) in halt_compensation.drain(..) {
                                    if let Ok(mut a) = agent_store.get(&id) {
                                        a.metadata = prev;
                                        let _ = agent_store.update(&a);
                                    }
                                }
                            }
                        }
                        return Err(Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!("Failed to queue message: {error}"),
                        ));
                    }
                };
                let duplicate_suppressed = matches!(
                    enqueue_outcome,
                    cas_store::EnqueueOutcome::SuppressedDuplicate(_)
                );
                (enqueue_outcome.id(), duplicate_suppressed, None)
            };

        if let Some(notification_id) = explicit_reply_to {
            queue.ack(notification_id).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!(
                        "Reply {message_id} queued but failed to confirm in_reply_to notification {notification_id}: {error}"
                    ),
                )
            })?;
        }

        // cas-85fd: an urgent halt is a course-correction exchange, not a
        // permanent worker state. Once the row exists, bind every halt this
        // send armed to its exact prompt id. A later urgent has a newer
        // generation and is deliberately left untouched by this write.
        if !halt_bindings.is_empty() {
            use crate::mcp::tools::core::task::lifecycle::stale_close_guard::bind_halt_to_prompt_if_generation;
            use crate::store::open_agent_store;
            if let Ok(agent_store) = open_agent_store(&self.inner.cas_root) {
                for (agent_id, generation) in &halt_bindings {
                    let Ok(mut agent) = agent_store.get(agent_id) else {
                        continue;
                    };
                    if bind_halt_to_prompt_if_generation(
                        &mut agent.metadata,
                        *generation,
                        message_id,
                    ) {
                        if let Err(error) = agent_store.update(&agent) {
                            tracing::warn!(
                                agent_id,
                                message_id,
                                error = %error,
                                "could not bind urgent halt to its prompt; task start remains the recovery path"
                            );
                        }
                    }
                }
            }
        }

        // cas-6ad2: sending a response is a recipient-side consumption signal
        // for prior messages from that counterparty. The old explicit
        // message_ack API was never invoked by factory prompts, leaving every
        // acted-on message stuck at Delivered/AwaitingAck.
        //
        // cas-99d2 (GH #126): "a reply happened" is NOT on its own evidence
        // that any particular earlier message was consumed. The store now
        // requires the reply to post-date the message's transport handoff AND a
        // surfacing receipt to exist for it — so this call needs the reply's
        // own enqueue instant, read back from the row just written rather than
        // sampled as "now" (which would drift past the true enqueue time by
        // however long the surrounding bookkeeping takes and could sweep in a
        // message delivered in that window).
        //
        // Supervisors have two queue identities: outbound source
        // `"supervisor"` and their generated pane/display name as an inbound
        // target. Workers use their display name both ways. Include both alias
        // shapes and advance only already transport-delivered rows.
        let mut recipient_aliases = vec![display_name.as_str()];
        if let Some(name) = env_agent_name.as_deref()
            && !recipient_aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(name))
        {
            recipient_aliases.push(name);
        }
        let resolved_target_role = resolved_target_agent.as_ref().map(|agent| agent.role);
        let resolved_target_cli = resolved_target_agent
            .as_ref()
            .map(|agent| crate::mcp::tools::service::factory_ops::worker_cli_from_agent(agent));
        let target_is_supervisor = addressed_logical_supervisor
            || resolved_target_role == Some(cas_types::AgentRole::Supervisor);
        let mut counterparty_aliases = vec![resolved_target.as_str()];
        if role == "worker"
            && target_is_supervisor
            && !counterparty_aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case("supervisor"))
        {
            counterparty_aliases.push("supervisor");
        }
        let reply_enqueued_at = queue
            .message_delivery_report(message_id)
            .ok()
            .flatten()
            .map(|report| report.enqueued_at)
            .unwrap_or_else(chrono::Utc::now);
        if let Err(error) = queue.ack_delivered_for_recipient(
            &recipient_aliases,
            &counterparty_aliases,
            factory_session.as_deref(),
            reply_enqueued_at,
        ) {
            tracing::warn!(
                message_id,
                source = %display_name,
                target_agent = %resolved_target,
                error = %error,
                "failed to confirm prior delivered messages after recipient response"
            );
        }

        // cas-85fd: release only the halt bound to an urgent that this worker
        // demonstrably consumed and answered. `Confirmed` is the queue's
        // existing proof: the urgent had a transport handoff + surfacing
        // receipt and the worker's reply post-dated both. Do not clear a
        // legacy/unbound halt or a newer halt that replaced this exchange.
        if role == "worker" && target_is_supervisor {
            use crate::mcp::tools::core::task::lifecycle::stale_close_guard::{
                clear_halt_metadata, halt_prompt_id,
            };
            use crate::store::open_agent_store;
            if let Ok(agent_store) = open_agent_store(&self.inner.cas_root)
                && let Ok(mut agent) = agent_store.get(&source)
                && let Some(prompt_id) = halt_prompt_id(&agent.metadata)
                && matches!(
                    queue.message_status(prompt_id),
                    Ok(Some(cas_store::MessageStatus::Confirmed))
                )
            {
                clear_halt_metadata(&mut agent.metadata);
                if let Err(error) = agent_store.update(&agent) {
                    tracing::warn!(
                        agent_id = %source,
                        prompt_id,
                        error = %error,
                        "could not release confirmed urgent halt"
                    );
                }
            }
        }

        let persist_latency_ms = enqueue_started.elapsed().as_secs_f64() * 1000.0;
        tracing::debug!(
            target: "cas::coordination",
            stage = "enqueue",
            channel = "prompt_queue",
            message_id,
            source = %display_name,
            target_agent = %resolved_target,
            priority = ?priority,
            duplicate_suppressed,
            persist_ms = persist_latency_ms,
            "prompt_queue enqueue resolved"
        );

        // Notify daemon that prompt queue has new data (best-effort)
        if !duplicate_suppressed {
            let notify_started = std::time::Instant::now();
            let notify_outcome = cas_factory::notify_daemon(&self.inner.cas_root);
            let notify_latency_ms = notify_started.elapsed().as_secs_f64() * 1000.0;
            match notify_outcome {
                Ok(()) => {
                    tracing::debug!(
                        target: "cas::coordination",
                        stage = "notify",
                        channel = "prompt_queue",
                        message_id,
                        notify_ms = notify_latency_ms,
                        "daemon wakeup signal sent"
                    );
                }
                Err(ref e) => {
                    // Kept as debug because this is expected when the daemon is
                    // not running (e.g. `cas serve` standalone sessions).
                    tracing::debug!(
                        target: "cas::coordination",
                        stage = "notify",
                        channel = "prompt_queue",
                        message_id,
                        notify_ms = notify_latency_ms,
                        error = %e,
                        "daemon wakeup signal failed (daemon may not be running)"
                    );
                }
            }
        }

        // cas-71d9 (GH #269): an urgent write to a Claude teammate is not a
        // successful interrupt until the daemon's pane-output probe records a
        // recipient-side transport receipt. Wait through that bounded probe
        // window and fail explicitly when no observation arrives. The queue
        // row remains durable, so a late retry is still possible, but the
        // caller can no longer mistake enqueue success for a broken wait state.
        if urgent && resolved_target_cli == Some(cas_mux::SupervisorCli::Claude) {
            wait_for_claude_interrupt_delivery(&*queue, message_id)
                .await
                .map_err(|message| Self::error(ErrorCode::INTERNAL_ERROR, message))?;
        }

        // cas-6913 / cas-893c: honest delivery-status line. Urgent takes
        // priority in the wording since it describes the delivery MECHANISM
        // (interrupt) — but an urgent message to an unregistered target
        // still can't interrupt a turn that doesn't exist yet, so the
        // registration caveat wins even for urgent sends.
        //
        // cas-893c: the non-urgent line previously read "queued for next
        // poll (target is registered)", which a sender reasonably read as
        // "delivered". It is not: the daemon enqueues this row and will
        // attempt a transport handoff (teams-inbox file write or PTY
        // inject) on its next tick, but that handoff succeeding is not the
        // same as the recipient actually reading it — a Claude teammate
        // only polls its inbox at a turn boundary, which an idle worker
        // parked awaiting input may not reach on its own for a long time.
        // The daemon now also nudges an idle recipient directly over PTY
        // (see queue_and_events.rs `worker_looks_idle`), but that nudge is
        // best-effort, not guaranteed — so the response must not claim
        // delivery either way. Use `message_status` to check the actual
        // stage reached.
        let delivery_status = if !target_is_registered {
            "Delivery: queued — target not yet registered, will deliver on registration\n"
                .to_string()
        } else if urgent {
            "Delivery: interrupt-and-redirect (breaks the target's in-flight turn, then injects)\n"
                .to_string()
        } else {
            format!(
                "Delivery: enqueued (target is registered) — not yet confirmed delivered. The \
                 daemon will attempt transport handoff on its next tick and nudge the recipient if \
                 it looks idle, but neither guarantees the recipient has read it. Check \
                 `message_status` with `notification_id={message_id}` if you need to know whether \
                 this landed before escalating.\n"
            )
        };

        let message_id_text = message_id.to_string();
        let enqueue_result = if duplicate_suppressed {
            "suppressed_duplicate"
        } else {
            "created"
        };
        let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
            &self.inner.cas_root,
            "coordination_message",
            &[
                ("message_id", &message_id_text),
                ("source", &display_name),
                ("target", &resolved_target),
                ("summary", &summary),
                ("urgent", if urgent { "true" } else { "false" }),
                ("enqueue_result", enqueue_result),
            ],
        );

        if duplicate_suppressed {
            return Ok(Self::success(format!(
                "Duplicate message suppressed\n\nnotification_id: {}\nFrom: {} ({})\nTo: {}\n\
                 Delivery: no new queue row — an identical message was delivered recently and \
                 still awaits confirmation. Check `message_status` with \
                 `notification_id={message_id}` before retrying.\nMessage: {}",
                message_id,
                display_name,
                role,
                resolved_target,
                truncate_str(&message, 100)
            )));
        }

        Ok(Self::success(format!(
            "{} queued\n\nnotification_id: {}\n{}From: {} ({})\nTo: {}\n{}Message: {}",
            if urgent { "URGENT message" } else { "Message" },
            message_id,
            supervisor_copy_id
                .map(|id| format!("supervisor_copy_notification_id: {id}\n"))
                .unwrap_or_default(),
            display_name,
            role,
            resolved_target,
            delivery_status,
            truncate_str(&message, 100)
        )))
    }

    pub(in crate::mcp::tools::service) async fn inbox_poll(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_prompt_queue_store};

        let agent_id = self.inner.get_agent_id().ok();
        let registered_agent = agent_id.as_deref().and_then(|id| {
            open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.get(id).ok())
        });
        let recipient = resolve_inbox_recipient(
            registered_agent.as_ref().map(|agent| agent.name.clone()),
            std::env::var("CAS_AGENT_NAME").ok(),
        )
        .ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                "inbox_poll requires a registered agent identity",
            )
        })?;
        let factory_session = std::env::var("CAS_FACTORY_SESSION")
            .ok()
            .filter(|session| !session.trim().is_empty())
            .or_else(|| {
                registered_agent
                    .as_ref()
                    .and_then(|agent| agent.factory_session.clone())
            });
        let limit = req.limit.unwrap_or(10).min(100);

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {error}"),
            )
        })?;
        // cas-d047 (GH #69): a freshly spawned worker must never be handed a
        // months-old item addressed to a recycled name. The daemon sweeps
        // these too, but a worker can poll in a session with no daemon tick
        // yet (or none at all), so quarantine on the read path as well — the
        // store keeps the row and its forensics, it just stops being
        // deliverable, and every withheld row is named in the log.
        match queue.expire_stale_pending(cas_store::PROMPT_QUEUE_STALE_TTL_SECS) {
            Ok(stale) => {
                for row in &stale {
                    tracing::warn!(
                        target: "cas::coordination",
                        stage = "stale_quarantine",
                        prompt_id = row.id,
                        source = %row.source,
                        target_agent = %row.target,
                        created_at = %row.created_at.to_rfc3339(),
                        age_secs = (chrono::Utc::now() - row.created_at).num_seconds(),
                        recipient = %recipient,
                        "cas-d047: withheld stale queue item from inbox delivery"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    target: "cas::coordination",
                    stage = "stale_quarantine",
                    %error,
                    "cas-d047: stale queue sweep failed; stale rows remain filtered out of this poll"
                );
            }
        }

        // This is an at-most-once claim for the polling API: the store records
        // recipient-seen state in the same transaction that selects the rows,
        // before this MCP response is handed back. That prevents concurrent
        // duplicate delivery, but a response lost after this point is not
        // replayed by a later poll. Daemon transport state remains independent.
        // cas-3bf1 (GH #176): poll every alias this agent answers to, not just
        // its registered pane name. A supervisor answers to two, and rows
        // addressed to the logical `supervisor` alias were previously
        // UNREACHABLE from here — the unseen predicate matches
        // `q.target = ?alias OR q.target = 'all_workers'`, and `supervisor` was
        // never passed as an alias, so supervisor-addressed mail was visible
        // only via the turn-start hook. Measured on the live queue before this
        // change: 40 of 50 `supervisor`-addressed rows never receipted, against
        // 15 of 59 for the pane name.
        let aliases = crate::harness_policy::inbox_aliases(
            &recipient,
            registered_agent
                .as_ref()
                .is_some_and(|agent| agent.role == cas_types::AgentRole::Supervisor),
        );
        let mut messages: Vec<cas_store::QueuedPrompt> = Vec::new();
        for alias in &aliases {
            let remaining = limit.saturating_sub(messages.len());
            if remaining == 0 {
                break;
            }
            let found = queue
                .poll_unseen_for_recipient(alias, factory_session.as_deref(), remaining)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to poll recipient inbox: {error}"),
                    )
                })?;
            for row in found {
                // A broadcast comes back from BOTH aliases; handing it to the
                // caller twice in one response is the duplicate this task must
                // not create while removing the cross-turn one.
                if messages.iter().any(|existing| existing.id == row.id) {
                    continue;
                }
                messages.push(row);
            }
        }
        // Retire every polled row for the whole identity, so the turn-start
        // hook does not re-inject what this poll just handed over.
        crate::harness_policy::mirror_receipts_across_aliases(&*queue, &messages, &aliases);

        if messages.is_empty() {
            return Ok(Self::success(format!("No unread messages for {recipient}")));
        }

        // cas-99d2 (GH #127): a row the daemon already handed to this
        // recipient's transport is not new mail, and the poll used to render it
        // byte-identically with no way to tell. Classify each row before
        // rendering: withhold one whose solicited transition already happened,
        // and mark the rest as repeats.
        let task_store = crate::store::open_task_store_local(&self.inner.cas_root).ok();
        let mut rendered = 0usize;
        // Every withholding path below follows one policy: first identify the
        // typed action the row requests, then require fresh positive evidence
        // that action is moot. Missing/unreadable state delivers. A withheld
        // row is always retained here with its notification id and an
        // operator-readable reason; no suppression may disappear silently.
        let mut withheld: Vec<(i64, String)> = Vec::new();
        let mut redelivered = 0usize;
        let mut body = String::new();
        for message in &messages {
            // A supervisor can claim a queue row directly through inbox_poll,
            // bypassing the daemon's transport-time check. Re-run the exact
            // merge-request predicate here so an invalidated delivery anchor
            // cannot become actionable merely because this is the first
            // delivery surface the supervisor used.
            let stale_merge_request = crate::prompt_revalidation::parse_merge_request_envelope(
                &message.prompt,
            )
            .and_then(|envelope| {
                let store = task_store.as_ref()?;
                let task = store.get(&envelope.task_id).ok()?;
                use crate::mcp::tools::core::task::repo_context::resolve_repo_context;
                let repo_root = task
                    .deliverables
                    .work_target
                    .as_ref()
                    .and_then(|work_target| {
                        resolve_repo_context(&self.inner.cas_root, work_target).ok()
                    })
                    .map(|repo| repo.repo_root)
                    .unwrap_or_else(|| {
                        self.inner
                            .cas_root
                            .parent()
                            .unwrap_or(&self.inner.cas_root)
                            .to_path_buf()
                    });
                let git = crate::prompt_revalidation::revalidate_merge_request(
                    &repo_root,
                    &envelope.branch_tip,
                    &envelope.target_branch,
                );
                (!matches!(
                    crate::prompt_revalidation::merge_request_delivery_decision(
                        Some(&task),
                        &envelope,
                        &git,
                    ),
                    crate::prompt_revalidation::MergeRequestDelivery::Deliver
                ))
                .then_some(envelope.task_id)
            });
            if let Some(task_id) = stale_merge_request {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "inbox_withheld_stale_merge_request",
                    prompt_id = message.id,
                    recipient = %recipient,
                    task_id = %task_id,
                    "withheld a merge request whose delivery anchor no longer holds at inbox pop"
                );
                withheld.push((
                    message.id,
                    format!("merge request for {task_id}, already done"),
                ));
                continue;
            }

            // Lifecycle rows have the same direct-poll bypass. In particular,
            // a queued MERGE REQUIRED relay must not survive request_changes,
            // cancel, reset, or a completed merge merely because no daemon
            // tick ran before the supervisor polled.
            let lifecycle_task = crate::prompt_revalidation::parse_lifecycle_envelope(
                &message.prompt,
            )
            .and_then(|envelope| {
                task_store
                    .as_ref()
                    .and_then(|store| store.get(&envelope.task_id).ok())
            });
            let stale_lifecycle =
                lifecycle_relay_is_stale_at_inbox_pop(&message.prompt, lifecycle_task.as_ref());
            if stale_lifecycle {
                let task_id = crate::prompt_revalidation::parse_lifecycle_envelope(&message.prompt)
                    .map(|envelope| envelope.task_id)
                    .unwrap_or_default();
                tracing::info!(
                    target: "cas::coordination",
                    stage = "inbox_withheld_stale_merge_relay",
                    prompt_id = message.id,
                    recipient = %recipient,
                    task_id = %task_id,
                    "withheld a stale lifecycle relay at inbox pop"
                );
                withheld.push((
                    message.id,
                    format!("lifecycle relay for {task_id}, already done"),
                ));
                continue;
            }
            let solicited_task = assignment_solicited_task_id(&message.prompt);
            let terminal_assignment = match (&solicited_task, task_store.as_ref()) {
                (Some(task_id), Some(store)) => store.get(task_id).ok().and_then(|task| {
                    assignment_targets_terminal_task(&message.prompt, task.status)
                        .map(|task_id| (task_id, task.status))
                }),
                _ => None,
            };
            if let Some((task_id, status)) = terminal_assignment {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "inbox_withheld_terminal_assignment",
                    prompt_id = message.id,
                    recipient = %recipient,
                    task_id = %task_id,
                    status = %status,
                    "cas-8aee: withheld a queued assignment whose task is terminal"
                );
                withheld.push((
                    message.id,
                    format!("assignment for {task_id}, already done: {status}"),
                ));
                continue;
            }
            // GH #589: a registration-time spawn brief that arrives after the
            // addressed worker has already moved its task beyond Open is stale
            // even when the daemon never stamped transport delivery. Do not
            // render old `task start` boilerplate from a direct inbox poll.
            let started_spawn_assignment = if message.source.eq_ignore_ascii_case("director")
                && message
                    .summary
                    .as_deref()
                    .is_some_and(|summary| summary.starts_with("Assigned task:"))
            {
                solicited_task.as_deref().and_then(|task_id| {
                    task_store.as_ref().and_then(|store| {
                        store.get(task_id).ok().and_then(|task| {
                            crate::prompt_revalidation::assignment_targets_started_task(
                                &message.prompt,
                                task.status,
                                task.assignee.as_deref(),
                                &recipient,
                            )
                            .map(|task_id| (task_id, task.status))
                        })
                    })
                })
            } else {
                None
            };
            if let Some((task_id, status)) = started_spawn_assignment {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "inbox_withheld_started_assignment",
                    prompt_id = message.id,
                    recipient = %recipient,
                    task_id = %task_id,
                    status = %status,
                    "cas-589: withheld a delayed spawn assignment after the addressed worker started the task"
                );
                withheld.push((
                    message.id,
                    format!("spawn assignment for {task_id}, already started: {status}"),
                ));
                continue;
            }
            // The transition an assignment solicits: the ADDRESSED recipient
            // started that task. `assignee` must match — another worker
            // starting the task says nothing about whether this recipient ever
            // saw the message.
            let solicited_action_observed = match (&solicited_task, task_store.as_ref()) {
                (Some(task_id), Some(store)) => store
                    .get(task_id)
                    .ok()
                    .map(|task| {
                        task.status != cas_types::TaskStatus::Open
                            && task
                                .assignee
                                .as_deref()
                                .is_some_and(|assignee| assignee.eq_ignore_ascii_case(&recipient))
                    })
                    .unwrap_or(false),
                _ => false,
            };
            match inbox_redelivery_decision(
                message.processed_at.is_some(),
                solicited_action_observed,
            ) {
                InboxRedelivery::WithholdConsumed => {
                    let task_id = solicited_task.unwrap_or_default();
                    tracing::info!(
                        target: "cas::coordination",
                        stage = "inbox_withheld_consumed",
                        prompt_id = message.id,
                        recipient = %recipient,
                        task_id = %task_id,
                        "cas-99d2: withheld an already-delivered assignment whose solicited \
                         task start is already recorded for this recipient"
                    );
                    withheld.push((
                        message.id,
                        format!("assignment for {task_id}, already done"),
                    ));
                    continue;
                }
                InboxRedelivery::MarkRedelivery => {
                    redelivered += 1;
                    body.push_str(&format!(
                        "**[{}] From: {} — {INBOX_REDELIVERY_MARKER} (already delivered {})**\n\
                        Summary: {}\n{}\nMessage: {}\n\n",
                        message.id,
                        message.source,
                        message
                            .processed_at
                            .map(|at| at.to_rfc3339())
                            .unwrap_or_else(|| "earlier".to_string()),
                        message.summary.as_deref().unwrap_or("(no summary)"),
                        queued_message_provenance(message),
                        message.prompt,
                    ));
                }
                InboxRedelivery::FirstDelivery => {
                    body.push_str(&format!(
                        "**[{}] From: {}**\nSummary: {}\n{}\nMessage: {}\n\n",
                        message.id,
                        message.source,
                        message.summary.as_deref().unwrap_or("(no summary)"),
                        queued_message_provenance(message),
                        message.prompt,
                    ));
                }
            }
            rendered += 1;
        }

        if rendered == 0 {
            let ids = withheld
                .iter()
                .map(|(id, reason)| format!("{id} ({reason})"))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(Self::success(format!(
                "No unread messages for {recipient} — withheld {} message(s) already \
                 done: {ids}",
                withheld.len()
            )));
        }

        let mut output = format!(
            "Pulled {rendered} unread message(s) for {recipient} (at-most-once inbox claim: \
             marked seen before this response is delivered; daemon transport delivery is \
             unchanged)"
        );
        if redelivered > 0 {
            output.push_str(&format!(
                ". {redelivered} marked {INBOX_REDELIVERY_MARKER}: already handed to your \
                 transport once — treat as a duplicate unless you never acted on it"
            ));
        }
        if !withheld.is_empty() {
            output.push_str(&format!(
                ". Withheld {} message(s) already done: {}",
                withheld.len(),
                withheld
                    .iter()
                    .map(|(id, reason)| format!("{id} ({reason})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        output.push_str(":\n\n");
        output.push_str(&body);

        Ok(Self::success(output))
    }

    pub(in crate::mcp::tools::service) async fn message_ack(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_prompt_queue_store;

        let notification_id = req.notification_id.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "notification_id required for message_ack (ordinary prompt message ID, or the durable notification ID printed in a lifecycle relay)",
            )
        })?;

        // cas-20ac: lifecycle envelopes print supervisor_queue IDs, not
        // prompt_queue IDs. Resolve that lane first; blindly acking the same
        // integer in prompt_queue can confirm an unrelated row while the
        // lifecycle relay remains pending and is injected again.
        if let Some(linked) = super::supervisor_queue::acknowledge_linked_lifecycle_notification(
            &self.inner.cas_root,
            notification_id,
        )
        .map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to acknowledge linked lifecycle notification: {error}"),
            )
        })? {
            return Ok(Self::success(format!(
                "Lifecycle message {} acknowledged across durable and prompt queues{}; exact-notification redelivery is now terminal.",
                linked.durable_notification_id,
                linked
                    .prompt_id
                    .map(|id| format!(" (linked message_id: {id})"))
                    .unwrap_or_else(|| " (linked prompt already terminal or absent)".to_string())
            )));
        }

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {error}"),
            )
        })?;

        // Historical terminal relays carry only their `lifecycle-wake:<id>`
        // marker; that ID is neither a durable notification nor a prompt row.
        if let Some(prompt_id) = queue.ack_lifecycle_wake(notification_id).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to acknowledge lifecycle relay: {error}"),
            )
        })? {
            return Ok(Self::success(format!(
                "Lifecycle relay {notification_id} acknowledged (prompt message_id: {prompt_id}); it will no longer replay in worker_status."
            )));
        }

        queue.ack(notification_id).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to acknowledge message: {error}"),
            )
        })?;

        // cas-85fd/cas-dcf2: a reply after an inbox drain is intentionally
        // only `assumed_seen`, so it cannot release an urgent-stop halt. An
        // explicit acknowledgement of the *bound* urgent is the authoritative
        // receipt instead. Release no other (including newer) halt.
        use crate::mcp::tools::core::task::lifecycle::stale_close_guard::{
            clear_halt_metadata, halt_prompt_id,
        };
        if let Ok(agent_id) = self.inner.get_agent_id()
            && let Ok(agent_store) = self.inner.open_agent_store()
            && let Ok(mut agent) = agent_store.get(&agent_id)
            && halt_prompt_id(&agent.metadata) == Some(notification_id)
            && matches!(
                queue.message_status(notification_id),
                Ok(Some(cas_store::MessageStatus::Confirmed))
            )
        {
            clear_halt_metadata(&mut agent.metadata);
            if let Err(error) = agent_store.update(&agent) {
                tracing::warn!(
                    agent_id = %agent_id,
                    prompt_id = notification_id,
                    error = %error,
                    "could not release explicitly acknowledged urgent halt"
                );
            }
        }

        // cas-45c4 (GH #102): say what an ack actually proves. It is the
        // caller's claim that it received this message — not evidence Cassy
        // observed the content being surfaced, and not a guarantee for anyone
        // else's copy of a broadcast.
        Ok(Self::success(format!(
            "Message {notification_id} acknowledged by this session (confirmation_source: \
             explicit_ack). This records YOUR claim to have received it; Cassy does not \
             independently observe that the content was surfaced. Use message_status to see \
             transport handoff, wake/reaction observations, and confirmation provenance \
             separately."
        )))
    }

    pub(in crate::mcp::tools::service) async fn message_status_query(
        &self,
        req: AgentRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_prompt_queue_store;

        let notification_id = req.notification_id.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "notification_id required for message_status (the prompt queue message ID)",
            )
        })?;

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {error}"),
            )
        })?;

        // cas-2c5f: stage-based report is additive; legacy status string is
        // preserved on the first line for older clients/scripts.
        let report = queue
            .message_delivery_report(notification_id)
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to query message status: {error}"),
                )
            })?;

        match report {
            Some(mut r) => {
                enrich_report_from_harness_artifact(&self.inner.cas_root, &mut r);
                // cas-893c AC2: `delivered_at` is only transport handoff (the
                // teams-inbox write / PTY inject succeeding). cas-4fb9 adds
                // artifact-backed wake/reaction observations where the
                // recipient harness exposes them; a missing artifact remains
                // `Unobserved`. `confirmed_at` is still the only field that means
                // "the recipient told us it got this" (an explicit
                // `message_ack`), which most recipients never call. So the
                // honest "how long has this been undelivered" clock runs
                // from `enqueued_at` until `confirmed_at`, not until
                // `delivered_at` — a message can sit "delivered" (transport
                // succeeded) but functionally unread for a long time, which
                // is exactly the failure mode this task exists to surface.
                let now = chrono::Utc::now();
                let undelivered_after_secs = if r.confirmed_at.is_none() {
                    Some((now - r.enqueued_at).num_seconds().max(0))
                } else {
                    None
                };

                let mut json_value = serde_json::to_value(&r).unwrap_or_else(|_| {
                    serde_json::json!({
                        "id": r.id,
                        "legacy_status": r.legacy_status,
                        "stage": r.stage,
                    })
                });
                if let Some(obj) = json_value.as_object_mut() {
                    obj.insert(
                        "undelivered_after_secs".to_string(),
                        match undelivered_after_secs {
                            Some(secs) => serde_json::Value::Number(secs.into()),
                            None => serde_json::Value::Null,
                        },
                    );
                }
                let json = serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| {
                    format!(
                        "{{\"id\":{},\"legacy_status\":\"{}\",\"stage\":\"{}\"}}",
                        r.id, r.legacy_status, r.stage
                    )
                });
                let reason = r
                    .pending_reason
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "none".into());
                let undelivered_line = match undelivered_after_secs {
                    Some(secs) => format!(
                        "undelivered_after: {secs}s (not yet confirmed received — \
                         transport handoff succeeding is not the same as the recipient \
                         reading it; escalate if this is climbing and the target is idle)\n"
                    ),
                    // cas-45c4 (GH #102): an ack inferred from a later reply is
                    // evidence the recipient took a turn, NOT that this
                    // message's content was surfaced to it. Reporting both the
                    // same way is what let status claim a confirmation the
                    // recipient never made.
                    None => format!(
                        "undelivered_after: n/a (confirmation_source: {}{})\n",
                        r.confirmation_source,
                        if r.confirmation_source.is_recipient_claim() {
                            " — the recipient's own claim about this message"
                        } else {
                            " — Cassy inferred this from later activity; the recipient never \
                             claimed to have read THIS message"
                        }
                    ),
                };
                // cas-ac7e (GH #130): stage=delivered used to be the writer's
                // unchecked claim about itself. Say out loud when the
                // recipient side cannot corroborate it, instead of reporting
                // "delivered" and leaving the operator to discover from the
                // recipient that nothing arrived.
                let transport_line = recipient_transport_warning(
                    r.stage,
                    &r.target,
                    r.recipient_transport_at.is_some(),
                )
                .unwrap_or("");
                // cas-7a01 (GH #155): `wake: unobserved` used to be a
                // hardcoded constant with no backing column — three incidents
                // read it and learned nothing, because it could not tell "Cassy
                // never nudged this recipient" from "Cassy nudged it and the
                // harness started a turn without surfacing the message".
                // `wake_attempt` is the daemon's own record of which of those
                // happened; `wake` remains recipient-side evidence.
                let wake_attempt_line = wake_attempt_narrative(r.wake_attempt, r.wake);
                Ok(Self::success(format!(
                    "Message {notification_id} status: {}\n\
                     stage: {}  pending_reason: {}  wake: {}  wake_attempt: {}  wake_gate_declines: {}  \
                     reaction: {}  confirmation_source: {}\n\
                     {wake_attempt_line}\
                     {transport_line}\
                     {undelivered_line}\
                     {json}",
                    r.legacy_status,
                    r.stage,
                    reason,
                    r.wake,
                    r.wake_attempt,
                    r.wake_gate_declines,
                    r.reaction,
                    r.confirmation_source
                )))
            }
            None => Ok(Self::success(format!(
                "Message {notification_id} not found"
            ))),
        }
    }
}

/// Enrich the store's deliberately conservative report with real harness
/// records. Failure at any lookup/parse step leaves both stages unobserved.
///
/// The delivery timestamp is only an ordering floor: it never becomes an
/// observation itself. A non-Unobserved value requires a concrete record in
/// the target worker's resolved artifact, and the record timestamp + path are
/// attached to the report as provenance.
fn enrich_report_from_harness_artifact(
    cas_root: &std::path::Path,
    report: &mut cas_store::MessageDeliveryReport,
) {
    use cas_store::ObservationStatus;

    let Some(delivered_at) = report.delivered_at else {
        return;
    };
    let Ok(store) = crate::store::open_agent_store(cas_root) else {
        return;
    };
    let Ok(agents) = store.list(None) else {
        return;
    };
    let Some(agent) = agents.into_iter().find(|agent| {
        (agent.name == report.target || agent.id == report.target)
            && report
                .factory_session
                .as_ref()
                .is_none_or(|session| agent.factory_session.as_ref() == Some(session))
    }) else {
        return;
    };
    let cli = crate::mcp::tools::service::factory_ops::worker_cli_from_agent(&agent);
    if cli == cas_mux::SupervisorCli::OpenCode {
        let session_id = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let Some(observation) = crate::mcp::tools::service::opencode_liveness::observe(
            cas_root,
            session_id,
            now_ms,
            crate::mcp::tools::service::agent_liveness::agent_process_is_alive(&agent),
        ) else {
            return;
        };
        let Some(mapped_session) =
            crate::mcp::tools::service::opencode_liveness::mapped_session_id(&observation)
        else {
            return;
        };
        let evidence_prefix = format!("OpenCode mapped session {mapped_session}");
        if let Some(at) = observation
            .state
            .last_activity_at
            .and_then(|at| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at as i64))
            .filter(|at| *at >= delivered_at)
        {
            report.wake = ObservationStatus::Observed;
            report.wake_observed_at = Some(at);
            report.wake_evidence = Some(format!(
                "{evidence_prefix} plugin activity signal at {}",
                at.to_rfc3339()
            ));
        }
        if let Some(at) = observation
            .state
            .last_tool
            .as_ref()
            .and_then(|tool| tool.completed_at)
            .and_then(|at| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at as i64))
            .filter(|at| *at >= delivered_at)
        {
            report.reaction = ObservationStatus::Observed;
            report.reaction_observed_at = Some(at);
            report.reaction_evidence = Some(format!(
                "{evidence_prefix} completed tool attribution at {}",
                at.to_rfc3339()
            ));
        }
        return;
    }
    let Some(path) =
        crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(cas_root, &agent)
    else {
        return;
    };
    let observations = crate::mcp::tools::service::harness_observation::observations_after_delivery(
        &path,
        cli,
        delivered_at,
        &report.prompt,
    );
    if let Some(wake) = observations.wake {
        report.wake = ObservationStatus::Observed;
        report.wake_observed_at = Some(wake.at);
        report.wake_evidence = Some(wake.evidence);
    }
    if let Some(reaction) = observations.reaction {
        report.reaction = ObservationStatus::Observed;
        report.reaction_observed_at = Some(reaction.at);
        report.reaction_evidence = Some(reaction.evidence);
    }
}

/// Which `factory_session` a queued row must carry (cas-15f2).
///
/// The row is stamped with the **recipient's** session, never the sender's.
/// Every downstream filter — the daemon's `peek_for_targets`, `inbox_poll`'s
/// `drain_unseen_for_recipient`, turn-start surfacing, and the `worker_status`
/// unseen counter — selects on `factory_session = <observer's own session>`.
/// Stamping at enqueue with the target's session is therefore the single change
/// that makes all four see the row, and it keeps every cross-session isolation
/// test true: the row genuinely belongs to the recipient's session, so nothing
/// leaks into a session that was not addressed.
///
/// Before this, `message_send` stamped the sender's `CAS_FACTORY_SESSION` while
/// validating the target against a session-blind name lookup, so a supervisor
/// addressing a supervisor in another session got an optimistic "enqueued
/// (target is registered)" for a row no daemon could ever select: the sender's
/// daemon matched the session but not its roster, the recipient's daemon
/// matched its roster but not the session. The row sat at
/// `stage=enqueued / awaiting_delivery` until the sender-side 15-minute poison
/// sweep abandoned it as `abandoned_unknown_target`.
///
/// Logical fan-out names stay sender-scoped: `all_workers` is a broadcast to
/// *this* factory, and `supervisor` / `director` resolve within the caller's own
/// session. Only a concrete registered agent redirects the stamp.
fn row_factory_session(
    resolved_target: &str,
    target_agent: Option<&cas_types::Agent>,
    sender_session: Option<&str>,
) -> Option<String> {
    if matches!(
        resolved_target.to_ascii_lowercase().as_str(),
        "all_workers" | "supervisor" | "director"
    ) {
        return sender_session.map(str::to_owned);
    }
    target_agent
        .and_then(|agent| agent.factory_session.clone())
        .filter(|session| !session.trim().is_empty())
        .or_else(|| sender_session.map(str::to_owned))
}

#[cfg(test)]
mod cross_session_routing_tests {
    use super::row_factory_session;
    use cas_types::Agent;

    fn agent_in(name: &str, session: Option<&str>) -> Agent {
        let mut agent = Agent::new(format!("id-{name}"), name.to_string());
        agent.factory_session = session.map(str::to_owned);
        agent
    }

    /// The cas-15f2 regression: supervisor A in session A messages supervisor B
    /// in session B. The row must belong to B, or no daemon ever selects it.
    #[test]
    fn a_message_to_an_agent_in_another_session_is_stamped_with_the_recipients_session() {
        let target = agent_in("noble-lynx-44", Some("cas-src-vivid-sparrow-8"));

        let session = row_factory_session(
            "noble-lynx-44",
            Some(&target),
            Some("cas-src-young-raven-93"),
        );

        assert_eq!(session.as_deref(), Some("cas-src-vivid-sparrow-8"));
    }

    #[test]
    fn a_same_session_message_is_unchanged() {
        let target = agent_in("daring-marten-11", Some("cas-src-young-raven-93"));

        let session = row_factory_session(
            "daring-marten-11",
            Some(&target),
            Some("cas-src-young-raven-93"),
        );

        assert_eq!(session.as_deref(), Some("cas-src-young-raven-93"));
    }

    /// `all_workers` means "this factory's workers". Redirecting it to some
    /// other session's roster would be a broadcast into a factory the caller
    /// does not run.
    #[test]
    fn logical_fanout_names_stay_scoped_to_the_sender() {
        for name in ["all_workers", "supervisor", "director", "ALL_WORKERS"] {
            let stray = agent_in(name, Some("cas-src-other-session"));
            let session = row_factory_session(name, Some(&stray), Some("cas-src-mine"));
            assert_eq!(
                session.as_deref(),
                Some("cas-src-mine"),
                "{name} must not redirect the stamp"
            );
        }
    }

    #[test]
    fn an_unregistered_or_sessionless_target_keeps_the_senders_session() {
        assert_eq!(
            row_factory_session("ghost", None, Some("cas-src-mine")).as_deref(),
            Some("cas-src-mine")
        );
        let legacy = agent_in("legacy", None);
        assert_eq!(
            row_factory_session("legacy", Some(&legacy), Some("cas-src-mine")).as_deref(),
            Some("cas-src-mine")
        );
        let blank = agent_in("blank", Some("   "));
        assert_eq!(
            row_factory_session("blank", Some(&blank), Some("cas-src-mine")).as_deref(),
            Some("cas-src-mine")
        );
    }

    #[test]
    fn a_sessionless_sender_still_routes_to_the_recipients_session() {
        let target = agent_in("noble-lynx-44", Some("cas-src-vivid-sparrow-8"));
        assert_eq!(
            row_factory_session("noble-lynx-44", Some(&target), None).as_deref(),
            Some("cas-src-vivid-sparrow-8")
        );
    }
}

#[cfg(test)]
mod inbox_poll_identity_tests {
    use super::{
        enrich_report_from_harness_artifact, interrupt_unconfirmed_message,
        lifecycle_relay_is_stale_at_inbox_pop, recipient_transport_warning,
        resolve_inbox_recipient,
    };
    use cas_store::DeliveryStage;

    #[test]
    fn claude_interrupt_timeout_is_an_explicit_failure_surface() {
        let message = interrupt_unconfirmed_message(269, None);
        assert!(message.contains("Could not confirm Claude interrupt delivery"));
        assert!(message.contains("notification 269"));
        assert!(message.contains("did not silently claim"));
        assert!(message.contains("message_status notification_id=269"));
    }

    /// cas-7a01 (GH #155): the combination that used to be completely
    /// invisible — Cassy nudged the pane and no turn ever carried the message —
    /// must be named, not left for an operator to infer from two fields.
    #[test]
    fn a_fired_nudge_with_no_surfacing_is_called_out() {
        let line = super::wake_attempt_narrative(
            cas_store::WakeAttempt::Fired,
            cas_store::ObservationStatus::Unobserved,
        );
        assert!(line.contains("DID nudge"), "{line}");
        assert!(line.contains("#155"), "{line}");
    }

    /// The three wake-attempt states must read differently. If two of them
    /// render the same sentence, the split this task exists to create is
    /// cosmetic.
    #[test]
    fn every_wake_attempt_state_reads_differently() {
        use cas_store::{ObservationStatus, WakeAttempt};
        let lines: Vec<String> = [
            WakeAttempt::Fired,
            WakeAttempt::Failed,
            WakeAttempt::NotAttempted,
        ]
        .into_iter()
        .map(|a| super::wake_attempt_narrative(a, ObservationStatus::Unobserved))
        .collect();
        let mut unique = lines.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            3,
            "wake states collapse to the same text: {lines:?}"
        );
    }

    /// A confirmed surfacing outranks whatever the nudge did: the message
    /// demonstrably reached a turn.
    #[test]
    fn an_observed_wake_reports_the_surfacing_regardless_of_the_nudge() {
        use cas_store::{ObservationStatus, WakeAttempt};
        for attempt in [
            WakeAttempt::Fired,
            WakeAttempt::Failed,
            WakeAttempt::NotAttempted,
        ] {
            let line = super::wake_attempt_narrative(attempt, ObservationStatus::Observed);
            assert!(line.contains("injected into a turn"), "{attempt}: {line}");
        }
    }

    /// cas-ac7e (GH #130): the 7183 shape — delivered per the writer, with
    /// nothing on the recipient's side to corroborate it — must be called out.
    #[test]
    fn a_delivered_row_without_a_recipient_stamp_warns() {
        let warning = recipient_transport_warning(DeliveryStage::Delivered, "fast-cobra-90", false);
        assert!(
            warning.is_some_and(|w| w.contains("MISSING")),
            "silently reporting stage=delivered here is exactly the blind spot #130 reported"
        );
    }

    #[test]
    fn a_corroborated_delivery_does_not_warn() {
        assert_eq!(
            recipient_transport_warning(DeliveryStage::Delivered, "fast-cobra-90", true),
            None
        );
    }

    #[test]
    fn a_broadcast_is_exempt_from_the_recipient_stamp_warning() {
        assert_eq!(
            recipient_transport_warning(DeliveryStage::Delivered, "all_workers", false),
            None,
            "a broadcast's per-recipient transport is its broadcast counts; warning here              would fire on every healthy broadcast and train operators to ignore it"
        );
    }

    #[test]
    fn a_row_that_never_reached_delivered_does_not_warn() {
        for stage in [
            DeliveryStage::Enqueued,
            DeliveryStage::Selected,
            DeliveryStage::Gated,
            DeliveryStage::Confirmed,
        ] {
            assert_eq!(
                recipient_transport_warning(stage, "worker-1", false),
                None,
                "{stage} has made no delivery claim to contradict"
            );
        }
    }

    #[test]
    fn registered_identity_precedes_environment_fallback() {
        assert_eq!(
            resolve_inbox_recipient(
                Some("registered-worker".to_string()),
                Some("env-worker".to_string()),
            ),
            Some("registered-worker".to_string())
        );
    }

    #[test]
    fn environment_identity_is_used_when_registration_is_unavailable() {
        assert_eq!(
            resolve_inbox_recipient(None, Some("env-worker".to_string())),
            Some("env-worker".to_string())
        );
        assert_eq!(resolve_inbox_recipient(None, Some("  ".to_string())), None);
    }

    #[test]
    fn unreadable_task_never_withholds_a_lifecycle_relay_at_inbox_pop() {
        let prompt = "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-live\" old=\"in_progress\" new=\"awaiting_merge\" actor=\"worker\" notification_id=\"1\" occurrence=\"2026-08-14T14:00:00Z\">\nMERGE REQUIRED\n</task-lifecycle>";
        assert!(
            !lifecycle_relay_is_stale_at_inbox_pop(prompt, None),
            "a read failure is uncertainty, not evidence that a live relay is moot"
        );
    }

    #[test]
    fn message_report_is_enriched_only_by_target_codex_rollout_records() {
        use cas_store::ObservationStatus;

        let _lock = crate::hooks::test_env_lock();
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join("project");
        std::fs::create_dir_all(&cas_root).unwrap();
        let codex_home = temp.path().join("codex-home");
        let sessions = codex_home.join("sessions/2099/01/01");
        std::fs::create_dir_all(&sessions).unwrap();
        // Exercise the live legacy-row shape: factory registration has no
        // clone_path metadata, so status resolves the convention worktree.
        let clone_path = cas_root.join("worktrees/worker-a");
        std::fs::create_dir_all(&clone_path).unwrap();
        let rollout = sessions.join("rollout-live-worker-session.jsonl");
        std::fs::write(
            &rollout,
            format!(
                concat!(
                    "{{\"timestamp\":\"2099-01-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"cwd\":{:?},\"originator\":\"codex-tui\",\"source\":\"cli\"}}}}\n",
                    "{{\"timestamp\":\"2099-01-01T00:00:02Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\",\"turn_id\":\"observed-turn\"}}}}\n",
                    "{{\"timestamp\":\"2099-01-01T00:00:02.500Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"Message from supervisor: act\"}}],\"internal_chat_message_metadata_passthrough\":{{\"turn_id\":\"observed-turn\"}}}}}}\n",
                    "{{\"timestamp\":\"2099-01-01T00:00:03Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"internal_chat_message_metadata_passthrough\":{{\"turn_id\":\"observed-turn\"}}}}}}\n"
                ),
                clone_path.display().to_string()
            ),
        )
        .unwrap();

        let old_codex_home = std::env::var("CODEX_HOME").ok();
        unsafe { std::env::set_var("CODEX_HOME", &codex_home) };

        let agent_store = crate::store::open_agent_store(&cas_root).unwrap();
        let mut agent =
            cas_types::Agent::new("live-worker-session".to_string(), "worker-a".to_string());
        agent.role = cas_types::AgentRole::Worker;
        agent.factory_session = Some("factory-1".to_string());
        agent
            .metadata
            .insert("worker_cli".to_string(), "codex".to_string());
        agent_store.register(&agent).unwrap();

        let queue = crate::store::open_prompt_queue_store(&cas_root).unwrap();
        let message_id = queue
            .enqueue_with_session("supervisor", "worker-a", "act", "factory-1")
            .unwrap();
        queue.mark_transport_delivered(message_id).unwrap();
        let mut report = queue.message_delivery_report(message_id).unwrap().unwrap();
        assert_eq!(report.wake, ObservationStatus::Unobserved);
        assert!(
            serde_json::to_value(&report)
                .unwrap()
                .get("prompt")
                .is_none()
        );

        enrich_report_from_harness_artifact(&cas_root, &mut report);

        unsafe {
            match old_codex_home {
                Some(value) => std::env::set_var("CODEX_HOME", value),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
        assert_eq!(report.wake, ObservationStatus::Observed);
        assert_eq!(report.reaction, ObservationStatus::Observed);
        assert!(report.wake_evidence.as_deref().is_some_and(|evidence| {
            evidence.contains("task_started") && evidence.contains("rollout-live-worker")
        }));
    }

    #[test]
    fn message_report_uses_opencode_mapping_for_wake_and_reaction() {
        use cas_mux::{
            OpenCodeSessionEvent, OpenCodeSessionEventKind, OpenCodeSessionState,
            persist_opencode_session_state,
        };
        use cas_store::ObservationStatus;

        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join("project");
        std::fs::create_dir_all(&cas_root).unwrap();
        let clone_path = cas_root.join("worktrees/opencode-worker");
        std::fs::create_dir_all(&clone_path).unwrap();
        let session_id = "opencode-opencode-worker";
        let event_at = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let mut state = OpenCodeSessionState::new(session_id, clone_path.display().to_string());
        state.apply(OpenCodeSessionEvent {
            at: event_at,
            kind: OpenCodeSessionEventKind::RootCreated {
                session_id: "ses_mapped-worker".to_string(),
                directory: clone_path.display().to_string(),
            },
        });
        state.apply(OpenCodeSessionEvent {
            at: event_at + 1,
            kind: OpenCodeSessionEventKind::ToolBefore {
                session_id: "ses_mapped-worker".to_string(),
                name: "bash".to_string(),
                call_id: Some("call-1".to_string()),
            },
        });
        state.apply(OpenCodeSessionEvent {
            at: event_at + 2,
            kind: OpenCodeSessionEventKind::ToolAfter {
                session_id: "ses_mapped-worker".to_string(),
                call_id: Some("call-1".to_string()),
                success: true,
            },
        });
        persist_opencode_session_state(
            &cas_mux::opencode_session_state_path(&cas_root, session_id),
            &state,
        )
        .unwrap();

        let agent_store = crate::store::open_agent_store(&cas_root).unwrap();
        let mut agent =
            cas_types::Agent::new(session_id.to_string(), "opencode-worker".to_string());
        agent.role = cas_types::AgentRole::Worker;
        agent.factory_session = Some("factory-1".to_string());
        agent
            .metadata
            .insert("worker_cli".to_string(), "opencode".to_string());
        agent_store.register(&agent).unwrap();

        let queue = crate::store::open_prompt_queue_store(&cas_root).unwrap();
        let message_id = queue
            .enqueue_with_session("supervisor", "opencode-worker", "act", "factory-1")
            .unwrap();
        queue.mark_transport_delivered(message_id).unwrap();
        let mut report = queue.message_delivery_report(message_id).unwrap().unwrap();
        report.delivered_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            event_at.saturating_sub(1_000) as i64,
        );

        enrich_report_from_harness_artifact(&cas_root, &mut report);

        assert_eq!(report.wake, ObservationStatus::Observed);
        assert_eq!(report.reaction, ObservationStatus::Observed);
        assert!(
            report
                .wake_evidence
                .as_deref()
                .is_some_and(|evidence| evidence.contains("ses_mapped-worker"))
        );
        assert!(
            report
                .reaction_evidence
                .as_deref()
                .is_some_and(|evidence| evidence.contains("completed tool attribution"))
        );
        assert_eq!(report.wake_observed_at, report.reaction_observed_at);
    }
}

#[cfg(test)]
mod cas99d2_redelivery_tests {
    use super::{INBOX_REDELIVERY_MARKER, InboxRedelivery, inbox_redelivery_decision};
    use crate::prompt_revalidation::assignment_solicited_task_id;

    /// The literal text of notification 7112 (supervisor hand-written dispatch).
    #[test]
    fn the_real_7112_assignment_names_its_solicited_task() {
        let prompt = "You are assigned task cas-7587 (P2 bug, epic cas-b0c7). Run \
                      `mcp__cas__task action=show id=cas-7587` then \
                      `mcp__cas__task action=start id=cas-7587`.\n\nScope: GH #122 …";
        assert_eq!(
            assignment_solicited_task_id(prompt).as_deref(),
            Some("cas-7587")
        );
    }

    /// The director's generated assignment prompt shape.
    #[test]
    fn the_generated_assignment_prompt_names_its_solicited_task() {
        let prompt = "You have been assigned a new task:\n\
                      Task ID: cas-99d2\n\
                      Title: Message confirmation truth\n\n\
                      Start working: mcp__cas__task action=start id=cas-99d2";
        assert_eq!(
            assignment_solicited_task_id(prompt).as_deref(),
            Some("cas-99d2")
        );
    }

    /// A message that merely mentions a task id is not an assignment. Treating
    /// it as one would let unrelated task progress silence real instructions.
    #[test]
    fn non_assignment_messages_solicit_nothing() {
        for prompt in [
            "factory/fierce-crow-25 is merged into the epic branch. Re-run close for cas-bcfb.",
            "status on cas-7587? your last note was 20 minutes ago",
            "Reviewed the diff for cas-7587 myself — nice work.",
            "stand down and shut down cleanly",
        ] {
            assert_eq!(
                assignment_solicited_task_id(prompt),
                None,
                "must not be read as an assignment: {prompt}"
            );
        }
    }

    /// An assignment phrase with no task-shaped id yields nothing rather than
    /// a bogus id that would be looked up and (not) found.
    #[test]
    fn an_assignment_without_a_task_id_solicits_nothing() {
        assert_eq!(
            assignment_solicited_task_id("You have been assigned a new task, details to follow"),
            None
        );
    }

    #[test]
    fn redelivery_decision_covers_the_three_cases() {
        assert_eq!(
            inbox_redelivery_decision(false, false),
            InboxRedelivery::FirstDelivery
        );
        assert_eq!(
            inbox_redelivery_decision(true, false),
            InboxRedelivery::MarkRedelivery,
            "a repeat with no consumption evidence is still delivered, but marked"
        );
        assert_eq!(
            inbox_redelivery_decision(true, true),
            InboxRedelivery::WithholdConsumed,
            "GH #127: delivered once AND the solicited action happened"
        );
        assert_eq!(
            inbox_redelivery_decision(false, true),
            InboxRedelivery::FirstDelivery,
            "a row never handed to a transport must be delivered even if the task \
             happens to have moved — the recipient has provably not seen this text"
        );
    }

    /// The marker must match the token the daemon's teams-inbox writer already
    /// recognises as an intentional redelivery, so the two channels agree.
    #[test]
    fn the_marker_matches_the_teams_inbox_redelivery_token() {
        assert_eq!(INBOX_REDELIVERY_MARKER, "[redelivery]");
    }
}

/// Regression coverage for GH #328. A task can remain AwaitingMerge briefly
/// after its branch lands, precisely when a worker may need supervisor help to
/// escape a rejected close gate. Only a typed merge request may be withheld in
/// that state; the same worker's ordinary escalation must still be queued.
#[cfg(test)]
mod cas_89e1_post_merge_message_type_tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use cas_types::{Agent, AgentRole, Task, TaskStatus, WorkTarget};
    use rmcp::model::RawContent;
    use std::path::Path;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn response_text(result: CallToolResult) -> String {
        result
            .content
            .into_iter()
            .filter_map(|content| match content.raw {
                RawContent::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn message_request(merge_request: bool) -> AgentRequest {
        serde_json::from_value(serde_json::json!({
            "action": "message",
            "target": "supervisor",
            "task_id": "cas-89e1",
            "merge_request": merge_request,
            "summary": if merge_request { "ready to merge" } else { "close gate rejected" },
            "message": if merge_request {
                "Fresh worker delivery; please merge the branch."
            } else {
                "My close was rejected after the merge landed; please help unblock the gate."
            },
        }))
        .expect("static message request")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_merge_suppression_requires_the_explicit_merge_request_type() {
        let mut env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().expect("host repo schema");
        let host_collision = tempfile::tempdir().expect("host collision repo");
        let collision_repo = host_collision.path();
        git(collision_repo, &["init", "-q", "-b", "main"]);
        std::fs::create_dir(collision_repo.join(".cas")).expect("collision Cassy directory");
        std::fs::write(
            collision_repo.join(".cas/config.toml"),
            "[project]\ncanonical_id = \"cas-89e1-message-test\"\n",
        )
        .expect("collision Cassy config");
        crate::store::known_repos::register_repo_strict(collision_repo)
            .expect("register host collision");

        let project = tempfile::tempdir().expect("temporary project");
        let repo = project.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "cas-test@example.invalid"]);
        git(repo, &["config", "user.name", "Cassy Test"]);
        std::fs::create_dir(repo.join(".cas")).expect("Cassy directory");
        std::fs::write(
            repo.join(".cas/config.toml"),
            "[project]\ncanonical_id = \"cas-89e1-message-test\"\n",
        )
        .expect("Cassy config");
        std::fs::write(repo.join("base.txt"), "base\n").expect("base file");
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base"]);
        git(repo, &["checkout", "-qb", "factory/worker-a"]);
        std::fs::write(repo.join("delivery.txt"), "delivery\n").expect("delivery file");
        git(repo, &["add", "delivery.txt"]);
        git(repo, &["commit", "-qm", "delivery"]);
        git(repo, &["checkout", "-q", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker-a", "-m", "merge"],
        );

        let cas_root = repo.join(".cas");
        let core = crate::mcp::server::CasCore::with_daemon(cas_root.clone(), None, None);
        let agents = core.open_agent_store().expect("agent store");
        let mut worker = Agent::new("worker-id".to_string(), "worker-a".to_string());
        worker.role = AgentRole::Worker;
        agents.register(&worker).expect("register worker");
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        agents.register(&supervisor).expect("register supervisor");
        core.set_agent_id_for_testing(worker.id.clone());

        let tasks = core.open_task_store().expect("task store");
        let mut task = Task::new(
            "cas-89e1".to_string(),
            "post-merge message type".to_string(),
        );
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some(worker.name.clone());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:cas-89e1-message-test".to_string(),
            target_branch: "main".to_string(),
        });
        task.deliverables.parked_branch = Some("factory/worker-a".to_string());
        tasks.add(&task).expect("add parked task");

        // The fixture starts hermetic even when the test binary was launched
        // by a supervisor. Re-introduce a conflicting ambient role only after
        // registration to prove the persisted worker role wins at the MCP
        // message boundary.
        assert!(std::env::var_os("CAS_AGENT_ROLE").is_none());
        env.set("CAS_AGENT_ROLE", "supervisor");

        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core.clone(), None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core.clone());

        let escalation = response_text(
            service
                .message_send(message_request(false))
                .await
                .expect("ordinary escalation is delivered"),
        );
        assert!(escalation.contains("Message queued"), "{escalation}");
        assert!(
            !escalation.contains("Merge already landed"),
            "an untyped escalation must not be suppressed: {escalation}"
        );
        let queued = crate::store::open_prompt_queue_store(&cas_root)
            .expect("prompt queue")
            .poll_all(10)
            .expect("queued escalation");
        assert_eq!(queued.len(), 1, "ordinary escalation must reach the queue");
        assert!(
            crate::prompt_revalidation::parse_merge_request_envelope(&queued[0].prompt).is_none(),
            "ordinary escalation must not acquire a merge envelope: {:?}",
            queued[0].prompt
        );

        let stale_merge = response_text(
            service
                .message_send(message_request(true))
                .await
                .expect("stale merge request receives guidance"),
        );
        assert!(
            stale_merge.contains("Merge already landed"),
            "an explicitly typed stale merge request must still be suppressed: {stale_merge}"
        );
    }
}
