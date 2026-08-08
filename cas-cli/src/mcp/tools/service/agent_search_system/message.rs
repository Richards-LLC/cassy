use crate::mcp::tools::service::imports::*;

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

/// cas-99d2 (GH #127): the task id an assignment message solicits a `start`
/// for, if this message is an assignment at all.
///
/// Two shapes exist in the wild and both must parse:
/// - the director's generated prompt — `"You have been assigned a new task:\n
///   Task ID: cas-7587\n…"`;
/// - a supervisor's hand-written dispatch — `"You are assigned task cas-7587
///   (P2 bug, …). Run `… action=show id=cas-7587` then `… action=start
///   id=cas-7587`."` (this is the literal shape of notification 7112).
///
/// Requires BOTH an assignment phrase and an explicit task id: a status
/// question or review note that merely mentions a task id must not be treated
/// as an assignment, because that would let unrelated task progress silence a
/// message that was never about assigning work.
pub(crate) fn assignment_solicited_task_id(prompt: &str) -> Option<String> {
    let lowered = prompt.to_lowercase();
    const ASSIGNMENT_PHRASES: [&str; 4] = [
        "you have been assigned",
        "you are assigned",
        "you're assigned",
        "assigned task",
    ];
    if !ASSIGNMENT_PHRASES
        .iter()
        .any(|phrase| lowered.contains(phrase))
    {
        return None;
    }
    // `action=start id=<task>` is the most explicit statement of the solicited
    // transition; fall back to the declared `Task ID:` field, then to the first
    // task-shaped token anywhere in the assignment text.
    for marker in ["action=start id=", "task id:", "assigned task "] {
        if let Some(index) = lowered.find(marker)
            && let Some(id) = first_task_id_token(&prompt[index + marker.len()..])
        {
            return Some(id);
        }
    }
    first_task_id_token(prompt)
}

