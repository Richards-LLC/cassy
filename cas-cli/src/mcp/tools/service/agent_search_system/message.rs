use crate::mcp::tools::service::imports::*;

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
        let message = req.message.ok_or_else(|| {
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

        // cas-6ad2: sending a response is an authoritative recipient-side
        // consumption signal for prior messages from that counterparty. The
        // old explicit message_ack API was never invoked by factory prompts,
        // leaving every acted-on message stuck at Delivered/AwaitingAck.
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
        if let Err(error) = queue.ack_delivered_for_recipient(
            &recipient_aliases,
            &counterparty_aliases,
            factory_session.as_deref(),
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
        let recipient = registered_agent
            .as_ref()
            .map(|agent| agent.name.clone())
            .or_else(|| std::env::var("CAS_AGENT_NAME").ok())
            .filter(|name| !name.trim().is_empty())
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
        // This is an at-most-once claim for the polling API: the store records
        // recipient-seen state in the same transaction that selects the rows,
        // before this MCP response is handed back. That prevents concurrent
        // duplicate delivery, but a response lost after this point is not
        // replayed by a later poll. Daemon transport state remains independent.
        let messages = queue
            .poll_unseen_for_recipient(&recipient, factory_session.as_deref(), limit)
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to poll recipient inbox: {error}"),
                )
        })?;

        if messages.is_empty() {
            return Ok(Self::success(format!(
                "No unread messages for {recipient}"
            )));
        }

        let mut output = format!(
            "Pulled {} unread message(s) for {recipient} (at-most-once inbox claim: \
             marked seen before this response is delivered; daemon transport delivery is \
             unchanged):\n\n",
            messages.len()
        );
        for message in &messages {
            output.push_str(&format!(
                "**[{}] From: {}**\nSummary: {}\nCreated: {}\nMessage: {}\n\n",
                message.id,
                message.source,
                message.summary.as_deref().unwrap_or("(no summary)"),
                message.created_at.to_rfc3339(),
                message.prompt,
            ));
        }

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

        Ok(Self::success(format!(
            "Message {notification_id} acknowledged (delivery confirmed)"
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
            Some(r) => {
                // cas-893c AC2: `delivered_at` is only transport handoff (the
                // teams-inbox write / PTY inject succeeding) — cas cannot
                // observe whether the recipient's harness actually consumed
                // it (see `ObservationStatus` / the `wake`/`reaction` fields
                // below, which stay `Unobserved` because cas has no signal
                // for that). `confirmed_at` is the only field that means
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
                    None => "undelivered_after: n/a (recipient confirmed via message_ack)\n"
                        .to_string(),
                };
                Ok(Self::success(format!(
                    "Message {notification_id} status: {}\n\
                     stage: {}  pending_reason: {}  wake: {}  reaction: {}\n\
                     {undelivered_line}\
                     {json}",
                    r.legacy_status, r.stage, reason, r.wake, r.reaction
                )))
            }
            None => Ok(Self::success(format!(
                "Message {notification_id} not found"
            ))),
        }
    }
}