/// First `cas-<hex>` token in `text`, stripped of surrounding punctuation.
fn first_task_id_token(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find(|token| {
            let Some(suffix) = token
                .strip_prefix("cas-")
                .or_else(|| token.strip_prefix("CAS-"))
            else {
                return false;
            };
            suffix.len() >= 4 && suffix.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .map(|token| format!("cas-{}", &token[4..]))
}

/// cas-7a01 (GH #155): render the wake evidence pair as a sentence.
///
/// The two fields answer different questions and the failure the issue reported
/// lives precisely in their disagreement, so an operator must not have to
/// derive it. `wake_attempt` is what CAS did; `wake` is whether a turn
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
            "wake evidence: CAS DID nudge this recipient's pane, and no turn is recorded as \
             having carried this message — the nudge landed but the harness surfaced nothing \
             (GH #155 signature)."
        }
        (WakeAttempt::Failed, ObservationStatus::Unobserved) => {
            "wake evidence: CAS ATTEMPTED a wake and it FAILED (see wake_attempt_detail); the \
             recipient was never nudged for this message."
        }
        (WakeAttempt::NotAttempted, ObservationStatus::Unobserved) => {
            "wake evidence: CAS never attempted a wake for this message — the idle gate \
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
         per-recipient transport stamp. Either it was delivered before CAS \
         recorded them, or the stamp and the stage have diverged; treat the \
         delivery as unproven and re-send.\n",
    )
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
        let role = std::env::var("CAS_AGENT_ROLE")
            .ok()
            .or_else(|| agent_from_store.as_ref().map(|a| a.role.to_string()))
            .unwrap_or_else(|| "primary".to_string());

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
                if supervisor_name.as_deref() != Some(&target) {
                    return Err(Self::error(
                        ErrorCode::INVALID_REQUEST,
                        format!(
                            "Workers can only message their supervisor. Use target='supervisor' or '{}'",
                            supervisor_name.unwrap_or_else(|| "<supervisor>".to_string())
                        ),
                    ));
                }
                target
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

        // cas-bc8c: merge requests used to be indistinguishable from ordinary
        // prose, so a request queued just after the supervisor merged remained
        // actionable-looking when it eventually arrived. Give the parked task
        // identity to the send path explicitly when supplied, or infer it only
        // when this worker has exactly one AwaitingMerge task. CAS then derives
        // both immutable tips itself; callers cannot forget or stale them.
        // Untagged messages with zero/ambiguous parked tasks remain completely
        // unchanged on this shared public surface.
        if role == "worker" {
            use crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha;
            use crate::mcp::tools::core::task::repo_context::resolve_repo_context;
            use crate::prompt_revalidation::{
                MergeRequestDecision, MergeRequestEnvelope, attach_merge_request_envelope,
                merge_landed_guidance, revalidate_merge_request, select_unambiguous_merge_task,
            };
            use crate::store::open_task_store_local;
            use cas_types::TaskStatus;

            let merge_task = open_task_store_local(&self.inner.cas_root).ok().and_then(|store| {
                let parked = store.list(Some(TaskStatus::AwaitingMerge)).ok()?;
                select_unambiguous_merge_task(
                    &parked,
                    &display_name,
                    req.task_id.as_deref(),
                )
                .cloned()
            });

            if let Some(task) = merge_task
                && let Some(work_target) = task.deliverables.work_target.as_ref()
                && let Ok(repo) = resolve_repo_context(&self.inner.cas_root, work_target)
            {
                let branch = task
                    .deliverables
                    .parked_branch
                    .clone()
                    .or_else(|| task.assignee.as_ref().map(|name| format!("factory/{name}")));
                if let Some(branch) = branch
                    && let Some(branch_tip) = resolve_branch_sha(&repo.repo_root, &branch)
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
        let target_is_registered = resolved_target == "all_workers"
            || resolved_target == "supervisor"
            || resolved_target.eq_ignore_ascii_case("director")
            || {
                use crate::store::open_agent_store;
                open_agent_store(&self.inner.cas_root)
                    .ok()
                    .and_then(|store| store.list(None).ok())
                    .map(|agents| {
                        agents
                            .iter()
                            .any(|a| a.name.eq_ignore_ascii_case(&resolved_target))
                    })
                    .unwrap_or(false)
            };

        let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
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

        // cas-b269 review 2: halt fan-out is session-scoped, authorized by
        // AgentRole::Supervisor|Director (and display fallback), fail-closed
        // on store errors, generation-stamped, and all-or-none with enqueue
        // (compensate halt writes if enqueue fails).
        let mut halt_compensation: Vec<(String, std::collections::HashMap<String, String>)> =
            Vec::new();
        {
            use crate::mcp::tools::core::task::lifecycle::stale_close_guard::{
                HaltWorkerCandidate, apply_halt_metadata, halt_targets_for_urgent,
                is_merge_reclose_exempt_urgent, may_source_role_set_halt, may_source_set_halt,
                session_scoped_worker_names, should_persist_urgent_halt,
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
                    let halt_generation = chrono::Utc::now().timestamp_millis().max(0) as u64;
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
        let enqueue_outcome = match queue.enqueue_urgent_with_outcome(
            &display_name,
            &resolved_target,
            &message,
            factory_session.as_deref(),
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
        let message_id = enqueue_outcome.id();
        let duplicate_suppressed = matches!(
            enqueue_outcome,
            cas_store::EnqueueOutcome::SuppressedDuplicate(_)
        );

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
        let resolved_target_role = {
            use crate::store::open_agent_store;
            open_agent_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.list(None).ok())
                .and_then(|agents| {
                    agents
                        .into_iter()
                        .find(|agent| agent.name.eq_ignore_ascii_case(&resolved_target))
                        .map(|agent| agent.role)
                })
        };
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
            "{} queued\n\nnotification_id: {}\nFrom: {} ({})\nTo: {}\n{}Message: {}",
            if urgent { "URGENT message" } else { "Message" },
            message_id,
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
            return Ok(Self::success(format!(
                "No unread messages for {recipient}"
            )));
        }

        // cas-99d2 (GH #127): a row the daemon already handed to this
        // recipient's transport is not new mail, and the poll used to render it
        // byte-identically with no way to tell. Classify each row before
        // rendering: withhold one whose solicited transition already happened,
        // and mark the rest as repeats.
        let task_store = crate::store::open_task_store_local(&self.inner.cas_root).ok();
        let mut rendered = 0usize;
        let mut withheld: Vec<(i64, String)> = Vec::new();
        let mut redelivered = 0usize;
        let mut body = String::new();
        for message in &messages {
            let solicited_task = assignment_solicited_task_id(&message.prompt);
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
                    withheld.push((message.id, task_id));
                    continue;
                }
                InboxRedelivery::MarkRedelivery => {
                    redelivered += 1;
                    body.push_str(&format!(
                        "**[{}] From: {} — {INBOX_REDELIVERY_MARKER} (already delivered {})**\n\
                         Summary: {}\nCreated: {}\nMessage: {}\n\n",
                        message.id,
                        message.source,
                        message
                            .processed_at
                            .map(|at| at.to_rfc3339())
                            .unwrap_or_else(|| "earlier".to_string()),
                        message.summary.as_deref().unwrap_or("(no summary)"),
                        message.created_at.to_rfc3339(),
                        message.prompt,
                    ));
                }
                InboxRedelivery::FirstDelivery => {
                    body.push_str(&format!(
                        "**[{}] From: {}**\nSummary: {}\nCreated: {}\nMessage: {}\n\n",
                        message.id,
                        message.source,
                        message.summary.as_deref().unwrap_or("(no summary)"),
                        message.created_at.to_rfc3339(),
                        message.prompt,
                    ));
                }
            }
            rendered += 1;
        }

        if rendered == 0 {
            let ids = withheld
                .iter()
                .map(|(id, task)| format!("{id} (assignment for {task})"))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(Self::success(format!(
                "No unread messages for {recipient} — withheld {} already-delivered \
                 message(s) whose requested action is already done: {ids}",
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
                ". Withheld {} already-delivered message(s) whose requested action is already \
                 recorded: {}",
                withheld.len(),
                withheld
                    .iter()
                    .map(|(id, task)| format!("{id} (assignment for {task})"))
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
                "notification_id required for message_ack (the prompt queue message ID)",
            )
        })?;

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {error}"),
            )
        })?;

        queue.ack(notification_id).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to acknowledge message: {error}"),
            )
        })?;

        // cas-45c4 (GH #102): say what an ack actually proves. It is the
        // caller's claim that it received this message — not evidence CAS
        // observed the content being surfaced, and not a guarantee for anyone
        // else's copy of a broadcast.
        Ok(Self::success(format!(
            "Message {notification_id} acknowledged by this session (confirmation_source: \
             explicit_ack). This records YOUR claim to have received it; CAS does not \
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
                            " — CAS inferred this from later activity; the recipient never \
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
                // read it and learned nothing, because it could not tell "CAS
                // never nudged this recipient" from "CAS nudged it and the
                // harness started a turn without surfacing the message".
                // `wake_attempt` is the daemon's own record of which of those
                // happened; `wake` remains recipient-side evidence.
                let wake_attempt_line = wake_attempt_narrative(r.wake_attempt, r.wake);
                Ok(Self::success(format!(
                    "Message {notification_id} status: {}\n\
                     stage: {}  pending_reason: {}  wake: {}  wake_attempt: {}  \
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
            && report.factory_session.as_ref().is_none_or(|session| {
                agent.factory_session.as_ref() == Some(session)
            })
    }) else {
        return;
    };
    let cli = crate::mcp::tools::service::factory_ops::worker_cli_from_agent(&agent);
    let Some(path) = crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(
        cas_root,
        &agent,
    ) else {
        return;
    };
    let observations =
        crate::mcp::tools::service::harness_observation::observations_after_delivery(
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

#[cfg(test)]
mod inbox_poll_identity_tests {
    use super::{
        enrich_report_from_harness_artifact, recipient_transport_warning, resolve_inbox_recipient,
    };
    use cas_store::DeliveryStage;

    /// cas-7a01 (GH #155): the combination that used to be completely
    /// invisible — CAS nudged the pane and no turn ever carried the message —
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
        assert_eq!(unique.len(), 3, "wake states collapse to the same text: {lines:?}");
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
        let mut agent = cas_types::Agent::new(
            "live-worker-session".to_string(),
            "worker-a".to_string(),
        );
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
        assert!(serde_json::to_value(&report).unwrap().get("prompt").is_none());

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
}

#[cfg(test)]
mod cas99d2_redelivery_tests {
    use super::{
        INBOX_REDELIVERY_MARKER, InboxRedelivery, assignment_solicited_task_id,
        inbox_redelivery_decision,
    };

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
