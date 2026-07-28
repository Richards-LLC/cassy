use crate::mcp::tools::service::imports::*;

/// Heartbeat age at which a worker is considered **stale** and becomes
/// eligible for the opportunistic prune in `factory_worker_status`.
///
/// Re-exported from [`super::agent_liveness`] (cas-e98e single source of
/// truth). Callers/tests that assert the 30s pin continue to use this
/// name.
pub(crate) use super::agent_liveness::WORKER_STALE_SECS;

/// Heartbeat age at which a worker is escalated to **dead** in the
/// supervisor-facing render: hard `[DEAD]` label + transcript-path
/// surfacing so the supervisor can salvage the last in-flight tool call.
///
/// Two-band model (cas-8240): `WORKER_STALE_SECS` (30s) drives the
/// opportunistic prune and a lighter-weight `[stale]` indicator on any
/// worker that slipped past the prune (e.g. `mark_stale` hit a DB lock).
/// `WORKER_DEAD_SECS` (75s) gates the more expensive `[DEAD]` + transcript
/// emission so tokio scheduler jitter or a missed 30s daemon tick cannot
/// produce false-positive DEAD labels that train supervisors to distrust
/// the signal. Picked at 2.5× the stale threshold: gives the daemon one
/// full heartbeat interval of grace past the prune window before the
/// render escalates, which in practice means a worker has to have
/// missed at least two consecutive heartbeats before we surface it as
/// dead.
pub(crate) const WORKER_DEAD_SECS: i64 = 75;

/// The harness (`SupervisorCli`) a worker registered under, read from
/// `Agent.metadata["worker_cli"]` (cas-058f: persisted at registration time
/// from `CAS_FACTORY_WORKER_CLI`, mirroring the existing `worker_effort`
/// pattern — see `apply_factory_worker_metadata` in `mcp/daemon.rs`).
/// Defaults to `Claude` for legacy agents registered before this metadata
/// key existed, or any unparseable value — the same "no signal ⇒ don't
/// guess exotic" default the rest of the harness-detection code already
/// uses (`worker_harness_from_env`).
pub(crate) fn worker_cli_from_agent(agent: &cas_types::Agent) -> cas_mux::SupervisorCli {
    agent
        .metadata
        .get("worker_cli")
        .and_then(|s| s.parse::<cas_mux::SupervisorCli>().ok())
        .unwrap_or(cas_mux::SupervisorCli::Claude)
}

fn parse_spawn_cli(cli: Option<&str>) -> Result<Option<cas_mux::SupervisorCli>, String> {
    cli.map(|s| {
        s.parse::<cas_mux::SupervisorCli>()
            .map_err(|_| format!("invalid cli value {s:?}: expected 'claude', 'codex', or 'grok'"))
    })
    .transpose()
}

fn parse_spawn_effort(effort: Option<&str>) -> Result<Option<cas_mux::Effort>, String> {
    match effort {
        Some(s) => Ok(Some(
            s.parse::<cas_mux::Effort>()
                .map_err(|e| format!("invalid effort value {s:?}: {e}"))?,
        )),
        None => Ok(None),
    }
}

fn default_worker_model_for_cli(cli: cas_mux::SupervisorCli) -> &'static str {
    match cli {
        cas_mux::SupervisorCli::Claude => "sonnet",
        cas_mux::SupervisorCli::Codex => crate::config::STOCK_WORKER_MODEL,
        // EPIC cas-8888 (cas-9a31, Phase 1): grok 0.2.93 default model.
        cas_mux::SupervisorCli::Grok => "grok-4.5",
    }
}

fn default_worker_effort_for_cli(_cli: cas_mux::SupervisorCli) -> cas_mux::Effort {
    crate::config::STOCK_WORKER_REASONING_EFFORT
        .parse::<cas_mux::Effort>()
        .unwrap_or(cas_mux::Effort::Medium)
}

fn is_frontier_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.contains("opus") || model.contains("fable") || model.contains("mythos")
}

fn format_effort(effort: Option<cas_mux::Effort>) -> String {
    effort
        .map(|e| e.to_string())
        .unwrap_or_else(|| "(backend default)".to_string())
}

/// Build a JSON-serialized [`cas_mux::WorkerSpec`] from optional string overrides
/// supplied via the MCP `spawn_workers` action or the cloud protocol.
///
/// Returns `Err(String)` when a parameter value is invalid.
#[cfg(test)]
pub(crate) fn build_spawn_spec_json(
    cli: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<String, String> {
    build_spawn_spec_json_with_project_config(cli, model, effort, None)
}

pub(crate) fn build_spawn_spec_json_with_project_config(
    cli: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    project_config: Option<std::path::PathBuf>,
) -> Result<String, String> {
    let parsed_cli = parse_spawn_cli(cli)?;
    let parsed_effort = parse_spawn_effort(effort)?;

    let sources = cas_factory::ConfigSources {
        project_config,
        cli_flag: parsed_cli,
        model_flag: model.map(String::from),
        effort_flag: parsed_effort,
        ..Default::default()
    };
    let configured_cli = cas_factory::worker_slot_cli_configured(0, &sources)
        .map_err(|e| format!("failed to inspect worker cli config: {e}"))?;
    let configured_effort = cas_factory::worker_slot_effort_configured(0, &sources)
        .map_err(|e| format!("failed to inspect worker effort config: {e}"))?;
    let mut spec = cas_factory::resolve_specs(1, sources)
        .map_err(|e| format!("failed to resolve worker spec: {e}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "failed to resolve worker spec: no worker slots returned".to_string())?;

    // EPIC cas-8888 (cas-9a31, Phase 1) SILENT SITE — audited, left AS-IS
    // per the task's own guidance: this default-cli auto-upgrade only ever
    // fires when the resolved default happens to be Claude (never Grok, since
    // nothing defaults TO Grok yet — it isn't a stock/default CLI at this
    // phase), so no Grok arm is needed here.
    if cli.is_none() && !configured_cli && spec.cli == cas_mux::SupervisorCli::Claude {
        spec.cli = cas_mux::SupervisorCli::Codex;
    }
    if model.is_none() && spec.model.is_none() {
        spec.model = Some(default_worker_model_for_cli(spec.cli).to_string());
    }
    if effort.is_none() && !configured_effort && spec.effort == Some(cas_mux::Effort::High) {
        spec.effort = Some(default_worker_effort_for_cli(spec.cli));
    }

    let json =
        serde_json::to_string(&spec).map_err(|e| format!("failed to serialize WorkerSpec: {e}"))?;
    Ok(json)
}

/// `[factory] strict_cli` lookup for the cas-7199 / cas-a487 Codex-fallback
/// check applied in `factory_spawn_workers`. Deliberately NOT inside
/// `build_spawn_spec_json_with_project_config` above: that function is pure
/// cascade resolution + serialization, exercised directly by unit tests
/// (e.g. `spawn_spec_omitted_cli_without_config_uses_stock_codex_defaults`)
/// that assert the stock/default `cli` value itself and run under an
/// isolated fake `HOME` — folding a REAL `codex_available()` probe in
/// there would make those tests depend on whether the isolated `HOME`
/// happens to contain a `.codex/auth.json` (it never does), silently
/// changing their resolved `cli` out from under them and making the whole
/// suite depend on the test machine's actual Codex install/login state.
/// The availability check belongs at the actual "about to queue this for
/// spawn" checkpoint instead — see `factory_spawn_workers` below, which
/// mirrors exactly where `cli/factory/mod.rs` applies the same fallback:
/// AFTER cascade resolution, not inside it.
fn strict_cli_from_project_config(project_config: Option<&std::path::Path>) -> bool {
    project_config
        .and_then(|p| p.parent())
        .map(|cas_root| {
            use crate::config::Config;
            Config::load(cas_root).unwrap_or_default().factory().strict_cli
        })
        .unwrap_or(false)
}

fn spawn_spec_summary(spec_json: &str) -> String {
    match serde_json::from_str::<cas_mux::WorkerSpec>(spec_json) {
        Ok(spec) => format!(
            "{} model={} effort={}",
            spec.cli.as_str(),
            spec.model.as_deref().unwrap_or("(backend default)"),
            format_effort(spec.effort)
        ),
        Err(_) => "unparseable worker spec".to_string(),
    }
}

fn spawn_spec_warning(model_explicit: bool, effort_explicit: bool, spec_json: &str) -> String {
    let mut warnings = Vec::new();
    if !model_explicit || !effort_explicit {
        warnings.push(
            "Warning: spawn_workers should include explicit model= and effort=; omitted fields were resolved to safe worker defaults for this request."
                .to_string(),
        );
    }
    if !model_explicit {
        if let Ok(spec) = serde_json::from_str::<cas_mux::WorkerSpec>(spec_json) {
            if spec.model.as_deref().is_some_and(is_frontier_model) {
                warnings.push(format!(
                    "Warning: resolved worker model {} is frontier-tier but model= was omitted; pass model= explicitly to acknowledge the cost/risk.",
                    spec.model.as_deref().unwrap_or("unknown")
                ));
            }
        }
    }
    if warnings.is_empty() {
        String::new()
    } else {
        format!("\n{}", warnings.join("\n"))
    }
}

fn current_factory_session() -> Option<String> {
    std::env::var("CAS_FACTORY_SESSION")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn parse_worker_name_filter(filter: Option<&String>) -> std::collections::HashSet<String> {
    filter
        .into_iter()
        .flat_map(|names| names.split(','))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

impl CasService {
    pub(super) async fn factory_spawn_workers(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_spawn_queue_store, open_task_store};
        use cas_types::{TaskStatus, TaskType};

        // Check that there's an active EPIC before spawning workers
        let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open task store: {e}"),
            )
        })?;

        let open_epics: Vec<_> = task_store
            .list(None)
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list tasks: {e}"),
                )
            })?
            .into_iter()
            .filter(|t| t.task_type == TaskType::Epic && t.status != TaskStatus::Closed)
            .collect();

        if open_epics.is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_REQUEST,
                "No active EPIC found. Before spawning workers, create or assign an EPIC:\n\
                 1. Create EPIC: mcp__cas__task action=create task_type=epic title=\"...\" description=\"...\"\n\
                 2. Or assign existing EPIC: mcp__cas__task action=start id=<epic-id>\n\
                 3. Optionally gather requirements using the epic-spec skill\n\
                 4. Break into tasks using the epic-breakdown skill\n\
                 5. Then spawn workers to work on the tasks",
            ));
        }

        let queue = open_spawn_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spawn queue: {e}"),
            )
        })?;

        let count = req.count.unwrap_or(1);
        let isolate = req.isolate.unwrap_or(false);
        let worker_names: Vec<String> = req
            .worker_names
            .map(|names| {
                names
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // cas-6913: task_id pre-assigns a task to the (single) spawned
        // worker. Reject anything that would make "the spawned worker"
        // ambiguous — a task can't be assigned to N workers at once — so a
        // supervisor gets a clear error instead of silent no-op or a
        // surprising assignment to whichever worker happens to finish
        // spawning first.
        if let Some(ref task_id) = req.task_id {
            let requested_worker_count = if worker_names.is_empty() {
                count
            } else {
                worker_names.len() as i32
            };
            if requested_worker_count != 1 {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "task_id can only be used with a single-worker spawn_workers request \
                         (count=1, or exactly one name in worker_names) — got {requested_worker_count} \
                         worker(s) requested. Assign the task after spawning instead: \
                         mcp__cas__task action=update id={task_id} assignee=<worker-name>."
                    ),
                ));
            }

            let task = task_store.get(task_id).map_err(|e| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("task_id {task_id} not found: {e}"),
                )
            })?;
            if task.status == TaskStatus::Closed {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("task_id {task_id} is already closed — cannot pre-assign it to a spawned worker."),
                ));
            }
        }

        // Resolve a concrete WorkerSpec for every queued spawn. Omitting model
        // or effort must never inherit the supervisor session's frontier-tier
        // defaults by accident.
        let mut spec_json_owned: String = build_spawn_spec_json_with_project_config(
            req.cli.as_deref(),
            req.model.as_deref(),
            req.effort.as_deref(),
            Some(self.inner.cas_root.join("config.toml")),
        )
        .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e))?;

        // cas-7199 / cas-a487: this is the mid-session `spawn_workers` MCP
        // path — the one the original incident actually hit (a worker
        // whose resolved spec requested Codex on a host without it, queued
        // and only failing much later as a raw PTY spawn error). Applied
        // HERE — the "about to actually queue this" checkpoint — rather
        // than inside `build_spawn_spec_json_with_project_config`, so the
        // pure-resolution unit tests for that function stay independent of
        // real host Codex install/login state (see
        // `strict_cli_from_project_config`'s doc comment for why that
        // matters). Mirrors exactly where `cli/factory/mod.rs` applies the
        // same fallback: after cascade resolution, not inside it.
        let mut codex_fallback_notice = String::new();
        if let Ok(mut spec) = serde_json::from_str::<cas_mux::WorkerSpec>(&spec_json_owned) {
            let strict_cli =
                strict_cli_from_project_config(Some(&self.inner.cas_root.join("config.toml")));
            let claude_default_model = default_worker_model_for_cli(cas_mux::SupervisorCli::Claude);
            let notices = cas_factory::apply_codex_fallback(
                std::slice::from_mut(&mut spec),
                strict_cli,
                Some(claude_default_model),
            )
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e.to_string()))?;
            if !notices.is_empty() {
                for notice in &notices {
                    tracing::warn!(target: "cas::factory", "{notice}");
                }
                codex_fallback_notice = format!("\nWarning: {}", notices.join("\nWarning: "));
                // The fallback rewrote `cli` (and possibly `model`) — the
                // queued spec and the summary shown back to the caller must
                // reflect what will ACTUALLY spawn, not the pre-fallback request.
                spec_json_owned = serde_json::to_string(&spec).map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("failed to re-serialize worker spec after codex fallback: {e}"),
                    )
                })?;
            }
        }

        let spec_summary = spawn_spec_summary(&spec_json_owned);
        let spec_warning =
            spawn_spec_warning(req.model.is_some(), req.effort.is_some(), &spec_json_owned);

        let factory_session = current_factory_session();
        let request_id = queue
            .enqueue_spawn(
                count,
                &worker_names,
                isolate,
                Some(spec_json_owned.as_str()),
                factory_session.as_deref(),
                req.task_id.as_deref(),
            )
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to queue spawn request: {e}"),
                )
            })?;

        let task_id_note = req
            .task_id
            .as_ref()
            .map(|id| format!("\nTask: {id} will be pre-assigned once the worker boots"))
            .unwrap_or_default();
        let request_id_text = request_id.to_string();
        let count_text = count.to_string();
        let worker_names_text = worker_names.join(",");
        let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
            &self.inner.cas_root,
            "workers_spawn_queued",
            &[
                ("request_id", &request_id_text),
                ("count", &count_text),
                ("workers", &worker_names_text),
                ("task_id", req.task_id.as_deref().unwrap_or("")),
                ("isolate", if isolate { "true" } else { "false" }),
            ],
        );

        let msg = if worker_names.is_empty() {
            format!(
                "Queued spawn request for {count} worker(s) (request ID: {request_id})\nWorker spec: {spec_summary}{spec_warning}{codex_fallback_notice}{task_id_note}"
            )
        } else {
            format!(
                "Queued spawn request for worker(s): {} (request ID: {})\nWorker spec: {spec_summary}{spec_warning}{codex_fallback_notice}{task_id_note}",
                worker_names.join(", "),
                request_id
            )
        };

        Ok(Self::success(msg))
    }

    pub(super) async fn factory_shutdown_workers(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_spawn_queue_store};
        use cas_types::{AgentRole, AgentStatus};

        let mut worker_names: Vec<String> = req
            .worker_names
            .map(|names| {
                names
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // When supervisor has no specific worker names requested, scope to owned workers
        // so a supervisor cannot shut down another supervisor's workers.
        if worker_names.is_empty() {
            if let Some(owned) = supervisor_owned_workers() {
                worker_names = owned.into_iter().collect();
            }
        }

        // Validate workers exist before queuing (synchronous validation)
        if !worker_names.is_empty() {
            let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open agent store: {e}"),
                )
            })?;

            // Include both active and stale workers — stale workers are often
            // exactly what supervisors want to shut down.
            let mut known_agents = agent_store.list(Some(AgentStatus::Active)).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list agents: {e}"),
                )
            })?;
            if let Ok(stale) = agent_store.list(Some(AgentStatus::Stale)) {
                known_agents.extend(stale);
            }

            // Get worker names, scoped to this supervisor's workers when applicable
            let owned = supervisor_owned_workers();
            let factory_session = current_factory_session();
            let known_workers: std::collections::HashSet<String> = known_agents
                .iter()
                .filter(|a| {
                    a.role == AgentRole::Worker
                        && a.visible_to_factory_session(factory_session.as_deref())
                        && owned.as_ref().is_none_or(|set| set.contains(&a.name))
                })
                .map(|a| a.name.clone())
                .collect();

            // Check each requested worker exists
            let mut not_found = Vec::new();
            for name in &worker_names {
                if !known_workers.contains(name) {
                    not_found.push(name.clone());
                }
            }

            if !not_found.is_empty() {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Worker(s) not found: {}. Known workers: {}",
                        not_found.join(", "),
                        if known_workers.is_empty() {
                            "(none)".to_string()
                        } else {
                            known_workers.into_iter().collect::<Vec<_>>().join(", ")
                        }
                    ),
                ));
            }
        }

        // Validation passed, queue the shutdown
        let queue = open_spawn_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spawn queue: {e}"),
            )
        })?;

        let count = req.count;
        let force = req.force.unwrap_or(false);
        let factory_session = current_factory_session();
        let request_id = queue
            .enqueue_shutdown(count, &worker_names, force, factory_session.as_deref())
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to queue shutdown request: {e}"),
                )
            })?;
        let request_id_text = request_id.to_string();
        let count_text = count.map(|value| value.to_string()).unwrap_or_default();
        let worker_names_text = worker_names.join(",");
        let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
            &self.inner.cas_root,
            "workers_shutdown_queued",
            &[
                ("request_id", &request_id_text),
                ("count", &count_text),
                ("workers", &worker_names_text),
                ("force", if force { "true" } else { "false" }),
            ],
        );

        let msg = if !worker_names.is_empty() {
            format!(
                "Queued shutdown request for worker(s): {} (request ID: {})",
                worker_names.join(", "),
                request_id
            )
        } else if let Some(c) = count {
            if c == 0 {
                format!("Queued shutdown request for ALL workers (request ID: {request_id})")
            } else {
                format!("Queued shutdown request for {c} worker(s) (request ID: {request_id})")
            }
        } else {
            format!("Queued shutdown request for ALL workers (request ID: {request_id})")
        };

        Ok(Self::success(msg))
    }

    pub(super) async fn factory_worker_status(
        &self,
        _req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_agent_store;
        use cas_types::{AgentRole, AgentStatus};

        let store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;

        // Opportunistically prune stale agents so status output stays actionable.
        // Worker threshold tightened from 120s → 30s per cas-2749 so a dead CC
        // client is detected within one supervisor poll. Paired with the
        // daemon-side PID liveness gate in mcp::daemon::send_agent_heartbeat,
        // a crashed worker stops heartbeating within the 30s daemon tick and
        // transitions to "dead" in the next status call. Supervisors/directors
        // are long-lived and less chatty and are filtered out of the prune by
        // the role check below; they remain visible until their own
        // daemon-level cleanup eventually removes them.
        //
        // See the module-level `WORKER_STALE_SECS` and `WORKER_DEAD_SECS`
        // constants (cas-8240) for the two-band model that separates the
        // prune + `[stale]` indicator (30s) from the hard `[DEAD]` + transcript
        // surface (75s).
        //
        // cas-1ec7: cross-correlate with the worker_activity event log before
        // pruning. A worker whose heartbeat lapsed during a long CPU-bound
        // operation (cargo build/test stretches) but who has a recent
        // WorkerFileEdited or WorkerGitCommit event within the same window is
        // NOT stale — it's just busy. Suppress the prune for these workers and
        // surface a "[heartbeat stale, active I/O]" annotation so the
        // supervisor can see the dual-signal without misdiagnosing a dead worker.
        let worker_stale_threshold_secs: i64 = WORKER_STALE_SECS;

        // Query recent I/O events once, reuse per agent in the prune loop.
        let recent_io_cutoff =
            chrono::Utc::now() - chrono::Duration::seconds(worker_stale_threshold_secs);
        let recent_io_events: Vec<cas_types::Event> = {
            use cas_store::{EventStore, SqliteEventStore};
            SqliteEventStore::open(&self.inner.cas_root)
                .and_then(|es| es.list_since(recent_io_cutoff, 50))
                .unwrap_or_default()
        };

        // cas-9829: configurable stall threshold (`[factory]
        // stall_threshold_secs` in `.cas/config.toml`, default
        // `cas_factory::DEFAULT_STALL_THRESHOLD_SECS`) used below to mark a
        // worker row `⚠ STALLED` when it has an in-progress task but no
        // observable activity past the threshold — the same signal the
        // director uses for the `WorkerStalled` auto-nudge/escalation, now
        // surfaced for a supervisor manually polling `worker_status`.
        let stall_threshold_secs: i64 = {
            use crate::config::Config;
            Config::load(&self.inner.cas_root)
                .unwrap_or_default()
                .factory()
                .stall_threshold_secs as i64
        };

        // cas-86c5: wider activity window (10 min) for the per-worker "last
        // activity" line in worker_status. A worker in the investigation phase
        // emits checkpoint events but no edits; showing "last activity: 45s ago
        // (checkpoint)" is the signal that tells the supervisor NOT to reset.
        const ACTIVITY_WINDOW_SECS: i64 = 600;
        let activity_cutoff = chrono::Utc::now() - chrono::Duration::seconds(ACTIVITY_WINDOW_SECS);
        let activity_events: Vec<cas_types::Event> = {
            use cas_store::{EventStore, SqliteEventStore};
            SqliteEventStore::open(&self.inner.cas_root)
                .and_then(|es| es.list_since(activity_cutoff, 200))
                .unwrap_or_default()
        };

        // Agents suppressed from pruning due to recent I/O activity or a live
        // harness PID. Used in the render loop to show the dual-signal
        // annotation (cas-1ec7 I/O / cas-3e56 process-alive).
        let mut active_io_suppressed: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut process_alive_suppressed: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut stale_pruned = 0usize;
        let factory_session = current_factory_session();
        if let Ok(stale_agents) = store.list_stale(worker_stale_threshold_secs) {
            for agent in stale_agents {
                if !agent.visible_to_factory_session(factory_session.as_deref()) {
                    continue;
                }
                if agent.role == AgentRole::Supervisor || agent.role == AgentRole::Director {
                    continue;
                }
                // cas-1ec7: suppress stale-prune when observable I/O is recent.
                if has_recent_worker_io_activity(&recent_io_events, &agent.id) {
                    active_io_suppressed.insert(agent.id.clone());
                    continue;
                }
                // cas-3e56 / cas-e98e: suppress when process proves mid-turn.
                // Prefer revive of already-Stale rows so agent_list and
                // worker_status agree on Active identity.
                if agent_process_is_alive(&agent) {
                    process_alive_suppressed.insert(agent.id.clone());
                    let _ = store.revive(&agent.id);
                    continue;
                }
                // cas-2e81: capture held leases BEFORE mark_stale revokes them,
                // then park orphaned InProgress tasks + emit worker_died.
                let held: Vec<String> = store
                    .list_agent_leases(&agent.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|l| l.task_id)
                    .collect();
                if store.mark_stale(&agent.id).is_ok() {
                    stale_pruned += 1;
                    let _ = super::orphan_recovery::recover_worker_vanished(
                        &self.inner.cas_root,
                        store.as_ref(),
                        &agent,
                        &held,
                        "worker_status stale prune (heartbeat gone, process not alive)",
                    );
                }
            }
        }

        // cas-2e81: reclaim expired leases and park tasks when the holder is
        // already dead/stale (lease expiry alone must not silence orphans).
        if let Ok(active_leases) = store.list_active_leases() {
            let now = chrono::Utc::now();
            let expired: Vec<(String, String)> = active_leases
                .into_iter()
                .filter(|l| l.expires_at < now)
                .map(|l| (l.task_id, l.agent_id))
                .collect();
            if !expired.is_empty() {
                let _ = store.reclaim_expired_leases();
                let _ = super::orphan_recovery::recover_expired_leases_for_dead_holders(
                    &self.inner.cas_root,
                    store.as_ref(),
                    &expired,
                    worker_stale_threshold_secs,
                );
            }
        }

        let agents: Vec<_> = store
            .list(Some(AgentStatus::Active))
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list agents: {e}"),
                )
            })?
            .into_iter()
            .filter(|a| a.visible_to_factory_session(factory_session.as_deref()))
            .collect();

        // cas-2e81: always surface recently-died-while-leased even when the
        // Active roster is empty — "None active" must not hide a mid-P0 crash.
        let died_section = super::orphan_recovery::format_recently_died_while_leased(
            &self.inner.cas_root,
            store.as_ref(),
            factory_session.as_deref(),
            3600, // 1h window
        );

        if agents.is_empty() {
            let mut msg = String::from(
                "No active agents registered.\n\nNote: Factory TUI must be running for agents to be registered.",
            );
            msg.push_str(&died_section);
            if stale_pruned > 0 {
                msg.push_str(&format!(
                    "\nFiltered stale agent record(s): {stale_pruned} (>{worker_stale_threshold_secs}s heartbeat age)\n"
                ));
            }
            return Ok(Self::success(msg));
        }

        let owned = supervisor_owned_workers();
        let mut output = String::from("Worker Status\n=============\n\n");

        // cas-d165 (Finding 2): assignees of currently InProgress tasks,
        // resolved ONCE for the whole roster. `has_in_progress_task` below
        // must not rely on lease presence alone — leases are a fixed-
        // duration claim nothing in production renews (see the long
        // comment at the `has_in_progress_task` computation), so they
        // silently expire under a genuinely working agent. Real task
        // assignment is the ground truth; a lease is corroborating (and
        // currently the only) evidence *before* that expiry.
        let in_progress_assignees: std::collections::HashSet<String> = {
            use crate::store::open_task_store;
            open_task_store(&self.inner.cas_root)
                .ok()
                .and_then(|ts| ts.list(Some(cas_types::TaskStatus::InProgress)).ok())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|t| t.assignee)
                .collect()
        };

        let workers: Vec<_> = agents
            .iter()
            .filter(|a| {
                a.role == AgentRole::Worker
                    && owned.as_ref().is_none_or(|set| set.contains(&a.name))
            })
            .collect();
        let self_name = std::env::var("CAS_AGENT_NAME").ok();
        let supervisors: Vec<_> = agents
            .iter()
            .filter(|a| {
                (a.role == AgentRole::Supervisor || a.role == AgentRole::Director)
                    && if owned.is_some() {
                        // When scoped, only show this supervisor (not others)
                        self_name.as_ref() == Some(&a.name)
                    } else {
                        true
                    }
            })
            .collect();

        if !supervisors.is_empty() {
            output.push_str("Supervisors:\n");
            for agent in supervisors {
                let elapsed = (chrono::Utc::now() - agent.last_heartbeat).num_seconds();
                let since = format!("{elapsed}s ago");
                output.push_str(&format!("  • {} (heartbeat: {})\n", &agent.name, since));
            }
            output.push('\n');
        }

        if workers.is_empty() {
            output.push_str("Workers: None active\n");
        } else {
            output.push_str(&format!("Workers ({}):\n", workers.len()));
            for agent in workers {
                let elapsed = (chrono::Utc::now() - agent.last_heartbeat).num_seconds();
                let since = format!("{elapsed}s ago");
                // cas-8240 two-band model — see `liveness_label_for`.
                //
                // cas-1ec7: prefer "[heartbeat stale, active I/O]" over "[stale]"
                // for workers that were suppressed from the stale-prune because a
                // recent WorkerFileEdited/WorkerGitCommit event confirms they are
                // still making progress despite a heartbeat lapse (e.g. during a
                // long cargo build/test run).
                //
                // cas-3e56: "[alive — heartbeat stale]" when the registered
                // harness PID is still live. Honest dual-signal so supervisors
                // never re-spawn solely on heartbeat age.
                let liveness_label = if active_io_suppressed.contains(&agent.id) {
                    " [heartbeat stale, active I/O]"
                } else if process_alive_suppressed.contains(&agent.id) {
                    " [alive — heartbeat stale]"
                } else {
                    liveness_label_for(elapsed)
                };
                let worktree_status = collect_worker_worktree_status(&self.inner.cas_root, agent);
                let clone_path = worktree_status.clone_path;
                let clone_info = worktree_status.clone_info;
                // cas-844bf: git introspection — branch/HEAD/ahead-behind/dirty/PR
                let git_info = worktree_status.git_info;
                // Surface transcript path only for hard-dead workers so the
                // supervisor can salvage whatever was in-flight when the CC
                // client died (cas-2749 AC: transcript-path-surfacing on
                // crash). The `[stale]` tier does NOT emit the transcript —
                // a worker lagging past 30s under scheduler jitter does not
                // need its transcript surfaced yet, and emitting it there
                // would produce the false-positive noise cas-8240 is fixing.
                //
                // cas-900b: when we do emit, use `format_transcript_block`
                // which globs ~/.claude/projects/*/<session_id>.jsonl for
                // the real on-disk path and falls back to the reconstructed
                // path only when the glob can't pin a unique match. Always
                // surfaces session_id so a supervisor who doesn't trust our
                // resolution can grep the projects tree themselves.
                //
                // In factory mode, `agent.id` is the CC SessionStart UUID
                // (daemon.rs + server/mod.rs both construct the Agent via
                // `Agent::new(session_id, name)`). If `cc_session_id` is
                // ever populated separately in the future we prefer it —
                // but for now `id` is the right key and has been correct
                // since cas-2749.
                let transcript_info = if elapsed >= WORKER_DEAD_SECS {
                    let session_id = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
                    format_transcript_block(
                        clone_path.as_deref(),
                        session_id,
                        worker_cli_from_agent(agent),
                    )
                } else {
                    String::new()
                };
                // Surface session UUID alongside the friendly name so the
                // supervisor can cross-reference task-ownership errors
                // ("owned by worker-backfill (0a7f2802-...)") without manual
                // table-lookup. cas-85bf.
                let session_uuid = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
                let model_info = match (
                    agent.metadata.get("worker_model"),
                    agent.metadata.get("worker_effort"),
                ) {
                    (Some(model), Some(effort)) => {
                        format!("\n    model: {model}\n    effort: {effort}")
                    }
                    (Some(model), None) => format!("\n    model: {model}\n    effort: unknown"),
                    (None, Some(effort)) => format!("\n    model: unknown\n    effort: {effort}"),
                    (None, None) => String::new(),
                };
                // Context usage (cas-573c): cheap tail-read of the session
                // transcript to surface a coarse band so the supervisor can
                // proactively preserve work before compaction. Falls back
                // silently when the transcript isn't found yet (new workers).
                //
                // cas-d165: hoisted out of the `context_info` block so the
                // same resolved path can also feed the in-flight-tool-call
                // check below — one resolution, two consumers, instead of
                // globbing/reading the transcript twice per worker.
                let worker_cli = worker_cli_from_agent(agent);
                let transcript_path_for_worker = worker_status_transcript_path(
                    clone_path.as_deref(),
                    session_uuid,
                    worker_cli,
                );
                let context_info = {
                    match transcript_path_for_worker
                        .as_deref()
                        .and_then(|path| read_context_usage_from_tail_for_cli(path, worker_cli))
                    {
                        Some(total) => {
                            let band = context_band(total);
                            let ktok = total / 1_000;
                            format!("\n    context: {band} (~{ktok}k tk)")
                        }
                        None => String::new(),
                    }
                };
                // cas-86c5: surface per-worker "last activity" age so the
                // supervisor can distinguish an actively-investigating worker
                // (no edits yet, but recent checkpoint events) from one that
                // is truly stalled. A clean diff + fresh checkpoint is normal
                // during the analysis phase; no events for >5 min is a stall.
                // cas-9829: a worker with an in-progress task and no
                // activity past `stall_threshold_secs` is STALLED, not
                // merely "investigating" — the soft hedge reads as fine and
                // is easy to skim past in a supervisor's manual poll.
                //
                // cas-d165 (Finding 2, ozer wave-2 report): a lease is a
                // fixed-duration claim that NOTHING in production ever
                // renews (`renew_lease` has no MCP-tool call site — grep
                // confirms it's dead outside tests). Any task that runs
                // longer than the configured lease duration (default 30m,
                // `[lease] default_duration_mins`) has its lease reclaimed
                // by the `reclaim_expired_leases` sweep above **even while
                // the worker is genuinely, actively working** — heartbeat
                // freshness alone keeps `recover_expired_leases_for_dead_holders`
                // from touching the task, so `task.status`/`assignee` stay
                // exactly as they were, but `list_agent_leases` now returns
                // empty. Gating `has_in_progress_task` on the lease alone
                // therefore silently goes blind ~30 minutes into every
                // single task, tripping exactly the "task list
                // status=in_progress returns ZERO despite every worker
                // having an assignee" symptom from the fleet-wide-wedge
                // report. Fixed by ALSO checking real task assignment
                // (`in_progress_assignees`, built once above from
                // `task_store.list(InProgress)`) — a worker counts as
                // holding an assignment if it has an active lease OR an
                // InProgress task assigned to its name/id, matching the
                // supervisor's "assignment-or-lease, not lease alone"
                // guidance.
                let has_in_progress_task = store
                    .list_agent_leases(&agent.id)
                    .map(|leases| !leases.is_empty())
                    .unwrap_or(false)
                    || in_progress_assignees.contains(agent.name.as_str())
                    || in_progress_assignees.contains(agent.id.as_str());
                // cas-a653: hook-less harnesses (Codex) only emit CAS events
                // on their own MCP calls — fold in the transcript's own mtime
                // so this doesn't freeze at the age of the last CAS call
                // while the worker keeps working via exec_command/apply_patch.
                // Reuses the same transcript path already resolved above for
                // context_info/in_flight_tool_call, and the same
                // wedged::transcript_mtime_age primitive `is-wedged` trusts.
                let last_activity = last_worker_activity_secs_with_transcript(
                    &activity_events,
                    &agent.id,
                    worker_cli,
                    transcript_path_for_worker.as_deref(),
                );
                // cas-d165 (Finding 1): reuse the SAME liveness evidence as
                // cas-7e85 / `cas factory is-wedged` — an outstanding tool
                // call (e.g. a dispatched research subagent) proves the
                // worker is actively waiting on real work, regardless of
                // checkpoint age. Before this fix, that evidence only fed
                // the director's WorkerStalled auto-nudge/escalation path
                // (director/events.rs); this human-facing `⚠ STALLED`
                // banner had NO in-flight input at all, so the two could
                // (and did, live: agile-puma-14) disagree at the same
                // instant — `is-wedged` says `in-flight tool call: true`
                // while this banner said `⚠ STALLED`. Do not invent a
                // second detector: call the identical
                // `wedged::transcript_has_in_flight_tool_call` primitive
                // against the same transcript path/cli resolution already
                // computed for `context_info` above.
                let in_flight_tool_call = transcript_path_for_worker
                    .as_deref()
                    .is_some_and(|p| {
                        crate::cli::factory::wedged::transcript_has_in_flight_tool_call(
                            p,
                            worker_cli,
                        )
                    });
                let stalled = is_worker_stalled(
                    has_in_progress_task,
                    last_activity.map(|(secs, _)| secs),
                    stall_threshold_secs,
                    in_flight_tool_call,
                );
                let activity_info = if !has_in_progress_task {
                    match last_activity {
                        Some((secs, phase)) => {
                            format!("\n    last activity: {secs}s ago ({phase})")
                        }
                        None => {
                            "\n    last activity: none in last 10m (may be investigating or idle)"
                                .to_string()
                        }
                    }
                } else if stalled {
                    match last_activity {
                        Some((secs, phase)) => format!(
                            "\n    last activity: {secs}s ago ({phase}) ⚠ STALLED (no activity ≥{stall_threshold_secs}s while task in progress)"
                        ),
                        None => "\n    ⚠ STALLED: no activity in last 10m while task in progress"
                            .to_string(),
                    }
                } else if in_flight_tool_call {
                    // Has an assignment, would otherwise read as stalled
                    // (old/absent checkpoint), but an in-flight tool call
                    // is direct evidence of real work in progress — never
                    // render the ambiguous "may be investigating or idle"
                    // hedge for a worker holding an assignment (AC3).
                    match last_activity {
                        Some((secs, phase)) => format!(
                            "\n    last activity: {secs}s ago ({phase}) — in-flight tool call (busy, not stalled)"
                        ),
                        None => "\n    in-flight tool call in progress (busy, not stalled — no checkpoint-class activity yet)"
                            .to_string(),
                    }
                } else {
                    match last_activity {
                        Some((secs, phase)) => {
                            format!("\n    last activity: {secs}s ago ({phase})")
                        }
                        None => {
                            // Unreachable in practice: has_in_progress_task
                            // && !in_flight_tool_call && last_activity==None
                            // always makes is_worker_stalled return true
                            // above (the None arm there stalls
                            // unconditionally absent in-flight evidence).
                            // Kept as a safe, non-hedging fallback in case
                            // that invariant ever changes.
                            "\n    last activity: none in last 10m (assigned task in progress — check in)"
                                .to_string()
                        }
                    }
                };
                output.push_str(&format!(
                    "  • {} (heartbeat: {}){}{}{}{}{}{}{}\n    session: {}\n",
                    &agent.name,
                    since,
                    liveness_label,
                    clone_info,
                    git_info,
                    transcript_info,
                    model_info,
                    context_info,
                    activity_info,
                    session_uuid
                ));
            }
        }

        // cas-2e81: died-while-leased section (empty-fleet vs crash distinction).
        output.push_str(&died_section);

        if stale_pruned > 0 {
            output.push_str(&format!(
                "\nFiltered stale agent record(s): {stale_pruned} (>{worker_stale_threshold_secs}s heartbeat age)\n"
            ));
        }

        Ok(Self::success(output))
    }

    pub(super) async fn factory_worker_activity(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_agent_store;
        use cas_store::{EventStore, SqliteEventStore};
        use cas_types::{AgentRole, AgentStatus, EventType};

        let event_store = SqliteEventStore::open(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open event store: {e}"),
            )
        })?;

        // Filter by worker name if specified, otherwise scope to this supervisor's workers.
        // `target` is accepted as a legacy alias for manual calls.
        let target_filter = req.worker_names.as_ref().or(req.target.as_ref());
        let requested_names = parse_worker_name_filter(target_filter);
        let owned = supervisor_owned_workers();
        let factory_session = current_factory_session();
        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;
        let mut visible_workers: Vec<_> = agent_store
            .list(None)
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list agents: {e}"),
                )
            })?
            .into_iter()
            .filter(|a| {
                a.role == AgentRole::Worker
                    && matches!(a.status, AgentStatus::Active | AgentStatus::Idle)
                    && a.visible_to_factory_session(factory_session.as_deref())
                    && owned.as_ref().is_none_or(|set| set.contains(&a.name))
            })
            .collect();

        if !requested_names.is_empty() {
            visible_workers
                .retain(|a| requested_names.contains(&a.name) || requested_names.contains(&a.id));
        }

        let visible_worker_ids: std::collections::HashSet<String> =
            visible_workers.iter().map(|a| a.id.clone()).collect();
        let visible_worker_names: std::collections::HashSet<String> =
            visible_workers.iter().map(|a| a.name.clone()).collect();

        // Get recent worker activity events
        let events = event_store.list_recent(50).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list events: {e}"),
            )
        })?;

        // Filter to worker activity events
        let worker_events: Vec<_> = events
            .into_iter()
            .filter(|e| {
                matches!(
                    e.event_type,
                    EventType::WorkerSubagentSpawned
                        | EventType::WorkerSubagentCompleted
                        | EventType::WorkerFileEdited
                        | EventType::WorkerGitCommit
                        | EventType::WorkerVerificationBlocked
                        | EventType::VerificationStarted
                        | EventType::VerificationAdded
                )
            })
            .filter(|e| {
                e.session_id
                    .as_ref()
                    .is_some_and(|id| visible_worker_ids.contains(id))
                    || visible_worker_ids.contains(&e.entity_id)
                    || visible_worker_names.contains(&e.entity_id)
            })
            .take(20)
            .collect();

        if worker_events.is_empty() {
            return Ok(Self::success(
                "No recent worker activity.\n\nWorker activity is tracked when workers edit files, run subagents, or commit code.",
            ));
        }

        let mut output = String::from("Worker Activity\n===============\n\n");
        for event in worker_events {
            let ago = format_relative_time(event.created_at);
            let session_short = event
                .session_id
                .as_ref()
                .map(|s| &s[..8.min(s.len())])
                .unwrap_or("unknown");
            output.push_str(&format!(
                "• {} - {} ({})\n",
                session_short, event.summary, ago
            ));
        }

        Ok(Self::success(output))
    }

    pub(super) async fn factory_clear_context(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_prompt_queue_store;

        let target = req.target.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "target required for clear_context",
            )
        })?;

        // Validate target is an owned worker when supervisor scoping applies
        if target != "all_workers" && target != "supervisor" {
            if let Some(owned) = supervisor_owned_workers() {
                if !owned.contains(&target) {
                    return Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "Worker '{}' not owned by this supervisor. Owned: {}",
                            target,
                            owned.into_iter().collect::<Vec<_>>().join(", ")
                        ),
                    ));
                }
            }
        }

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open message queue: {e}"),
            )
        })?;

        // Use the MCP caller's agent ID as the source
        let source = self
            .inner
            .get_agent_id()
            .unwrap_or_else(|_| "unknown".to_string());

        // Enqueue /clear directly without XML wrapping - this is a raw command
        let factory_session = current_factory_session();
        if let Some(ref session) = factory_session {
            queue
                .enqueue_with_session(&source, &target, "/clear", session)
                .map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to queue clear command: {e}"),
                    )
                })?;
        } else {
            queue.enqueue(&source, &target, "/clear").map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to queue clear command: {e}"),
                )
            })?;
        }

        let msg = if target == "all_workers" {
            "Queued /clear for all workers".to_string()
        } else {
            format!("Queued /clear for {target}")
        };

        Ok(Self::success(msg))
    }

    pub(super) async fn factory_my_context(
        &self,
        _req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_task_store};
        use cas_types::AgentRole;

        // Get current agent's info
        let agent_id = self.inner.get_agent_id().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to get agent ID: {e}"),
            )
        })?;

        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;

        let agent = agent_store.get(&agent_id).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to get agent: {e}"),
            )
        })?;

        let mut output = String::from("My Factory Context\n==================\n\n");

        // Agent info
        let role_str = match agent.role {
            AgentRole::Worker => "Worker",
            AgentRole::Supervisor => "Supervisor",
            AgentRole::Director => "Director",
            AgentRole::Standard => "Standard Agent",
        };
        output.push_str(&format!("**Name**: {}\n", agent.name));
        output.push_str(&format!("**Role**: {role_str}\n"));
        output.push_str(&format!("**ID**: {}\n\n", agent.id));

        // Clone path (from environment)
        if let Ok(cwd) = std::env::var("CAS_CLONE_PATH") {
            output.push_str(&format!("**Clone Path**: {cwd}\n"));
        } else if let Ok(cwd) = std::env::current_dir() {
            output.push_str(&format!("**Working Directory**: {}\n", cwd.display()));
        }

        // Current task(s)
        let leases = agent_store.list_agent_leases(&agent_id).unwrap_or_default();
        if leases.is_empty() {
            output.push_str("\n**Current Task**: None (idle)\n");
        } else {
            output.push_str("\n**Claimed Tasks**:\n");
            if let Ok(task_store) = open_task_store(&self.inner.cas_root) {
                for lease in &leases {
                    if let Ok(task) = task_store.get(&lease.task_id) {
                        output.push_str(&format!("  - {} {}\n", task.id, task.title));
                    } else {
                        output.push_str(&format!("  - {}\n", lease.task_id));
                    }
                }
            } else {
                for lease in &leases {
                    output.push_str(&format!("  - {}\n", lease.task_id));
                }
            }
        }

        // Git branch info
        if let Ok(branch_output) = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
        {
            if branch_output.status.success() {
                let branch = String::from_utf8_lossy(&branch_output.stdout)
                    .trim()
                    .to_string();
                output.push_str(&format!("\n**Git Branch**: {branch}\n"));
            }
        }

        Ok(Self::success(output))
    }

    pub(super) async fn factory_sync_all_workers(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_task_store};
        use cas_types::{AgentRole, AgentStatus, TaskStatus, TaskType};

        let store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;

        let owned = supervisor_owned_workers();
        let factory_session = current_factory_session();
        let mut workers: Vec<_> = store
            .list(Some(AgentStatus::Active))
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list agents: {e}"),
                )
            })?
            .into_iter()
            .filter(|a| {
                a.role == AgentRole::Worker
                    && a.visible_to_factory_session(factory_session.as_deref())
                    && owned.as_ref().is_none_or(|set| set.contains(&a.name))
            })
            .collect();

        if workers.is_empty() {
            return Ok(Self::success("No active workers found."));
        }

        if let Some(filter) = req.worker_names.as_ref() {
            let names: std::collections::HashSet<String> = filter
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            workers.retain(|w| names.contains(&w.name));
        }

        if workers.is_empty() {
            return Ok(Self::success(
                "No matching active workers found for requested worker_names filter.",
            ));
        }

        let sync_ref = if let Some(branch) = req.branch.clone().filter(|b| !b.trim().is_empty()) {
            branch
        } else {
            let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open task store: {e}"),
                )
            })?;

            let mut epic_branch = None;
            if let Ok(tasks) = task_store.list(None) {
                if let Some(epic) = tasks
                    .iter()
                    .find(|t| t.task_type == TaskType::Epic && t.status == TaskStatus::InProgress)
                {
                    epic_branch = epic.branch.clone();
                }
                if epic_branch.is_none() {
                    epic_branch = tasks
                        .iter()
                        .find(|t| t.task_type == TaskType::Epic && t.status == TaskStatus::Open)
                        .and_then(|t| t.branch.clone());
                }
            }
            epic_branch.unwrap_or_else(|| {
                // Use local main branch, not origin/main. In factory mode the
                // supervisor merges worker branches into the local main branch,
                // so workers should rebase onto it directly.
                use crate::worktree::GitOperations;
                GitOperations::detect_repo_root(&self.inner.cas_root)
                    .ok()
                    .map(GitOperations::new)
                    .map(|git| git.detect_default_branch())
                    .unwrap_or_else(|| "main".to_string())
            })
        };

        let mut synced = Vec::new();
        let mut skipped = Vec::new();
        let mut failed = Vec::new();

        for worker in workers {
            // cas-f53c: same path resolution as worker_status — do not require
            // clone_path metadata when the convention worktree already exists
            // (common race right after isolate spawn).
            let resolve = resolve_worker_clone_path(&self.inner.cas_root, &worker);
            if let Some(reason) = sync_skip_reason_for_clone_resolve(&worker.name, &resolve) {
                skipped.push(reason);
                continue;
            }
            let WorkerClonePathResolve::Ready(path) = resolve else {
                // Exhaustive: skip helper already covered NotOnDisk.
                continue;
            };

            match sync_worker_clone(&path, &sync_ref) {
                Ok(details) => synced.push(format!("{} ({})", worker.name, details)),
                Err(err) => failed.push(format!("{} ({})", worker.name, err)),
            }
        }

        let mut out =
            format!("Worker Sync Report\n==================\n\nSync target: {sync_ref}\n");
        if !synced.is_empty() {
            out.push_str("\nSynced:\n");
            for item in synced {
                out.push_str(&format!("  - {item}\n"));
            }
        }
        if !skipped.is_empty() {
            out.push_str("\nSkipped:\n");
            for item in skipped {
                out.push_str(&format!("  - {item}\n"));
            }
        }
        if !failed.is_empty() {
            out.push_str("\nFailed:\n");
            for item in failed {
                out.push_str(&format!("  - {item}\n"));
            }
        }

        Ok(Self::success(out))
    }

    pub(super) async fn factory_gc_report(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_prompt_queue_store, open_worktree_store};
        use cas_types::WorktreeStatus;
        use std::path::Path;

        let stale_after = req.older_than_secs.unwrap_or(120);
        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;
        let stale_agents = agent_store.list_stale(stale_after).unwrap_or_default();

        let prompt_queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {e}"),
            )
        })?;
        let pending_prompts = prompt_queue.pending_count().unwrap_or(0);

        let worktree_store = open_worktree_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open worktree store: {e}"),
            )
        })?;
        let active_worktrees = worktree_store
            .list_by_status(WorktreeStatus::Active)
            .unwrap_or_default();
        let orphan_worktrees: Vec<_> = active_worktrees
            .iter()
            .filter(|wt| !Path::new(&wt.path).exists())
            .collect();

        let mut out = String::from("Factory GC Report\n=================\n");
        out.push_str(&format!(
            "\nStale agent threshold: {}s\nStale agents: {}\nPending prompts: {}\nActive worktrees: {}\nOrphan worktrees: {}\n",
            stale_after,
            stale_agents.len(),
            pending_prompts,
            active_worktrees.len(),
            orphan_worktrees.len()
        ));

        if !stale_agents.is_empty() {
            out.push_str("\nStale agents:\n");
            for a in &stale_agents {
                out.push_str(&format!("  - {} ({})\n", a.name, a.id));
            }
        }
        if !orphan_worktrees.is_empty() {
            out.push_str("\nOrphan worktrees:\n");
            for wt in orphan_worktrees {
                out.push_str(&format!("  - {} ({})\n", wt.id, wt.path.display()));
            }
        }

        // Task cas-a9ab: surface uncommitted files in the main worktree as
        // "likely prior-factory WIP". Informational only — we never auto-delete.
        if let Some(summary) =
            crate::hooks::handlers::session_hygiene::wip_candidates(&self.inner.cas_root)
        {
            out.push_str(&format!(
                "\nMain worktree: {}\n",
                summary.worktree.display()
            ));
            if summary.is_clean() {
                out.push_str("Prior-factory WIP candidates: none (worktree clean)\n");
            } else {
                out.push_str(&format!(
                    "Prior-factory WIP candidates: {} ({} untracked, {} modified)\n",
                    summary.entries.len(),
                    summary.untracked_count(),
                    summary.modified_count(),
                ));
                for entry in &summary.entries {
                    out.push_str(&format!(
                        "  [{}] {} {}\n",
                        entry.label(),
                        entry.status,
                        entry.path,
                    ));
                }
                out.push_str(
                    "\nNote: these are not auto-deleted. Inspect, then commit/salvage/discard.\n",
                );
            }
        }

        Ok(Self::success(out))
    }

    /// cas-8f8f: read-only diagnostic that walks an epic's children
    /// and reports per-worker `factory/<assignee>` merge state vs.
    /// the epic's parent branch. Mirrors the `factory_gc_report`
    /// pattern: pure read, returns markdown via `Self::success`.
    ///
    /// Uses the same `count_unmerged_factory_commits` /
    /// `last_commit_unix` helpers that back the close-time gates in
    /// `close_ops.rs`, so the report can never disagree with what
    /// the gate actually enforces.
    pub(super) async fn factory_epic_status(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::core::task::lifecycle::close_ops::{
            collect_epic_branch_statuses, render_epic_status_report,
        };
        use crate::store::open_task_store;
        use cas_types::TaskType;

        let epic_id = req.id.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "epic_status requires `id`: mcp__cas__coordination action=epic_status id=<epic-id>",
            )
        })?;

        let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open task store: {e}"),
            )
        })?;

        let epic = task_store.get(epic_id).map_err(|e| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Task not found: {epic_id}: {e}"),
            )
        })?;

        if epic.task_type != TaskType::Epic {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "epic_status: task {epic_id} is not an Epic (task_type={:?}). \
                     This action only operates on Epic-type tasks.",
                    epic.task_type
                ),
            ));
        }

        // The parent branch the gate compares against: the epic's
        // own `branch` field (set by epic creation), falling back to
        // "master" to match the epic-close path's existing default.
        let parent_branch = epic.branch.as_deref().unwrap_or("master");

        let subtasks = task_store.get_subtasks(epic_id).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to walk subtasks of {epic_id}: {e}"),
            )
        })?;

        let close_project_root = self.inner.cas_root.parent().unwrap_or(&self.inner.cas_root);
        let statuses = collect_epic_branch_statuses(&subtasks, parent_branch, close_project_root);
        let report = render_epic_status_report(epic_id, parent_branch, &statuses);

        Ok(Self::success(report))
    }

    pub(super) async fn factory_focus_epic(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_task_store;
        use crate::ui::factory::{metadata_path, persist_session_metadata_pinned_epic_id_at};
        use cas_types::{TaskStatus, TaskType};

        let factory_session = current_factory_session().ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                "focus_epic requires an active factory session (CAS_FACTORY_SESSION is not set)",
            )
        })?;

        let clear = req.clear.unwrap_or(false);
        let epic_id = req.id.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let metadata_path = metadata_path(&factory_session);

        if clear {
            persist_session_metadata_pinned_epic_id_at(&metadata_path, None).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to clear pinned epic focus: {e}"),
                )
            })?;
            self.record_focus_epic_event(&factory_session, None);
            return Ok(Self::success(format!(
                "Cleared pinned epic focus for factory session {factory_session}"
            )));
        }

        let Some(epic_id) = epic_id else {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "focus_epic requires `id=<epic-id>` or `clear=true`",
            ));
        };

        let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open task store: {e}"),
            )
        })?;

        let epic = task_store.get(epic_id).map_err(|e| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Task not found: {epic_id}: {e}"),
            )
        })?;

        if epic.task_type != TaskType::Epic {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "focus_epic: task {epic_id} is not an Epic (task_type={:?}). \
                     This action only operates on Epic-type tasks.",
                    epic.task_type
                ),
            ));
        }
        if epic.status == TaskStatus::Closed {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "focus_epic: task {epic_id} is Closed. \
                     Closed epics cannot be pinned as the active factory focus.",
                ),
            ));
        }

        persist_session_metadata_pinned_epic_id_at(&metadata_path, Some(epic_id)).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to persist pinned epic focus: {e}"),
            )
        })?;
        self.record_focus_epic_event(&factory_session, Some(epic_id));

        Ok(Self::success(format!(
            "Pinned epic focus to {epic_id} for factory session {factory_session}"
        )))
    }

    fn record_focus_epic_event(&self, factory_session: &str, epic_id: Option<&str>) {
        use crate::store::open_event_store;
        use cas_types::{Event, EventEntityType, EventType};

        let Ok(event_store) = open_event_store(&self.inner.cas_root) else {
            return;
        };

        let summary = match epic_id {
            Some(epic_id) => format!("Pinned factory epic focus to {epic_id}"),
            None => "Cleared factory epic focus pin".to_string(),
        };
        let entity_type = if epic_id.is_some() {
            EventEntityType::Task
        } else {
            EventEntityType::Session
        };
        let entity_id = epic_id.unwrap_or(factory_session);
        let metadata = serde_json::json!({
            "factory_session": factory_session,
            "epic_id": epic_id,
        });
        let event = Event::new(
            EventType::SupervisorInjected,
            entity_type,
            entity_id,
            summary,
        )
        .with_metadata(metadata)
        .with_session(factory_session);
        let _ = event_store.record(&event);
    }

    pub(super) async fn factory_gc_cleanup(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::{open_agent_store, open_prompt_queue_store, open_worktree_store};
        use cas_types::{AgentRole, AgentStatus, WorktreeStatus};
        use std::path::Path;

        let stale_after = req.older_than_secs.unwrap_or(120);
        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;
        let stale_agents = agent_store.list_stale(stale_after).unwrap_or_default();
        let mut stale_marked = 0usize;
        for agent in stale_agents {
            // Don't let workers prune supervisors/directors
            if agent.role == AgentRole::Supervisor || agent.role == AgentRole::Director {
                continue;
            }
            if agent_store.mark_stale(&agent.id).is_ok() {
                stale_marked += 1;
            }
        }

        let mut dead_agent_records_purged = 0usize;
        for status in [AgentStatus::Stale, AgentStatus::Shutdown] {
            for agent in agent_store.list(Some(status)).unwrap_or_default() {
                if agent.role == AgentRole::Supervisor || agent.role == AgentRole::Director {
                    continue;
                }
                if agent_store.unregister(&agent.id).is_ok() {
                    dead_agent_records_purged += 1;
                }
            }
        }

        let worktree_store = open_worktree_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open worktree store: {e}"),
            )
        })?;
        let active_worktrees = worktree_store
            .list_by_status(WorktreeStatus::Active)
            .unwrap_or_default();
        let mut orphan_marked_removed = 0usize;
        for mut wt in active_worktrees {
            if !Path::new(&wt.path).exists() {
                wt.mark_removed();
                if worktree_store.update(&wt).is_ok() {
                    orphan_marked_removed += 1;
                }
            }
        }

        // Clear prompt queue only when explicitly forced.
        let mut cleared_prompts = 0usize;
        if req.force.unwrap_or(false) {
            let prompt_queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open prompt queue: {e}"),
                )
            })?;
            cleared_prompts = prompt_queue.clear().unwrap_or(0);
        }

        const SKILL_MARKER_MAX_AGE: std::time::Duration =
            std::time::Duration::from_secs(30 * 24 * 60 * 60);
        let stale_skill_markers_removed =
            cleanup_stale_skill_markers(&self.inner.cas_root, SKILL_MARKER_MAX_AGE);

        Ok(Self::success(format!(
            "Factory GC cleanup complete.\n\nStale agents marked: {stale_marked}\nDead agent records purged: {dead_agent_records_purged}\nOrphan worktrees marked removed: {orphan_marked_removed}\nPrompt queue entries cleared: {cleared_prompts}\nStale skill markers removed: {stale_skill_markers_removed}"
        )))
    }
}

fn cleanup_stale_skill_markers(cas_root: &std::path::Path, max_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(cas_root) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("session_skills_seen_"))
        })
        .filter(|entry| {
            let invalid_empty_suffix = entry.file_name() == "session_skills_seen_";
            let stale = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > max_age);
            (invalid_empty_suffix || stale) && std::fs::remove_file(entry.path()).is_ok()
        })
        .count()
}

/// Returns the set of worker names this supervisor owns, derived from the `CAS_FACTORY_WORKER_NAMES`
/// environment variable. Returns `None` when not running as a supervisor or when the variable is
/// absent, meaning no scoping should be applied.
fn supervisor_owned_workers() -> Option<std::collections::HashSet<String>> {
    let role = std::env::var("CAS_AGENT_ROLE").unwrap_or_default();
    if !role.eq_ignore_ascii_case("supervisor") {
        return None;
    }
    let csv = std::env::var("CAS_FACTORY_WORKER_NAMES").ok()?;
    if csv.trim().is_empty() {
        return None;
    }
    Some(
        csv.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerWorktreeStatus {
    clone_path: Option<String>,
    clone_info: String,
    git_info: String,
}

/// cas-f53c: shared resolution for worker clone/worktree path used by both
/// `worker_status` and `sync_all_workers`.
///
/// Priority when a path exists on disk:
/// 1. `agent.metadata["clone_path"]` if present and exists
/// 2. Convention path `{cas_root}/worktrees/{worker_name}` if it exists
///
/// When nothing is on disk, returns `NotOnDisk` with the best candidate path
/// for messaging (metadata if set, else convention). Sync treats that as a
/// **retryable** skip (registration/provisioning lag), not a success.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkerClonePathResolve {
    /// Worktree path exists and is ready for git ops / status.
    Ready(std::path::PathBuf),
    /// No worktree on disk at metadata or convention path.
    NotOnDisk {
        candidate: std::path::PathBuf,
        /// True when `clone_path` metadata was set (even if the path is missing).
        had_metadata: bool,
    },
}

fn resolve_worker_clone_path(
    cas_root: &std::path::Path,
    agent: &cas_types::Agent,
) -> WorkerClonePathResolve {
    let metadata_path = agent
        .metadata
        .get("clone_path")
        .map(|s| std::path::PathBuf::from(s));
    let convention_path = cas_root.join("worktrees").join(&agent.name);

    if let Some(ref meta) = metadata_path {
        if meta.exists() {
            return WorkerClonePathResolve::Ready(meta.clone());
        }
    }
    if convention_path.exists() {
        return WorkerClonePathResolve::Ready(convention_path);
    }

    WorkerClonePathResolve::NotOnDisk {
        candidate: metadata_path.unwrap_or(convention_path),
        had_metadata: agent.metadata.contains_key("clone_path"),
    }
}

/// Human-readable skip reason for `sync_all_workers` when no worktree is ready.
/// Pure for unit testing (cas-f53c).
fn sync_skip_reason_for_clone_resolve(
    worker_name: &str,
    resolve: &WorkerClonePathResolve,
) -> Option<String> {
    match resolve {
        WorkerClonePathResolve::Ready(_) => None,
        WorkerClonePathResolve::NotOnDisk {
            candidate,
            had_metadata: false,
        } => Some(format!(
            "{worker_name} (registration in progress or no worktree — retry sync after \
             isolate spawn completes; expected path: {})",
            candidate.display()
        )),
        WorkerClonePathResolve::NotOnDisk {
            candidate,
            had_metadata: true,
        } => Some(format!(
            "{worker_name} (clone path not found: {} — retry if spawn still provisioning)",
            candidate.display()
        )),
    }
}

fn collect_worker_worktree_status(
    cas_root: &std::path::Path,
    agent: &cas_types::Agent,
) -> WorkerWorktreeStatus {
    match resolve_worker_clone_path(cas_root, agent) {
        WorkerClonePathResolve::Ready(path) => {
            let clone_path = path.display().to_string();
            let gs = collect_worker_git_status(&path);
            WorkerWorktreeStatus {
                clone_info: format!("\n    Clone: {clone_path}"),
                git_info: format_worker_git_status(&gs),
                clone_path: Some(clone_path),
            }
        }
        WorkerClonePathResolve::NotOnDisk { candidate, .. } => {
            let missing_path = candidate.display().to_string();
            WorkerWorktreeStatus {
                clone_info: format!("\n    Clone: {missing_path} [missing-worktree]"),
                git_info: "\n    git: missing-worktree".to_string(),
                clone_path: Some(missing_path),
            }
        }
    }
}

fn run_git(path: &std::path::Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|e| format!("git {} failed to start: {}", args.join(" "), e))?;

    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn sync_worker_clone(
    path: &std::path::Path,
    sync_ref: &str,
) -> std::result::Result<String, String> {
    let status = run_git(path, &["status", "--porcelain"])?;
    let mut stashed = false;

    if !status.trim().is_empty() {
        let stash_msg = format!(
            "cas-factory-auto-sync {}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
        );
        let stash_out = run_git(
            path,
            &["stash", "push", "--include-untracked", "-m", &stash_msg],
        )?;
        if !stash_out.contains("No local changes") {
            stashed = true;
        }
    }

    let _ = run_git(path, &["fetch", "origin"]);

    if let Err(rebase_err) = run_git(path, &["rebase", sync_ref]) {
        let _ = run_git(path, &["rebase", "--abort"]);
        if stashed {
            let _ = run_git(path, &["stash", "pop"]);
        }
        return Err(format!("rebase failed: {rebase_err}"));
    }

    if stashed {
        run_git(path, &["stash", "pop"])
            .map_err(|e| format!("sync applied but stash pop failed: {e}"))?;
    }

    Ok(if stashed {
        "stashed + rebased + restored".to_string()
    } else {
        "rebased cleanly".to_string()
    })
}

/// Format a timestamp as relative time (e.g., "2s ago", "5m ago")
fn format_relative_time(dt: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(dt);

    if diff.num_seconds() < 0 {
        return "just now".to_string();
    }

    if diff.num_seconds() < 60 {
        return format!("{}s ago", diff.num_seconds());
    }

    if diff.num_minutes() < 60 {
        return format!("{}m ago", diff.num_minutes());
    }

    if diff.num_hours() < 24 {
        return format!("{}h ago", diff.num_hours());
    }

    format!("{}d ago", diff.num_days())
}

/// cas-1ec7: check whether the event log shows recent I/O progress
/// (file edits or git commits) for the given agent session ID.
///
/// Used in `factory_worker_status` to suppress stale-detection for workers
/// that are in long CPU-bound operations (e.g., `cargo build`/`cargo test`
/// stretches that extend past the 30s heartbeat threshold). Heartbeat lag
/// alone is insufficient evidence of death when observable I/O is
/// progressing within the same window.
///
/// `events` should be pre-filtered to the stale time window (caller queries
/// `list_since(now - WORKER_STALE_SECS, …)` once and passes the slice here
/// to avoid one DB round-trip per agent).
///
/// Matching is done by `session_id == agent_id` (the CC session UUID, set
/// identically in `agent.id` and `event.session_id` since daemon.rs stores
/// both from the same session_id field).
pub(crate) fn has_recent_worker_io_activity(events: &[cas_types::Event], agent_id: &str) -> bool {
    use cas_types::EventType;
    events.iter().any(|e| {
        matches!(
            e.event_type,
            EventType::WorkerFileEdited | EventType::WorkerGitCommit
        ) && e.session_id.as_deref() == Some(agent_id)
    })
}

// Process-alive probes live in `agent_liveness` (cas-e98e). Re-export so
// existing call sites and tests keep compiling.
pub(crate) use super::agent_liveness::agent_process_is_alive;

/// Return the age (seconds) and a short phase label of the worker's most recent
/// observable activity event (cas-86c5).
///
/// Scans `events` for any event whose `session_id == agent_id` and returns
/// `Some((elapsed_secs, phase))` for the freshest match, where `phase` is a
/// short human-readable label:
///
/// | EventType           | phase label |
/// |---------------------|-------------|
/// | WorkerFileEdited    | "editing"   |
/// | WorkerGitCommit     | "checkpoint"|
/// | WorkerSubagentSpawned / Completed | "spawning subagent" |
/// | anything else       | "activity"  |
///
/// Returns `None` when no matching event exists in the provided slice (which
/// the caller is expected to pre-filter to a reasonable recency window).
///
/// This is used in `factory_worker_status` to surface "last activity: Xs ago
/// (phase)" so a supervisor can distinguish a worker that is actively
/// investigating (no edits yet, but fresh checkpoint events) from one that is
/// truly stalled (no events of any kind for minutes).
pub(crate) fn last_worker_activity_secs(
    events: &[cas_types::Event],
    agent_id: &str,
) -> Option<(i64, &'static str)> {
    use cas_types::EventType;
    events
        .iter()
        .filter(|e| e.session_id.as_deref() == Some(agent_id) || e.entity_id == agent_id)
        .map(|e| {
            let elapsed = (chrono::Utc::now() - e.created_at).num_seconds().max(0);
            let phase: &'static str = match e.event_type {
                EventType::WorkerFileEdited => "editing",
                EventType::WorkerGitCommit => "checkpoint",
                EventType::WorkerSubagentSpawned | EventType::WorkerSubagentCompleted => {
                    "spawning subagent"
                }
                _ => "activity",
            };
            (elapsed, phase)
        })
        .min_by_key(|(elapsed, _)| *elapsed)
}

/// cas-a653 / cas-c2c2: fold transcript-freshness activity into
/// `last_worker_activity_secs` for **every** harness.
///
/// HISTORY: cas-a653 originally gated this on
/// `!HarnessCapabilities::supports_hooks` (Codex only), reasoning that
/// hook-capable harnesses (Claude, Grok) always get a CAS event recorded
/// for their real work. That reasoning was falsified live, on this
/// factory, on the shipped binary: Claude worker `interrupt-fixer` was
/// reported `last activity: 401s ago ⚠ STALLED` at 2026-07-27T21:29:20Z
/// while its transcript's last record was 21:29:18.757Z — two seconds
/// earlier — with tool calls in every single minute since 21:22 (cas-c2c2).
/// The real defect was never "Codex has no hooks"; it's that CAS's
/// **event store** only gets a row for the specific tool-use classes its
/// hooks are wired to translate into `WorkerFileEdited`/`WorkerGitCommit`/
/// subagent events (see `hooks::handlers`). A worker whose whole stretch of
/// work is e.g. `Bash`-driving a nested TUI — as `interrupt-fixer` was,
/// reproducing cas-4208 — produces tool calls the transcript records in
/// real time but that never touch the event store at all, hook support
/// notwithstanding. So the freeze is generic, not Codex-specific.
///
/// Fix: reuse the SAME primitive `cas factory is-wedged` / the director's
/// stall gate already trust — `wedged::effective_transcript_age`, which is
/// itself already harness-aware (Grok's `signals.json`-preferring path vs
/// Claude/Codex's plain transcript mtime, see `wedged::grok_activity_age`)
/// — against the worker's own resolved transcript/rollout path, and take
/// whichever of (event-store age, transcript age) is FRESHER (smaller).
/// Applying this universally, rather than re-gating on harness, is what
/// AC#2 requires this time; the harness dispatch now lives entirely inside
/// `effective_transcript_age` (AC#6 — Claude/Grok/Codex freshness-window
/// differences stay exactly where `wedged.rs` already defines them, this
/// function never re-derives or flattens them).
///
/// `None` in, `None` out when neither signal resolves — this never invents
/// an activity age from nothing.
pub(crate) fn last_worker_activity_secs_with_transcript(
    events: &[cas_types::Event],
    agent_id: &str,
    cli: cas_mux::SupervisorCli,
    transcript_path: Option<&std::path::Path>,
) -> Option<(i64, &'static str)> {
    let event_based = last_worker_activity_secs(events, agent_id);
    let Some(path) = transcript_path else {
        return event_based;
    };
    let Some(transcript_age) = crate::cli::factory::wedged::effective_transcript_age(path, cli)
    else {
        return event_based;
    };
    let transcript_secs = transcript_age.as_secs() as i64;
    match event_based {
        Some((secs, phase)) if secs <= transcript_secs => Some((secs, phase)),
        _ => Some((transcript_secs, "activity")),
    }
}

/// cas-9829: whether a `worker_status` row should render the `⚠ STALLED`
/// marker instead of the soft "may be investigating or idle" hedge.
///
/// True when the worker has an in-progress task (a lease and/or a real
/// task assignment — see the `has_in_progress_task` computation in
/// `factory_worker_status`, cas-d165 Finding 2) AND there is no in-flight
/// tool call (cas-d165 Finding 1) AND either:
/// - its last observable activity is at/past `stall_threshold_secs`, or
/// - no activity was observed at all within the query window (`None`).
///
/// A worker with no in-progress task is never "stalled" in this sense —
/// idle-with-no-task is a distinct, already-signaled state (`WorkerIdle`).
///
/// `in_flight_tool_call` is the SAME evidence cas-7e85 / `cas factory
/// is-wedged` consume (`wedged::transcript_has_in_flight_tool_call`) —
/// checked first and short-circuits to "not stalled" unconditionally,
/// mirroring `transcript_confirms_stall_for_age`'s AC1 in
/// `director/events.rs`. Before this parameter existed, this
/// human-facing banner had no in-flight input at all and could render
/// `⚠ STALLED` for a worker `cas factory is-wedged` simultaneously
/// reported as `in-flight tool call: true` — two disagreeing notions of
/// "not working" for the same worker at the same instant.
fn is_worker_stalled(
    has_in_progress_task: bool,
    last_activity_secs_ago: Option<i64>,
    stall_threshold_secs: i64,
    in_flight_tool_call: bool,
) -> bool {
    if !has_in_progress_task || in_flight_tool_call {
        return false;
    }
    match last_activity_secs_ago {
        Some(secs) => secs >= stall_threshold_secs,
        None => true,
    }
}

/// cas-8240 two-band liveness label for `factory_worker_status`:
///
/// * `elapsed >= WORKER_DEAD_SECS` → `" [DEAD]"` (hard escalation —
///   caller also surfaces the transcript path for salvage).
/// * `WORKER_STALE_SECS <= elapsed < WORKER_DEAD_SECS` → `" [stale]"`
///   (grace-window indicator — the worker slipped past the prune
///   without being `mark_stale`'d, but it's too early to declare it
///   dead).
/// * Otherwise → `""` (no label).
///
/// Leading space is intentional: the caller concatenates the returned
/// slice directly after the `heartbeat: <Xs ago>` segment, and an empty
/// string avoids a trailing space when the worker is fresh. Returning
/// `&'static str` keeps this allocation-free.
fn liveness_label_for(elapsed_secs: i64) -> &'static str {
    if elapsed_secs >= WORKER_DEAD_SECS {
        " [DEAD]"
    } else if elapsed_secs >= WORKER_STALE_SECS {
        " [stale]"
    } else {
        ""
    }
}

/// Resolution outcome for a worker's Claude Code transcript file
/// (cas-900b — replaces the brittle reconstruct-only `derive_transcript_path`).
///
/// Claude Code persists each session's JSONL under
/// `~/.claude/projects/<escaped-cwd>/<session-id>.jsonl`. Session IDs are
/// stable UUIDs unique across all projects, so we can glob
/// `~/.claude/projects/*/<session-id>.jsonl` and surface whichever real path
/// actually exists — rather than reconstructing the `<escaped-cwd>` from the
/// worker's clone_path, which was observed from a single field sample and
/// breaks on spaces, unicode, colons, and any future CC escape change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptResolution {
    /// Exactly one `~/.claude/projects/*/<session-id>.jsonl` matched.
    /// The real on-disk path ready for the supervisor to open.
    Resolved(std::path::PathBuf),
    /// Zero matches. Could be: CC never wrote a transcript, the home dir
    /// lookup failed, or the worker died before SessionStart. Carries the
    /// reconstructed (legacy) path, labelled "likely" at the call site.
    Synthesized(String),
    /// More than one match — should be rare (session_id collisions or a
    /// user who manually copied transcripts between projects). Surface
    /// all candidates so the supervisor can pick. `truncated` is true
    /// when the glob walk hit `MAX_TRANSCRIPT_CANDIDATES`.
    Ambiguous {
        matches: Vec<std::path::PathBuf>,
        synthesized: String,
        truncated: bool,
    },
}

/// Reconstruct the legacy `<escaped-cwd>` path. Used as the fallback in the
/// `Synthesized` and `Ambiguous` branches of `TranscriptResolution`; also
/// kept for tests that want to pin the historical escape semantics.
///
/// Observed in the wild: both `/` and `.` collapse to `-`. Underscores and
/// other characters are preserved. Example:
/// `/home/a/.cas/worktrees/x` → `-home-a--cas-worktrees-x`.
fn synthesized_transcript_path(clone_path: &str, session_id: &str) -> String {
    let escaped: String = clone_path
        .chars()
        .map(|c| match c {
            '/' | '.' => '-',
            other => other,
        })
        .collect();
    format!("~/.claude/projects/{escaped}/{session_id}.jsonl")
}

/// Hard cap on glob candidate collection to bound the worst-case
/// `worker_status` latency (adversarial cas-900b P1). On a long-lived
/// host `~/.claude/projects/` can accumulate thousands of transcripts;
/// listing more than 50 for a single worker isn't useful anyway — the
/// supervisor needs to pick one, not read a thousand paths. If the cap
/// is ever hit the output notes the truncation so the supervisor knows
/// to grep manually.
const MAX_TRANSCRIPT_CANDIDATES: usize = 50;

/// Glob `<projects_dir>/*/<session-id>.jsonl` and return up to
/// `MAX_TRANSCRIPT_CANDIDATES` matches plus a `truncated` flag.
///
/// - `session_id` is glob-escaped before interpolation (adversarial
///   cas-900b P1): an agent that registers with a malicious
///   `session_id = "*"` must not broaden the search and leak every
///   transcript on the host into `worker_status` output.
/// - Malformed glob patterns and I/O errors collapse to an empty vec;
///   the caller's fallback path preserves supervisor agency.
fn glob_transcript_candidates(
    projects_dir: &std::path::Path,
    session_id: &str,
) -> (Vec<std::path::PathBuf>, bool) {
    let escaped_session = glob::Pattern::escape(session_id);
    let pattern = format!(
        "{}/*/{}.jsonl",
        projects_dir.to_string_lossy(),
        escaped_session
    );
    let iter = match glob::glob(&pattern) {
        Ok(it) => it,
        Err(_) => return (Vec::new(), false),
    };
    let mut out = Vec::new();
    let mut truncated = false;
    for result in iter {
        if let Ok(p) = result {
            if out.len() >= MAX_TRANSCRIPT_CANDIDATES {
                truncated = true;
                break;
            }
            out.push(p);
        }
    }
    (out, truncated)
}

/// Resolve the transcript location for a worker, dispatching by harness
/// (cas-058f, EPIC cas-8888 Phase 4). Claude (and Codex, which has no
/// dedicated transcript reader of its own and simply resolves to
/// `Synthesized`/never-matches — see module docs) use
/// `~/.claude/projects/*/<session-id>.jsonl`. Grok uses a structurally
/// different, directory-based layout — see [`resolve_grok_transcript`].
///
/// `base_dir` is the harness-appropriate root (`~/.claude/projects` or
/// `~/.grok/sessions`); callers pick it via [`default_claude_projects_dir`]
/// / [`default_grok_sessions_dir`] (or inject a temp dir in tests).
///
/// `clone_path == None` means the worker registered without cwd metadata;
/// the `Synthesized` / `Ambiguous` fallback paths omit the reconstructed
/// legacy escape in that case (there's nothing to reconstruct from), and
/// the caller must label the output accordingly.
pub(crate) fn resolve_transcript(
    base_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> TranscriptResolution {
    match cli {
        cas_mux::SupervisorCli::Grok => resolve_grok_transcript(base_dir, clone_path, session_id),
        cas_mux::SupervisorCli::Codex => resolve_codex_transcript(base_dir, clone_path, session_id),
        cas_mux::SupervisorCli::Claude => {
            resolve_claude_transcript(base_dir, clone_path, session_id)
        }
    }
}

fn resolve_claude_transcript(
    projects_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
) -> TranscriptResolution {
    let synthesized = clone_path.map(|p| synthesized_transcript_path(p, session_id));
    let Some(projects) = projects_dir else {
        return TranscriptResolution::Synthesized(
            synthesized.unwrap_or_else(|| synthesized_unknown_clone_path(session_id)),
        );
    };
    let (mut matches, truncated) = glob_transcript_candidates(projects, session_id);
    match matches.len() {
        0 => TranscriptResolution::Synthesized(
            synthesized.unwrap_or_else(|| synthesized_unknown_clone_path(session_id)),
        ),
        1 => TranscriptResolution::Resolved(matches.remove(0)),
        _ => TranscriptResolution::Ambiguous {
            matches,
            synthesized: synthesized.unwrap_or_else(|| synthesized_unknown_clone_path(session_id)),
            truncated,
        },
    }
}

/// `~/.claude/projects` — Claude Code's per-user transcript root.
/// Returns `None` if the user's home dir isn't resolvable.
pub(crate) fn default_claude_projects_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Placeholder synthesized path used when clone_path metadata is absent.
/// The label explicitly names the missing input so a supervisor reading
/// the output sees *why* the synthesized path is a placeholder (adversarial
/// cas-900b P3: don't conflate clone_path-absent with home_dir-absent).
fn synthesized_unknown_clone_path(session_id: &str) -> String {
    format!("~/.claude/projects/<cwd>/{session_id}.jsonl (clone path unknown)")
}

// ---------------------------------------------------------------------------
// cas-058f (EPIC cas-8888 Phase 4): Grok transcript resolution.
//
// xAI Grok Build's on-disk layout is structurally different from Claude's —
// this is net-new resolution logic, not a match-arm addition:
//   Claude: ~/.claude/projects/<escaped-cwd '/'&'.'→'-'>/<session-id>.jsonl
//   Grok:   ~/.grok/sessions/<URL-encoded-cwd>/<session-uuid>/{
//             updates.jsonl   (authoritative ACP log — the "transcript"),
//             chat_history.jsonl,
//             signals.json    (token/turn counters — a better activity
//                              signal than raw mtime, see classify_worker)
//           }
// `session_id` is a directory component here, not a filename stem, and the
// cwd is URL-encoded rather than collapsed to `-`. `GROK_HOME` overrides the
// base directory (mirrors Grok's own env-var convention). A cwd longer than
// 255 bytes gets a slug+hash directory name instead (with the original cwd
// stashed in a `.cwd` file) — that hash scheme isn't reverse-engineered, so
// long cwds fall back to the un-truncated encoding for the *synthesized*
// (best-guess) path and rely on the glob to find the real directory anyway.
// ---------------------------------------------------------------------------

/// `~/.grok/sessions` — xAI Grok Build's per-user transcript root.
/// `GROK_HOME` overrides the base directory when set; falls back to the
/// user's real home dir. Returns `None` if neither resolves.
pub(crate) fn default_grok_sessions_dir() -> Option<std::path::PathBuf> {
    if let Ok(grok_home) = std::env::var("GROK_HOME") {
        return Some(std::path::PathBuf::from(grok_home).join("sessions"));
    }
    dirs::home_dir().map(|h| h.join(".grok").join("sessions"))
}

/// Reconstruct the legacy Grok transcript path:
/// `~/.grok/sessions/<url-encoded-cwd>/<session-uuid>/updates.jsonl`.
fn synthesized_grok_transcript_path(clone_path: &str, session_id: &str) -> String {
    let encoded = urlencoding::encode(clone_path);
    format!("~/.grok/sessions/{encoded}/{session_id}/updates.jsonl")
}

fn synthesized_unknown_grok_clone_path(session_id: &str) -> String {
    format!("~/.grok/sessions/<cwd>/{session_id}/updates.jsonl (clone path unknown)")
}

/// Glob `<sessions_dir>/*/<session-uuid>/updates.jsonl` — Grok's session id
/// is a directory component, not a filename stem (unlike Claude's
/// `<session-id>.jsonl`). Same glob-escaping and candidate cap as
/// [`glob_transcript_candidates`], for the same reasons.
fn glob_grok_transcript_candidates(
    sessions_dir: &std::path::Path,
    session_id: &str,
) -> (Vec<std::path::PathBuf>, bool) {
    let escaped_session = glob::Pattern::escape(session_id);
    let pattern = format!(
        "{}/*/{}/updates.jsonl",
        sessions_dir.to_string_lossy(),
        escaped_session
    );
    let iter = match glob::glob(&pattern) {
        Ok(it) => it,
        Err(_) => return (Vec::new(), false),
    };
    let mut out = Vec::new();
    let mut truncated = false;
    for result in iter.flatten() {
        if out.len() >= MAX_TRANSCRIPT_CANDIDATES {
            truncated = true;
            break;
        }
        out.push(result);
    }
    (out, truncated)
}

fn resolve_grok_transcript(
    sessions_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
) -> TranscriptResolution {
    let synthesized = clone_path.map(|p| synthesized_grok_transcript_path(p, session_id));
    let Some(dir) = sessions_dir else {
        return TranscriptResolution::Synthesized(
            synthesized.unwrap_or_else(|| synthesized_unknown_grok_clone_path(session_id)),
        );
    };
    let (mut matches, truncated) = glob_grok_transcript_candidates(dir, session_id);
    match matches.len() {
        0 => TranscriptResolution::Synthesized(
            synthesized.unwrap_or_else(|| synthesized_unknown_grok_clone_path(session_id)),
        ),
        1 => TranscriptResolution::Resolved(matches.remove(0)),
        _ => TranscriptResolution::Ambiguous {
            matches,
            synthesized: synthesized.unwrap_or_else(|| synthesized_unknown_grok_clone_path(session_id)),
            truncated,
        },
    }
}

// ---------------------------------------------------------------------------
// cas-c655: Codex rollout resolution.
//
// Codex does NOT use Claude's `~/.claude/projects/<escaped-cwd>/<session>.jsonl`
// layout. Factory workers get a CAS session id of the form
// `codex-<name>-<uuid>` (see `PtyConfig::codex`), but on-disk rollouts live at:
//   ~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<rollout-uuid>.jsonl
// with `session_meta.payload.cwd` equal to the worker's clone_path and a
// different rollout UUID than the CAS session id. Matching is therefore by
// cwd (primary) and by rollout UUID substring in the filename (secondary —
// useful only when the caller already knows the rollout id).
// ---------------------------------------------------------------------------

/// `~/.codex/sessions` — Codex CLI's per-user rollout root.
/// `CODEX_HOME` overrides the base directory when set (mirrors Codex's own
/// env-var convention). Returns `None` if neither resolves.
pub(crate) fn default_codex_sessions_dir() -> Option<std::path::PathBuf> {
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        return Some(std::path::PathBuf::from(codex_home).join("sessions"));
    }
    dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
}

fn synthesized_codex_transcript_path(clone_path: &str, session_id: &str) -> String {
    format!(
        "~/.codex/sessions/<YYYY/MM/DD>/rollout-*-*.jsonl (cwd={clone_path}; cas_session={session_id})"
    )
}

fn synthesized_unknown_codex_clone_path(session_id: &str) -> String {
    format!(
        "~/.codex/sessions/<YYYY/MM/DD>/rollout-*-*.jsonl (clone path unknown; cas_session={session_id})"
    )
}

#[derive(Debug, Default)]
struct CodexRolloutMetadata {
    cwd: Option<String>,
    originator: Option<String>,
    source: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum CodexRolloutKind {
    InteractiveCli,
    Exec,
    Other,
}

impl CodexRolloutMetadata {
    fn kind(&self) -> CodexRolloutKind {
        // `source` is Codex's serialized SessionSource enum and is the
        // strongest signal. `originator` covers older rollouts where source
        // was absent and independently confirms today's cli/exec values.
        match self.source.as_deref() {
            Some(source) if source.eq_ignore_ascii_case("exec") => CodexRolloutKind::Exec,
            Some(source) if source.eq_ignore_ascii_case("cli") => {
                CodexRolloutKind::InteractiveCli
            }
            _ => match self.originator.as_deref() {
                Some(originator)
                    if originator.eq_ignore_ascii_case("codex_exec")
                        || originator.eq_ignore_ascii_case("codex-exec") =>
                {
                    CodexRolloutKind::Exec
                }
                Some(originator)
                    if originator.eq_ignore_ascii_case("codex-tui")
                        || originator.eq_ignore_ascii_case("codex_cli")
                        || originator.eq_ignore_ascii_case("codex-cli") =>
                {
                    CodexRolloutKind::InteractiveCli
                }
                _ => CodexRolloutKind::Other,
            },
        }
    }
}

/// Read the first Codex JSONL line's session metadata. Returns defaults on
/// parse/IO failure so callers can conservatively treat it as an unknown
/// rollout rather than an exec child.
fn codex_rollout_metadata(path: &std::path::Path) -> CodexRolloutMetadata {
    let Ok(file) = std::fs::File::open(path) else {
        return CodexRolloutMetadata::default();
    };
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    if std::io::BufRead::read_line(&mut reader, &mut line).is_err() {
        return CodexRolloutMetadata::default();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return CodexRolloutMetadata::default();
    };
    // Be tolerant of missing/mismatched type — still try payload.cwd.
    let payload = value.get("payload");
    CodexRolloutMetadata {
        cwd: payload
            .and_then(|p| p.get("cwd"))
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        originator: payload
            .and_then(|p| p.get("originator"))
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        source: payload
            .and_then(|p| p.get("source"))
            .and_then(|value| value.as_str())
            .map(str::to_owned),
    }
}

/// Read `payload.cwd` from the first JSONL line when it is a `session_meta`
/// event. Returns `None` on any parse/IO failure — callers treat that as
/// "this rollout does not match by cwd".
pub(crate) fn codex_rollout_cwd(path: &std::path::Path) -> Option<String> {
    codex_rollout_metadata(path).cwd
}

/// Scan budget for `**/rollout-*.jsonl` under the sessions root. Codex hosts
/// accumulate hundreds of historical rollouts; matching by cwd only needs
/// recent ones, so we mtime-sort and cap the walk (cas-c655 / cas-900b cap
/// spirit).
const MAX_CODEX_ROLLOUT_SCAN: usize = 200;

/// Collect recent rollout paths under `sessions_dir`, newest mtime first.
fn collect_codex_rollouts(sessions_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let pattern = format!("{}/**/rollout-*.jsonl", sessions_dir.to_string_lossy());
    let iter = match glob::glob(&pattern) {
        Ok(it) => it,
        Err(_) => return Vec::new(),
    };
    // Keep only the newest bounded working set while walking. The previous
    // collect-all-then-truncate shape capped the returned vector but still
    // allocated every historical rollout on each worker_status poll.
    let mut newest = std::collections::BinaryHeap::with_capacity(MAX_CODEX_ROLLOUT_SCAN);
    for path in iter.flatten() {
        let modified = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if newest.len() < MAX_CODEX_ROLLOUT_SCAN {
            newest.push(std::cmp::Reverse((modified, path)));
            continue;
        }
        if newest
            .peek()
            .is_some_and(|std::cmp::Reverse((oldest, _))| modified > *oldest)
        {
            newest.pop();
            newest.push(std::cmp::Reverse((modified, path)));
        }
    }
    newest
        .into_sorted_vec()
        .into_iter()
        .map(|std::cmp::Reverse((_, path))| path)
        .collect()
}

/// Whether `path`'s filename contains `session_id` (rollout UUID match).
fn codex_rollout_filename_matches_session(path: &std::path::Path, session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|name| name.contains(session_id))
}

fn resolve_codex_transcript(
    sessions_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
) -> TranscriptResolution {
    let synthesized = clone_path
        .map(|p| synthesized_codex_transcript_path(p, session_id))
        .unwrap_or_else(|| synthesized_unknown_codex_clone_path(session_id));
    let Some(dir) = sessions_dir else {
        return TranscriptResolution::Synthesized(synthesized);
    };
    let candidates = collect_codex_rollouts(dir);
    let mut id_matches = Vec::new();
    let mut cli_matches = Vec::new();
    let mut fallback_matches = Vec::new();
    let mut truncated = false;
    for path in candidates {
        let metadata = codex_rollout_metadata(&path);
        let cwd_hit = clone_path
            .map(|cwd| metadata.cwd.as_deref() == Some(cwd))
            .unwrap_or(false);
        let id_hit = codex_rollout_filename_matches_session(&path, session_id);
        if id_hit {
            if id_matches.len() >= MAX_TRANSCRIPT_CANDIDATES {
                truncated = true;
                continue;
            }
            id_matches.push(path);
            continue;
        }
        if !cwd_hit {
            continue;
        }
        let destination = match metadata.kind() {
            CodexRolloutKind::InteractiveCli => &mut cli_matches,
            CodexRolloutKind::Exec => continue,
            CodexRolloutKind::Other => &mut fallback_matches,
        };
        if destination.len() >= MAX_TRANSCRIPT_CANDIDATES {
            truncated = true;
            continue;
        }
        destination.push(path);
    }

    match id_matches.len() {
        1 => return TranscriptResolution::Resolved(id_matches.remove(0)),
        2.. => {
            return TranscriptResolution::Ambiguous {
                matches: id_matches,
                synthesized,
                truncated,
            };
        }
        0 => {}
    }

    // `collect_codex_rollouts` is newest-first. Choose freshness only after
    // excluding exec children, never across the raw cwd match set.
    if let Some(path) = cli_matches.into_iter().next() {
        return TranscriptResolution::Resolved(path);
    }
    if let Some(path) = fallback_matches.into_iter().next() {
        return TranscriptResolution::Resolved(path);
    }
    TranscriptResolution::Synthesized(synthesized)
}

fn worker_status_codex_path_from_resolution(
    resolution: TranscriptResolution,
) -> Option<std::path::PathBuf> {
    match resolution {
        TranscriptResolution::Resolved(path) => Some(path),
        TranscriptResolution::Synthesized(_) | TranscriptResolution::Ambiguous { .. } => None,
    }
}

/// Pick the concrete evidence path from a transcript resolution.
///
/// This preserves `cas factory is-wedged`'s historical ambiguity behavior.
/// `worker_status` uses a stricter Resolved-only selector below. The Codex
/// resolver now disambiguates normal cli+exec cwd collisions before either
/// selector sees them, but exact-ID collisions can still be Ambiguous.
pub(crate) fn transcript_path_from_resolution(
    resolution: TranscriptResolution,
) -> Option<std::path::PathBuf> {
    match resolution {
        TranscriptResolution::Resolved(path) => Some(path),
        TranscriptResolution::Ambiguous { mut matches, .. } => {
            matches.sort_by_key(|path| {
                std::fs::metadata(path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
            });
            matches.pop()
        }
        TranscriptResolution::Synthesized(_) => None,
    }
}

/// Resolve a live worker's real transcript/rollout path using the same
/// harness-aware resolver as `cas factory is-wedged`.
pub(crate) fn resolve_worker_transcript_path(
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> Option<std::path::PathBuf> {
    let base_dir = match cli {
        cas_mux::SupervisorCli::Grok => default_grok_sessions_dir(),
        cas_mux::SupervisorCli::Codex => default_codex_sessions_dir(),
        cas_mux::SupervisorCli::Claude => default_claude_projects_dir(),
    };
    resolve_worker_transcript_path_in(base_dir.as_deref(), clone_path, session_id, cli)
}

/// Resolve the activity/context path for `worker_status`.
///
/// Codex is the cas-fa69 fix: use the existing cli-aware resolver so a real
/// rollout is reachable. Claude and Grok intentionally retain the historical
/// single-stat fast path byte-for-byte (AC7); changing those harnesses belongs
/// to a separately characterized change.
fn worker_status_transcript_path(
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> Option<std::path::PathBuf> {
    match cli {
        cas_mux::SupervisorCli::Codex => worker_status_codex_transcript_path(
            clone_path,
            session_id,
        ),
        cas_mux::SupervisorCli::Claude | cas_mux::SupervisorCli::Grok => {
            transcript_path_fast(clone_path, session_id)
        }
    }
}

fn worker_status_codex_transcript_path(
    clone_path: Option<&str>,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    let sessions_dir = default_codex_sessions_dir();
    worker_status_codex_transcript_path_in(
        sessions_dir.as_deref(),
        clone_path,
        session_id,
    )
}

/// Codex worker_status accepts only a resolved real rollout. Synthesized paths
/// are not evidence (cas-de95), and any residual Ambiguous result remains
/// unresolved rather than inventing an activity age.
fn worker_status_codex_transcript_path_in(
    sessions_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    worker_status_codex_path_from_resolution(resolve_transcript(
        sessions_dir,
        clone_path,
        session_id,
        cas_mux::SupervisorCli::Codex,
    ))
}

/// Injectable half of [`resolve_worker_transcript_path`] for deterministic
/// path-resolution and latency tests.
fn resolve_worker_transcript_path_in(
    base_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> Option<std::path::PathBuf> {
    transcript_path_from_resolution(resolve_transcript(
        base_dir, clone_path, session_id, cli,
    ))
}

/// Render the transcript block for `worker_status` output. Always surfaces
/// the raw `session_id` so a supervisor who doesn't trust our resolution
/// can grep the projects tree themselves (cas-900b AC).
///
/// `clone_path == None` is handled by the same resolver with a clearly
/// labelled fallback — no duplicated glob+match dispatch (maintainability
/// cas-900b P2). `cli` picks the harness-appropriate base dir (cas-058f).
fn format_transcript_block(
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> String {
    let base_dir = match cli {
        cas_mux::SupervisorCli::Grok => default_grok_sessions_dir(),
        cas_mux::SupervisorCli::Codex => default_codex_sessions_dir(),
        cas_mux::SupervisorCli::Claude => default_claude_projects_dir(),
    };
    let resolution = resolve_transcript(base_dir.as_deref(), clone_path, session_id, cli);
    render_transcript_block(&resolution, session_id, base_dir.is_some())
}

/// Pure string-rendering half of `format_transcript_block`, split out so
/// tests can drive it against a `TranscriptResolution` built via the
/// injectable resolver without touching `dirs::home_dir()`.
fn render_transcript_block(
    resolution: &TranscriptResolution,
    session_id: &str,
    home_resolved: bool,
) -> String {
    match resolution {
        TranscriptResolution::Resolved(path) => format!(
            "\n    Transcript: {}\n    Session: {session_id}",
            path.display()
        ),
        TranscriptResolution::Synthesized(path) => {
            // Distinguish home-dir-unresolvable from "glob returned 0
            // matches" so a supervisor triaging the output knows which
            // failure mode to chase (adversarial cas-900b P3).
            if home_resolved {
                format!("\n    Likely transcript: {path}\n    Session: {session_id}")
            } else {
                format!(
                    "\n    Likely transcript: {path}\n    (home dir unresolvable — glob skipped)\n    Session: {session_id}"
                )
            }
        }
        TranscriptResolution::Ambiguous {
            matches,
            synthesized,
            truncated,
        } => {
            let mut s = format!("\n    Transcript candidates (session {session_id}):");
            for m in matches {
                s.push_str(&format!("\n      - {}", m.display()));
            }
            if *truncated {
                s.push_str(&format!(
                    "\n      … (truncated at {MAX_TRANSCRIPT_CANDIDATES}; grep ~/.claude/projects for session)"
                ));
            }
            s.push_str(&format!("\n    Likely synthesized: {synthesized}"));
            s
        }
    }
}

// ============================================================================
// cas-573c: Context-usage indicator for worker_status
// ============================================================================

/// Standard Claude context window in tokens. Most frontier models (Sonnet,
/// Opus) support 200 K input tokens. Haiku is 100 K but we don't distinguish
/// at this layer — the bands are conservative enough that a Haiku worker only
/// reaches `near-limit` when it's genuinely close to compaction.
const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;

/// Classify a token count as a coarse context-usage band.
///
/// Thresholds (against `DEFAULT_CONTEXT_WINDOW = 200k`):
/// - `ok`          — 0–49 % (< 100k) — no action needed
/// - `approaching` — 50–79 % (100k–159k) — warn supervisor
/// - `near-limit`  — ≥ 80 % (≥ 160k) — proactively preserve work
pub(crate) fn context_band(total_input_tokens: u64) -> &'static str {
    let pct = total_input_tokens * 100 / DEFAULT_CONTEXT_WINDOW;
    match pct {
        0..=49 => "ok",
        50..=79 => "approaching",
        _ => "near-limit",
    }
}

/// Historical live-worker path lookup used by Claude and Grok reporting.
///
/// Reconstructs the Claude-layout path from `clone_path` + `session_id` and
/// checks it with one `stat(2)`. Keeping this unchanged preserves the
/// cas-573c latency and output contract for non-Codex harnesses.
fn transcript_path_fast(
    clone_path: Option<&str>,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    transcript_path_fast_in(&home, clone_path, session_id)
}

fn transcript_path_fast_in(
    home: &std::path::Path,
    clone_path: Option<&str>,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    let clone = clone_path?;
    let synthesized = synthesized_transcript_path(clone, session_id);
    let relative = synthesized.strip_prefix("~/")?;
    let real = home.join(relative);
    if real.exists() { Some(real) } else { None }
}

/// Read the last ≤ 8 KB of a JSONL session transcript and return the total
/// input-token count from the most recent assistant message that carries usage.
///
/// **Why tail-only?** Session transcripts grow throughout a session and can
/// reach tens of MB. Reading the full file on every `worker_status` poll
/// would violate the latency AC (cas-573c). The tail approach reads a
/// bounded slice (~8 KB, one `read(2)` after a `lseek(2)`) and scans it
/// backward for the latest assistant entry — the freshest usage snapshot.
///
/// Returns `None` when the file can't be opened, the tail has no parseable
/// assistant entry with a `usage` field, or any I/O error occurs.
pub(crate) fn read_context_usage_from_tail(path: &std::path::Path) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();

    const TAIL_BYTES: u64 = 8192;
    let start = file_len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;

    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;

    // Scan in reverse: the last assistant entry with usage is the freshest.
    for line in buf.lines().rev() {
        // Quick pre-filter before full parse to avoid repeated serde overhead.
        if !line.contains("\"type\":\"assistant\"") {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let usage = v.get("message")?.get("usage")?;
        let input = usage
            .get("input_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_create = usage
            .get("cache_creation_input_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_read = usage
            .get("cache_read_input_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        return Some(input + cache_create + cache_read);
    }
    None
}

/// Harness-aware context usage reader. Claude retains its existing parser
/// byte-for-byte; Codex reads the latest rollout `token_count` event's
/// per-turn input total. Grok has no equivalent parser at this layer yet.
fn read_context_usage_from_tail_for_cli(
    path: &std::path::Path,
    cli: cas_mux::SupervisorCli,
) -> Option<u64> {
    if cli != cas_mux::SupervisorCli::Codex {
        return read_context_usage_from_tail(path);
    }

    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    const TAIL_BYTES: u64 = 8192;
    file.seek(SeekFrom::Start(file_len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;

    for line in buf.lines().rev() {
        if !line.contains("\"type\":\"token_count\"") {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.pointer("/payload/type").and_then(|v| v.as_str()) != Some("token_count") {
            continue;
        }
        return value
            .pointer("/payload/info/last_token_usage/input_tokens")
            .and_then(|v| v.as_u64());
    }
    None
}

// =============================================================================
// B1 (cas-844bf): worker_status git introspection
// =============================================================================

/// Git state snapshot for a factory worker.
///
/// All fields are best-effort: a failed git sub-command yields a sentinel
/// value ("?" or "none" or 0) rather than aborting the status render.
/// See [`collect_worker_git_status`] for field semantics.
///
/// `pub(crate)` so the Stop hook (cas-5c0a) can reuse this struct without
/// creating a divergent duplicate.
#[derive(Debug)]
pub(crate) struct WorkerGitStatus {
    /// Current branch name (or "HEAD" if detached, "?" on error)
    pub branch: String,
    /// Short HEAD SHA (7 hex chars, or "?" on error)
    pub head_sha: String,
    /// Commits ahead of `base_branch` (0 when the count can't be determined)
    pub ahead: usize,
    /// Commits behind `base_branch` (0 when the count can't be determined)
    pub behind: usize,
    /// Branch used as the ahead/behind baseline (e.g. "origin/main")
    pub base_branch: String,
    /// `true` if the working tree has staged or unstaged changes
    pub dirty: bool,
    /// `"origin/<branch>"` when the branch has been pushed, otherwise `"none"`
    pub pushed_ref: String,
    /// Open pull-request URL, or `"none"` when not found / gh unavailable
    pub pr_url: String,
}

/// Collect git introspection data for a worker's worktree path.
///
/// Every git sub-command degrades gracefully on failure — a non-git dir or
/// missing network produces sentinel values, never a panic.
///
/// `pub(crate)` so the Stop hook (cas-5c0a / B3) can call this without
/// duplicating the collector.
///
/// NOTE: This is a synchronous, blocking function that shells out to `git`
/// and, optionally, `gh`.  It is intended to be called from within
/// `factory_worker_status` (which is `async` but performs several other
/// blocking operations already).  Callers in a strict async context should
/// wrap in `tokio::task::spawn_blocking` if needed.
pub(crate) fn collect_worker_git_status(worktree_path: &std::path::Path) -> WorkerGitStatus {
    // --- current branch -------------------------------------------------------
    let branch = run_git(worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_else(|_| "?".to_string());

    // --- HEAD short SHA -------------------------------------------------------
    let head_sha = run_git(worktree_path, &["rev-parse", "--short", "HEAD"])
        .unwrap_or_else(|_| "?".to_string());

    // --- base branch for ahead/behind ----------------------------------------
    // Prefer origin/HEAD (most authoritative), then fall back to "main".
    // `--short` strips the "refs/remotes/" prefix → "origin/main".
    let base_branch = run_git(
        worktree_path,
        &["symbolic-ref", "refs/remotes/origin/HEAD", "--short"],
    )
    .unwrap_or_else(|_| {
        // No remote HEAD symref; try origin/main, then plain "main".
        let probe = run_git(
            worktree_path,
            &["rev-parse", "--verify", "refs/remotes/origin/main"],
        );
        if probe.is_ok() {
            "origin/main".to_string()
        } else {
            "main".to_string()
        }
    });

    // --- ahead / behind -------------------------------------------------------
    // `git rev-list --left-right --count <base>...HEAD`
    // Output: "<behind>\t<ahead>" (the left side is commits in base not in HEAD).
    let (ahead, behind) = run_git(
        worktree_path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{base_branch}...HEAD"),
        ],
    )
    .map(|s| {
        let parts: Vec<&str> = s.split_whitespace().collect();
        let behind = parts.first().and_then(|v| v.parse().ok()).unwrap_or(0usize);
        let ahead = parts.get(1).and_then(|v| v.parse().ok()).unwrap_or(0usize);
        (ahead, behind)
    })
    .unwrap_or((0, 0));

    // --- dirty? ---------------------------------------------------------------
    let dirty = run_git(worktree_path, &["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    // --- pushed ref -----------------------------------------------------------
    // Check whether `refs/remotes/origin/<branch>` exists locally (i.e. has the
    // branch been pushed and fetched at least once).
    let pushed_ref = if branch == "?" {
        "none".to_string()
    } else {
        let origin_ref = format!("origin/{branch}");
        let exists = run_git(
            worktree_path,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/remotes/{origin_ref}"),
            ],
        )
        .is_ok();
        if exists {
            origin_ref
        } else {
            "none".to_string()
        }
    };

    // --- open PR URL (gh, graceful degrade) -----------------------------------
    // Only attempt the `gh` query when we know the branch has been pushed —
    // otherwise it will always return nothing and adds ~200ms latency.
    let pr_url = if pushed_ref == "none" || branch == "?" {
        "none".to_string()
    } else {
        std::process::Command::new("gh")
            .args([
                "pr", "list", "--head", &branch, "--json", "url", "--jq", ".[0].url",
            ])
            .current_dir(worktree_path)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "none".to_string())
    };

    WorkerGitStatus {
        branch,
        head_sha,
        ahead,
        behind,
        base_branch,
        dirty,
        pushed_ref,
        pr_url,
    }
}

/// Render a `WorkerGitStatus` as a multi-line block for injection into the
/// `worker_status` output.  Returns an empty string when the status is
/// entirely unknown (all sentinel values).
pub(crate) fn format_worker_git_status(gs: &WorkerGitStatus) -> String {
    // Skip entirely if everything is unknown — this keeps the output clean for
    // non-isolated (non-worktree) workers where the clone_path may not be set.
    if gs.branch == "?" && gs.head_sha == "?" {
        return String::new();
    }

    let dirty_label = if gs.dirty { "[dirty]" } else { "[clean]" };
    let pushed_label = if gs.pushed_ref == "none" {
        "[not pushed]".to_string()
    } else {
        format!("[pushed: {}]", gs.pushed_ref)
    };
    let pr_label = gs.pr_url.clone(); // "none" or a URL

    format!(
        "\n    git: {} @ {} {} {}\
         \n    ahead: {} behind: {} (vs {})\
         \n    PR: {}",
        gs.branch,
        gs.head_sha,
        dirty_label,
        pushed_label,
        gs.ahead,
        gs.behind,
        gs.base_branch,
        pr_label,
    )
}

// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::AgentRole;

    /// Session id used across the glob tests. Stable UUID shape, unique
    /// across fake projects so the glob doesn't collide with anything else
    /// the test happens to create.
    const TEST_SESSION: &str = "cas-900b-test-0000-0000-000000000000";

    struct HomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        old_home: Option<String>,
        _tmp: tempfile::TempDir,
    }

    impl HomeGuard {
        fn isolated() -> Self {
            let lock = crate::hooks::test_env_lock();
            let old_home = std::env::var("HOME").ok();
            let tmp = tempfile::tempdir().expect("temp HOME");
            unsafe {
                std::env::set_var("HOME", tmp.path());
            }
            Self {
                _lock: lock,
                old_home,
                _tmp: tmp,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.old_home {
                    Some(home) => std::env::set_var("HOME", home),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn decoded_spawn_spec(json: &str) -> cas_mux::WorkerSpec {
        serde_json::from_str(json).expect("valid WorkerSpec JSON")
    }

    /// Create a fake `~/.claude/projects/` layout in a tempdir and return
    /// the `projects` subdir path. `projects` is populated with `dirs`
    /// entries, each containing a `<session_id>.jsonl` for sessions in
    /// that dir's `contains_sessions` list.
    fn fake_projects_dir(dirs: &[(&str, &[&str])]) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        for (dir_name, sessions) in dirs {
            let d = projects.join(dir_name);
            std::fs::create_dir_all(&d).unwrap();
            for s in *sessions {
                std::fs::write(d.join(format!("{s}.jsonl")), b"").unwrap();
            }
        }
        (tmp, projects)
    }

    #[test]
    fn synthesized_path_matches_claude_code_escape() {
        // Observed in the field (crisp-badger-65): keep this pinned as the
        // fallback contract for Synthesized / Ambiguous branches.
        let clone = "/home/pippenz/Petrastella/cas-src/.cas/worktrees/crisp-badger-65";
        let session = "064e7b23-331d-4dae-9c6a-721cbbe9c024";
        let got = synthesized_transcript_path(clone, session);
        assert_eq!(
            got,
            "~/.claude/projects/-home-pippenz-Petrastella-cas-src--cas-worktrees-crisp-badger-65/\
             064e7b23-331d-4dae-9c6a-721cbbe9c024.jsonl"
        );
    }

    #[test]
    fn spawn_spec_cli_claude_without_model_effort_uses_safe_floor() {
        let _home = HomeGuard::isolated();
        let json = build_spawn_spec_json(Some("claude"), None, None).unwrap();
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(spec.model.as_deref(), Some("sonnet"));
        assert_eq!(spec.effort, Some(cas_mux::Effort::Medium));
    }

    #[test]
    fn spawn_spec_explicit_model_and_effort_are_unchanged() {
        let _home = HomeGuard::isolated();
        let json = build_spawn_spec_json(Some("claude"), Some("opus"), Some("high")).unwrap();
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(spec.model.as_deref(), Some("opus"));
        assert_eq!(spec.effort, Some(cas_mux::Effort::High));
    }

    #[test]
    fn spawn_spec_omitted_cli_without_config_uses_stock_codex_defaults() {
        let _home = HomeGuard::isolated();
        let json = build_spawn_spec_json(None, None, None).unwrap();
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::Codex);
        assert_eq!(
            spec.model.as_deref(),
            Some(crate::config::STOCK_WORKER_MODEL)
        );
        assert_eq!(
            spec.effort,
            Some(
                crate::config::STOCK_WORKER_REASONING_EFFORT
                    .parse::<cas_mux::Effort>()
                    .unwrap()
            )
        );
    }

    #[test]
    fn spawn_spec_omitted_cli_respects_project_factory_default() {
        let _home = HomeGuard::isolated();
        let tmp = tempfile::tempdir().expect("temp project config");
        let config = tmp.path().join("config.toml");
        std::fs::write(
            &config,
            r#"
[factory.defaults]
cli = "claude"
model = "sonnet"
effort = "high"
"#,
        )
        .unwrap();

        let json =
            build_spawn_spec_json_with_project_config(None, None, None, Some(config)).unwrap();
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(spec.model.as_deref(), Some("sonnet"));
        assert_eq!(spec.effort, Some(cas_mux::Effort::High));
    }

    #[test]
    fn spawn_spec_project_defaults_can_force_frontier_and_warning_nags() {
        let _home = HomeGuard::isolated();
        let tmp = tempfile::tempdir().expect("temp project config");
        let config = tmp.path().join("config.toml");
        std::fs::write(
            &config,
            r#"
[factory.defaults]
model = "opus"
effort = "high"
"#,
        )
        .unwrap();

        let json =
            build_spawn_spec_json_with_project_config(Some("claude"), None, None, Some(config))
                .unwrap();
        let spec = decoded_spawn_spec(&json);
        let warning = spawn_spec_warning(false, false, &json);

        assert_eq!(spec.model.as_deref(), Some("opus"));
        assert_eq!(spec.effort, Some(cas_mux::Effort::High));
        assert!(warning.contains("frontier-tier"), "{warning}");
    }

    // cas-7199 / cas-a487: `strict_cli_from_project_config` tests.

    #[test]
    fn strict_cli_from_project_config_true_when_set() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("config.toml"),
            "[factory]\nstrict_cli = true\n",
        )
        .unwrap();
        assert!(strict_cli_from_project_config(Some(
            &tmp.path().join("config.toml")
        )));
    }

    #[test]
    fn strict_cli_from_project_config_false_by_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("config.toml"), "[factory]\n").unwrap();
        assert!(!strict_cli_from_project_config(Some(
            &tmp.path().join("config.toml")
        )));
    }

    #[test]
    fn strict_cli_from_project_config_false_when_missing_or_none() {
        assert!(!strict_cli_from_project_config(None));
        assert!(!strict_cli_from_project_config(Some(
            &std::path::PathBuf::from("/tmp/cas-7199-definitely-missing/config.toml")
        )));
    }

    /// End-to-end (still deterministic — `HomeGuard::isolated()` guarantees
    /// no real `~/.codex/auth.json`, so the fallback always fires
    /// regardless of the test machine's actual Codex install/login state):
    /// a spec that resolves to Codex, with Codex unavailable, must come
    /// back rewritten to Claude via `apply_codex_fallback` — the same path
    /// `factory_spawn_workers` drives. Exercises the resolver + fallback
    /// composition directly (below the async MCP handler, which needs a
    /// full task/spawn-queue store to invoke) so this stays a fast unit
    /// test while still proving the two pieces compose correctly.
    #[test]
    fn resolved_codex_spec_falls_back_to_claude_when_codex_unavailable() {
        let _home = HomeGuard::isolated();
        let json = build_spawn_spec_json(None, None, None).unwrap();
        let mut spec = decoded_spawn_spec(&json);
        assert_eq!(
            spec.cli,
            cas_mux::SupervisorCli::Codex,
            "precondition: stock default must resolve to codex"
        );

        let claude_default_model = default_worker_model_for_cli(cas_mux::SupervisorCli::Claude);
        let notices = cas_factory::apply_codex_fallback(
            std::slice::from_mut(&mut spec),
            false,
            Some(claude_default_model),
        )
        .unwrap();

        assert_eq!(spec.cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(spec.model.as_deref(), Some(claude_default_model));
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("codex unavailable"));
    }

    #[test]
    fn synthesized_path_escapes_dots_preserves_underscores() {
        let got = synthesized_transcript_path("/tmp/my_proj.sub", "abc");
        assert_eq!(got, "~/.claude/projects/-tmp-my_proj-sub/abc.jsonl");
    }

    // --- cas-8240: two-band stale/dead threshold constants ------------------

    /// AC anchor: `WORKER_STALE_SECS` is pinned at 30. The supervisor-facing
    /// footer embeds this value as a literal ("30s heartbeat age") and the
    /// daemon heartbeat tick is tuned against it, so a silent change here
    /// would desync the prune window from the UX text.
    #[test]
    fn worker_stale_secs_is_pinned_at_30() {
        assert_eq!(WORKER_STALE_SECS, 30);
    }

    /// AC anchor: `WORKER_DEAD_SECS` is pinned at 75. The two-band model
    /// requires DEAD to lag STALE by roughly one grace window so scheduler
    /// jitter and missed ticks do not produce false-positive [DEAD] labels.
    /// Bumping this silently would regress the cas-8240 fix.
    #[test]
    fn worker_dead_secs_is_pinned_at_75() {
        assert_eq!(WORKER_DEAD_SECS, 75);
    }

    /// Invariant: the dead threshold must strictly exceed the stale
    /// threshold. Otherwise the two-band render collapses into one band
    /// and we reintroduce the false-positive DEAD labeling cas-8240 fixes.
    #[test]
    fn worker_dead_secs_exceeds_stale_secs() {
        assert!(
            WORKER_DEAD_SECS > WORKER_STALE_SECS,
            "WORKER_DEAD_SECS ({WORKER_DEAD_SECS}) must exceed WORKER_STALE_SECS ({WORKER_STALE_SECS}) — the two-band model collapses otherwise"
        );
    }

    // --- cas-8240: liveness_label_for branch matrix -------------------------

    #[test]
    fn liveness_label_fresh_worker_is_empty() {
        assert_eq!(liveness_label_for(0), "");
        assert_eq!(liveness_label_for(WORKER_STALE_SECS - 1), "");
    }

    #[test]
    fn liveness_label_grace_window_is_stale() {
        // Exactly at STALE → [stale]; just below DEAD → still [stale].
        assert_eq!(liveness_label_for(WORKER_STALE_SECS), " [stale]");
        assert_eq!(liveness_label_for(WORKER_DEAD_SECS - 1), " [stale]");
    }

    #[test]
    fn liveness_label_past_dead_is_hard_dead() {
        // Exactly at DEAD → [DEAD]; well past → still [DEAD].
        assert_eq!(liveness_label_for(WORKER_DEAD_SECS), " [DEAD]");
        assert_eq!(liveness_label_for(WORKER_DEAD_SECS * 10), " [DEAD]");
    }

    #[test]
    fn liveness_label_distinguishes_stale_from_dead() {
        // The cas-8240 core behavior: stale and DEAD are distinct bands.
        // A mutation that collapsed the stale branch into " [DEAD]"
        // would fail here.
        let stale = liveness_label_for(WORKER_STALE_SECS);
        let dead = liveness_label_for(WORKER_DEAD_SECS);
        assert_ne!(
            stale, dead,
            "stale and DEAD bands must render distinct labels"
        );
        assert!(stale.contains("stale"));
        assert!(dead.contains("DEAD"));
    }

    // --- cas-9829: is_worker_stalled helper ----------------------------------

    #[test]
    fn is_worker_stalled_false_without_in_progress_task() {
        // Idle worker with no observed activity at all — not "stalled",
        // that's the pre-existing "may be investigating or idle" case.
        assert!(!is_worker_stalled(false, None, 300, false));
        assert!(!is_worker_stalled(false, Some(1_000), 300, false));
    }

    #[test]
    fn is_worker_stalled_true_when_no_activity_observed_with_task() {
        assert!(is_worker_stalled(true, None, 300, false));
    }

    #[test]
    fn is_worker_stalled_compares_against_threshold() {
        assert!(!is_worker_stalled(true, Some(299), 300, false));
        assert!(is_worker_stalled(true, Some(300), 300, false));
        assert!(is_worker_stalled(true, Some(301), 300, false));
    }

    // --- cas-d165: is_worker_stalled in-flight-tool-call suppression --------
    //
    // Live no-fire fixture that motivated this: agile-puma-14 held an
    // in-progress task, `cas factory is-wedged` reported
    // `in-flight tool call: true` (it had dispatched a research subagent),
    // yet the OLD 3-arg `is_worker_stalled` had no way to know that and
    // would have kept reporting `⚠ STALLED` on old/absent checkpoint data.

    #[test]
    fn is_worker_stalled_suppressed_by_in_flight_tool_call_even_with_no_activity() {
        // Same shape as `is_worker_stalled_true_when_no_activity_observed_with_task`
        // but with an in-flight call — must now report false.
        assert!(!is_worker_stalled(true, None, 300, true));
    }

    #[test]
    fn is_worker_stalled_suppressed_by_in_flight_tool_call_past_threshold() {
        // Checkpoint age well past the threshold would normally stall —
        // an in-flight tool call overrides that regardless of age.
        assert!(!is_worker_stalled(true, Some(10_000), 300, true));
    }

    #[test]
    fn is_worker_stalled_still_fires_without_in_flight_evidence() {
        // Safety property (mirrors cas-7e85 AC2): absence of an in-flight
        // call must NOT itself suppress — a genuinely cold worker with no
        // outstanding call still stalls exactly as before.
        assert!(is_worker_stalled(true, Some(301), 300, false));
        assert!(is_worker_stalled(true, None, 300, false));
    }

    // --- cas-86c5: last_worker_activity_secs helper -------------------------

    fn make_event(
        event_type: cas_types::EventType,
        session_id: &str,
        age_secs: i64,
    ) -> cas_types::Event {
        let mut e = cas_types::Event::new(
            event_type,
            cas_types::EventEntityType::Agent,
            session_id,
            "test summary",
        )
        .with_session(session_id);
        // Back-date `created_at` by `age_secs` seconds to simulate historical events.
        e.created_at = chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        e
    }

    /// No events → None.
    #[test]
    fn last_worker_activity_returns_none_for_empty_events() {
        let got = last_worker_activity_secs(&[], "agent-abc");
        assert!(got.is_none(), "empty slice must return None");
    }

    /// No events for THIS agent (events exist for another) → None.
    #[test]
    fn last_worker_activity_returns_none_when_no_events_for_agent() {
        let events = vec![make_event(
            cas_types::EventType::WorkerFileEdited,
            "other-agent",
            10,
        )];
        let got = last_worker_activity_secs(&events, "my-agent");
        assert!(
            got.is_none(),
            "events for other agents must not match; got {got:?}"
        );
    }

    /// WorkerFileEdited → "editing" phase label.
    #[test]
    fn last_worker_activity_phase_file_edited_is_editing() {
        let events = vec![make_event(
            cas_types::EventType::WorkerFileEdited,
            "agent-x",
            5,
        )];
        let (elapsed, phase) =
            last_worker_activity_secs(&events, "agent-x").expect("must find the event");
        assert_eq!(phase, "editing");
        assert!(
            elapsed >= 4 && elapsed <= 7,
            "elapsed should be ~5s: {elapsed}"
        );
    }

    /// WorkerGitCommit → "checkpoint" phase label (cas-86c5: renamed from session-stop).
    #[test]
    fn last_worker_activity_phase_git_commit_is_checkpoint() {
        let events = vec![make_event(
            cas_types::EventType::WorkerGitCommit,
            "agent-y",
            20,
        )];
        let (_, phase) =
            last_worker_activity_secs(&events, "agent-y").expect("must find the event");
        assert_eq!(phase, "checkpoint");
    }

    /// With multiple events, the FRESHEST one wins.
    #[test]
    fn last_worker_activity_returns_freshest_event() {
        let events = vec![
            make_event(cas_types::EventType::WorkerGitCommit, "agent-z", 120), // 2m old
            make_event(cas_types::EventType::WorkerFileEdited, "agent-z", 15), // 15s old
        ];
        let (elapsed, phase) =
            last_worker_activity_secs(&events, "agent-z").expect("must find an event");
        assert_eq!(
            phase, "editing",
            "freshest event should be the FileEdited at 15s"
        );
        assert!(
            elapsed >= 14 && elapsed <= 17,
            "elapsed should be ~15s: {elapsed}"
        );
    }

    // --- cas-a653 / cas-c2c2: last_worker_activity_secs_with_transcript -----
    //
    // Reproduction of the reported defect: a worker whose last CAS event is
    // far in the past (frozen clock) but whose transcript is being actively
    // written. Before cas-a653, `worker_status` fed ONLY the event-store age
    // into the "last activity" line — indistinguishable from a genuinely
    // dead worker. cas-a653 fixed this for hook-less harnesses (Codex) only;
    // cas-c2c2 (below, after the Codex-specific tests) widened it to every
    // harness after the same freeze reproduced live on a Claude worker —
    // see the superseding-tests block further down for why "hook-capable"
    // was never the right gate.

    /// Codex, no CAS events at all, but a transcript that was JUST written —
    /// must report the transcript's freshness, not `None`. This is the
    /// literal reported symptom: "last activity: none / frozen" while the
    /// worker's rollout kept recording tool calls every minute.
    #[test]
    fn last_worker_activity_with_transcript_codex_no_events_uses_transcript_mtime() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        // Freshly created/touched -> mtime age ~0s.
        let got = last_worker_activity_secs_with_transcript(
            &[],
            "agent-codex",
            cas_mux::SupervisorCli::Codex,
            Some(tmp.path()),
        );
        let (elapsed, phase) = got.expect(
            "a fresh transcript must surface SOME activity age for a hook-less harness, \
             not None (the frozen/absent-clock symptom cas-a653 reports)",
        );
        assert!(elapsed <= 5, "expected a near-zero age, got {elapsed}s");
        assert_eq!(phase, "activity");
    }

    /// Codex with a STALE CAS event (e.g. 20 minutes old, well past any
    /// stall threshold) but a transcript mtime of just a few seconds ago —
    /// the exact "heads-down between CAS calls" shape from the ozer repro.
    /// The fresher (transcript) signal must win, not the frozen event age.
    #[test]
    fn last_worker_activity_with_transcript_codex_prefers_fresher_transcript_over_stale_event() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let events = vec![make_event(
            cas_types::EventType::WorkerGitCommit,
            "agent-codex",
            20 * 60, // 20 minutes old — the "frozen clock" reading
        )];
        let (elapsed, phase) = last_worker_activity_secs_with_transcript(
            &events,
            "agent-codex",
            cas_mux::SupervisorCli::Codex,
            Some(tmp.path()),
        )
        .expect("must resolve an age");
        assert!(
            elapsed < 60,
            "fresher transcript mtime must win over a 20m-stale CAS event; got {elapsed}s"
        );
        assert_eq!(phase, "activity");
    }

    /// Codex with a FRESH CAS event and a stale transcript — the event-store
    /// signal is still the fresher one here, so it (and its richer phase
    /// label) must win, not be discarded.
    #[test]
    fn last_worker_activity_with_transcript_codex_prefers_fresher_event_over_stale_transcript() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let old_mtime = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(20 * 60),
        );
        filetime::set_file_mtime(tmp.path(), old_mtime).unwrap();
        let events = vec![make_event(
            cas_types::EventType::WorkerFileEdited,
            "agent-codex",
            5,
        )];
        let (elapsed, phase) = last_worker_activity_secs_with_transcript(
            &events,
            "agent-codex",
            cas_mux::SupervisorCli::Codex,
            Some(tmp.path()),
        )
        .expect("must resolve an age");
        assert!(elapsed <= 7, "expected the fresh event's ~5s age: {elapsed}");
        assert_eq!(
            phase, "editing",
            "the richer event-derived phase label must survive, not be flattened to 'activity'"
        );
    }

    /// No transcript path resolvable for a Codex worker (e.g. session not
    /// yet matched) — falls back to the plain event-store behavior instead
    /// of panicking or fabricating an age.
    #[test]
    fn last_worker_activity_with_transcript_codex_no_transcript_falls_back_to_events() {
        let events = vec![make_event(
            cas_types::EventType::WorkerFileEdited,
            "agent-codex",
            8,
        )];
        let (elapsed, phase) = last_worker_activity_secs_with_transcript(
            &events,
            "agent-codex",
            cas_mux::SupervisorCli::Codex,
            None,
        )
        .expect("must resolve from events alone");
        assert!(elapsed >= 6 && elapsed <= 10);
        assert_eq!(phase, "editing");
    }

    // --- cas-c2c2: deliberately SUPERSEDES the cas-a653 AC5 guard tests ----
    //
    // cas-a653 originally pinned "Claude/Grok must ignore the transcript
    // signal entirely, hook-capable harnesses are always covered by the
    // event store" (`*_ignores_fresh_transcript`, asserting `is_none()`/a
    // stale age even with a fresh transcript present). That assumption was
    // falsified live on this factory: a Claude worker (`interrupt-fixer`,
    // hooks fully wired) was reported `⚠ STALLED` at 401s while its own
    // transcript had a record 2 seconds prior — because the CAS event store
    // only gets a row when a hook translates a SPECIFIC tool-use shape
    // (Edit/Write → WorkerFileEdited, git commit → WorkerGitCommit, ...)
    // into an event; a worker whose whole stretch is e.g. `Bash`-driving a
    // nested TUI produces transcript-visible activity that never reaches
    // the event store, hooks or not. So "hook-capable ⇒ event store is
    // trustworthy" was the wrong invariant.
    //
    // These two tests are intentionally REWRITTEN (not deleted) to assert
    // the new, correct behavior: transcript freshness is now folded in for
    // every harness, and a fresh transcript must be able to rescue a
    // stale/absent CAS-event reading for Claude and Grok exactly as it
    // already did for Codex. The old assertions (`is_none()` / stale-wins)
    // would now be WRONG, not merely obsolete — keeping them would silently
    // pin the cas-c2c2 defect back in place.

    /// Was `last_worker_activity_with_transcript_claude_ignores_fresh_transcript`
    /// (asserted `is_none()`). Now asserts the opposite on purpose: this is
    /// the literal shape of the interrupt-fixer incident — no CAS event
    /// at all (its Bash-loop work never touched the event store), but a
    /// transcript being written right now. Must surface that freshness.
    #[test]
    fn last_worker_activity_with_transcript_claude_no_events_uses_transcript_mtime() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile"); // mtime ~0s
        let (elapsed, phase) = last_worker_activity_secs_with_transcript(
            &[],
            "agent-claude",
            cas_mux::SupervisorCli::Claude,
            Some(tmp.path()),
        )
        .expect(
            "cas-c2c2: a fresh transcript must rescue a Claude worker with zero \
             CAS events, not report None (the interrupt-fixer STALLED-at-401s symptom)",
        );
        assert!(elapsed <= 5, "expected a near-zero age, got {elapsed}s");
        assert_eq!(phase, "activity");
    }

    /// Was `last_worker_activity_with_transcript_grok_ignores_fresh_transcript`
    /// — same rewrite, Grok variant. Uses a bare file (no sibling
    /// `signals.json`) so `effective_transcript_age` falls back to plain
    /// mtime, per `grok_activity_age`'s documented fallback.
    #[test]
    fn last_worker_activity_with_transcript_grok_no_events_uses_transcript_mtime() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let (elapsed, phase) = last_worker_activity_secs_with_transcript(
            &[],
            "agent-grok",
            cas_mux::SupervisorCli::Grok,
            Some(tmp.path()),
        )
        .expect("cas-c2c2: Grok must also be rescued by transcript freshness now");
        assert!(elapsed <= 5, "expected a near-zero age, got {elapsed}s");
        assert_eq!(phase, "activity");
    }

    /// Direct repro of the reported incident shape for Claude: a CAS event
    /// far in the past (well past any stall threshold) alongside a
    /// transcript mtime of just a few seconds ago. The fresher signal must
    /// win — this is what makes cas-c2c2's fix (AC#2) actually true.
    #[test]
    fn last_worker_activity_with_transcript_claude_prefers_fresher_transcript_over_stale_event() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let events = vec![make_event(
            cas_types::EventType::WorkerGitCommit,
            "agent-claude",
            401, // matches the reported "401s ago ⚠ STALLED" reading
        )];
        let (elapsed, phase) = last_worker_activity_secs_with_transcript(
            &events,
            "agent-claude",
            cas_mux::SupervisorCli::Claude,
            Some(tmp.path()),
        )
        .expect("must resolve an age");
        assert!(
            elapsed < 60,
            "fresher transcript mtime must win over a 401s-stale CAS event \
             (the exact interrupt-fixer reading); got {elapsed}s"
        );
        assert_eq!(phase, "activity");
    }

    /// AC#6 guard: this fix must not flatten the per-harness freshness
    /// *windows* wedged.rs owns (Claude/Grok 60s vs Codex 5m,
    /// `activity_fresh_window`) — it only changes which raw age feeds
    /// `worker_status`'s display and `is_worker_stalled`'s threshold
    /// comparison. Pin that `effective_transcript_age` (the primitive this
    /// function now calls) still dispatches Grok through
    /// `grok_activity_age`'s signals.json preference rather than a flattened
    /// plain-mtime read shared with Claude/Codex — i.e. this function did
    /// not reimplement harness dispatch, it delegates to the existing one.
    #[test]
    fn last_worker_activity_with_transcript_grok_prefers_fresher_signals_json_sibling() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let updates = tmp.path().join("updates.jsonl");
        let signals = tmp.path().join("signals.json");
        std::fs::write(&updates, b"{}").unwrap();
        // Stale updates.jsonl...
        let old_mtime = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(20 * 60),
        );
        filetime::set_file_mtime(&updates, old_mtime).unwrap();
        // ...but a signals.json sibling rewritten moments ago — the
        // finer-grained per-turn signal `grok_activity_age` prefers.
        std::fs::write(&signals, b"{}").unwrap();
        let (elapsed, _) = last_worker_activity_secs_with_transcript(
            &[],
            "agent-grok",
            cas_mux::SupervisorCli::Grok,
            Some(&updates),
        )
        .expect("must resolve an age");
        assert!(
            elapsed < 60,
            "fresh signals.json sibling must be preferred over the stale \
             updates.jsonl mtime — confirms harness dispatch still lives in \
             wedged::effective_transcript_age, not reimplemented here; got {elapsed}s"
        );
    }

    // --- cas-900b: glob-first transcript resolution -------------------------

    #[test]
    fn resolve_transcript_returns_resolved_on_unique_match() {
        // cas-900b AC (1): unique match → Resolved with the real on-disk path.
        let (_tmp, projects) = fake_projects_dir(&[
            ("-home-alice-workspace-one", &[TEST_SESSION]),
            ("-home-alice-workspace-two", &["other-session-zzz"]),
        ]);
        let got = resolve_transcript(
            Some(&projects),
            Some("/home/alice/workspace/one"),
            TEST_SESSION, cas_mux::SupervisorCli::Claude);
        let expected_path = projects
            .join("-home-alice-workspace-one")
            .join(format!("{TEST_SESSION}.jsonl"));
        assert_eq!(got, TranscriptResolution::Resolved(expected_path));
    }

    #[test]
    fn resolve_transcript_returns_synthesized_on_no_match() {
        // cas-900b AC (2): no match → Synthesized fallback, preserves
        // legacy reconstruct semantics.
        let (_tmp, projects) = fake_projects_dir(&[("-home-alice-workspace-one", &["unrelated"])]);
        let got = resolve_transcript(
            Some(&projects),
            Some("/home/alice/workspace/one"),
            TEST_SESSION, cas_mux::SupervisorCli::Claude);
        let expected = synthesized_transcript_path("/home/alice/workspace/one", TEST_SESSION);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    #[test]
    fn resolve_transcript_returns_ambiguous_on_multiple_matches() {
        // cas-900b AC (3): multiple matches → Ambiguous with all paths
        // surfaced for the supervisor to pick.
        let (_tmp, projects) = fake_projects_dir(&[
            ("-home-alice-workspace-one", &[TEST_SESSION]),
            ("-home-alice-workspace-two", &[TEST_SESSION]),
        ]);
        let got = resolve_transcript(
            Some(&projects),
            Some("/home/alice/workspace/one"),
            TEST_SESSION, cas_mux::SupervisorCli::Claude);
        match got {
            TranscriptResolution::Ambiguous {
                mut matches,
                synthesized,
                truncated,
            } => {
                assert!(!truncated, "2 < MAX_TRANSCRIPT_CANDIDATES");
                // Sort for deterministic comparison (glob order is
                // filesystem-dependent — cas-900b testing P3).
                matches.sort();
                let mut expected: Vec<_> = vec![
                    projects
                        .join("-home-alice-workspace-one")
                        .join(format!("{TEST_SESSION}.jsonl")),
                    projects
                        .join("-home-alice-workspace-two")
                        .join(format!("{TEST_SESSION}.jsonl")),
                ];
                expected.sort();
                assert_eq!(matches, expected);
                assert_eq!(
                    synthesized,
                    synthesized_transcript_path("/home/alice/workspace/one", TEST_SESSION)
                );
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn resolve_transcript_handles_unicode_clone_path() {
        // The whole point of cas-900b: a unicode cwd that the legacy
        // reconstruct would still escape (char-by-char, preserving the
        // codepoint) BUT the real CC escape might differ. With glob-first,
        // we don't care what escape CC chose — if the file exists, we find
        // it via session_id alone.
        let (_tmp, projects) = fake_projects_dir(&[("-home-usér-projet-café", &[TEST_SESSION])]);
        let got = resolve_transcript(
            Some(&projects),
            Some("/home/usér/projet/café"),
            TEST_SESSION, cas_mux::SupervisorCli::Claude);
        let expected_path = projects
            .join("-home-usér-projet-café")
            .join(format!("{TEST_SESSION}.jsonl"));
        assert_eq!(got, TranscriptResolution::Resolved(expected_path));
    }

    #[test]
    fn resolve_transcript_no_projects_dir_is_synthesized() {
        // If we can't resolve the home dir (shouldn't happen in practice),
        // the function still returns a usable Synthesized fallback.
        let got = resolve_transcript(None, Some("/home/alice/x"), TEST_SESSION, cas_mux::SupervisorCli::Claude);
        let expected = synthesized_transcript_path("/home/alice/x", TEST_SESSION);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    #[test]
    fn resolve_transcript_no_clone_path_falls_back_to_placeholder() {
        // When clone_path is None (worker registered without cwd metadata),
        // the Synthesized arm carries the placeholder label instead of a
        // reconstructed path.
        let (_tmp, projects) = fake_projects_dir(&[]);
        let got = resolve_transcript(Some(&projects), None, TEST_SESSION, cas_mux::SupervisorCli::Claude);
        let expected = synthesized_unknown_clone_path(TEST_SESSION);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    // --- cas-058f (EPIC cas-8888 Phase 4): Grok transcript resolution --------

    /// Build `<tmp>/sessions/<cwd-dir-name>/<session-uuid>/{updates.jsonl,
    /// chat_history.jsonl, signals.json}` — Grok's directory-per-session
    /// layout, structurally different from `fake_projects_dir`'s flat
    /// `<dir>/<session>.jsonl`. `cwd_dir_name` is whatever the caller wants
    /// on disk (tests pin exact encoding separately); `sessions` is a list
    /// of session-uuid directories to create under it, each populated with
    /// all three Grok files (empty).
    fn fake_grok_sessions_dir(
        dirs: &[(&str, &[&str])],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        for (cwd_dir_name, session_uuids) in dirs {
            let cwd_dir = sessions.join(cwd_dir_name);
            for uuid in *session_uuids {
                let session_dir = cwd_dir.join(uuid);
                std::fs::create_dir_all(&session_dir).unwrap();
                std::fs::write(session_dir.join("updates.jsonl"), b"").unwrap();
                std::fs::write(session_dir.join("chat_history.jsonl"), b"").unwrap();
                std::fs::write(session_dir.join("signals.json"), b"{}").unwrap();
            }
        }
        (tmp, sessions)
    }

    #[test]
    fn synthesized_grok_path_url_encodes_cwd() {
        // Grok URL-encodes the cwd (task description, VERIFIED) — a
        // structurally different escape scheme from Claude's '/'+'.'→'-'
        // collapse. Pin the exact encoding contract.
        let clone = "/home/alice/workspace one";
        let session = "064e7b23-331d-4dae-9c6a-721cbbe9c024";
        let got = synthesized_grok_transcript_path(clone, session);
        assert_eq!(
            got,
            format!("~/.grok/sessions/%2Fhome%2Falice%2Fworkspace%20one/{session}/updates.jsonl")
        );
    }

    #[test]
    fn resolve_grok_transcript_returns_resolved_on_unique_match() {
        let session = "grok-session-0000-0000-000000000000";
        let (_tmp, sessions) =
            fake_grok_sessions_dir(&[("%2Fhome%2Falice%2Fworkspace", &[session])]);
        let got = resolve_transcript(
            Some(&sessions),
            Some("/home/alice/workspace"),
            session,
            cas_mux::SupervisorCli::Grok,
        );
        let expected_path = sessions
            .join("%2Fhome%2Falice%2Fworkspace")
            .join(session)
            .join("updates.jsonl");
        assert_eq!(got, TranscriptResolution::Resolved(expected_path));
    }

    #[test]
    fn resolve_grok_transcript_returns_synthesized_on_no_match() {
        let session = "grok-session-0000-0000-000000000000";
        let (_tmp, sessions) = fake_grok_sessions_dir(&[("some-dir", &["unrelated-session"])]);
        let got = resolve_transcript(
            Some(&sessions),
            Some("/home/alice/workspace"),
            session,
            cas_mux::SupervisorCli::Grok,
        );
        let expected = synthesized_grok_transcript_path("/home/alice/workspace", session);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    #[test]
    fn resolve_grok_transcript_returns_ambiguous_on_multiple_matches() {
        let session = "grok-session-0000-0000-000000000000";
        let (_tmp, sessions) =
            fake_grok_sessions_dir(&[("dir-one", &[session]), ("dir-two", &[session])]);
        let got = resolve_transcript(
            Some(&sessions),
            Some("/home/alice/workspace"),
            session,
            cas_mux::SupervisorCli::Grok,
        );
        match got {
            TranscriptResolution::Ambiguous {
                mut matches,
                truncated,
                ..
            } => {
                assert!(!truncated);
                matches.sort();
                let mut expected = vec![
                    sessions.join("dir-one").join(session).join("updates.jsonl"),
                    sessions.join("dir-two").join(session).join("updates.jsonl"),
                ];
                expected.sort();
                assert_eq!(matches, expected);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn resolve_grok_transcript_no_sessions_dir_is_synthesized() {
        let session = "grok-session-0000-0000-000000000000";
        let got = resolve_transcript(
            None,
            Some("/home/alice/x"),
            session,
            cas_mux::SupervisorCli::Grok,
        );
        let expected = synthesized_grok_transcript_path("/home/alice/x", session);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    #[test]
    fn resolve_grok_transcript_no_clone_path_falls_back_to_placeholder() {
        let session = "grok-session-0000-0000-000000000000";
        let (_tmp, sessions) = fake_grok_sessions_dir(&[]);
        let got = resolve_transcript(Some(&sessions), None, session, cas_mux::SupervisorCli::Grok);
        let expected = synthesized_unknown_grok_clone_path(session);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    #[test]
    fn default_grok_sessions_dir_honors_grok_home_override() {
        let _lock = crate::hooks::test_env_lock();
        let old = std::env::var("GROK_HOME").ok();
        unsafe {
            std::env::set_var("GROK_HOME", "/custom/grok/home");
        }
        let got = default_grok_sessions_dir();
        unsafe {
            match &old {
                Some(v) => std::env::set_var("GROK_HOME", v),
                None => std::env::remove_var("GROK_HOME"),
            }
        }
        assert_eq!(
            got,
            Some(std::path::PathBuf::from("/custom/grok/home/sessions"))
        );
    }

    // --- cas-c655: Codex rollout resolution ---------------------------------

    /// Build `sessions/YYYY/MM/DD/rollout-...jsonl` with a `session_meta`
    /// first line carrying `cwd` — matches Codex CLI's on-disk layout.
    fn fake_codex_sessions_dir(
        rollouts: &[(&str /* relative path under sessions */, &str /* cwd */)],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let rollouts = rollouts
            .iter()
            .map(|(rel, cwd)| (*rel, *cwd, "codex-tui", "cli"))
            .collect::<Vec<_>>();
        fake_codex_sessions_dir_with_metadata(&rollouts)
    }

    fn fake_codex_sessions_dir_with_metadata(
        rollouts: &[(
            &str, /* relative path under sessions */
            &str, /* cwd */
            &str, /* originator */
            &str, /* source */
        )],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions = tmp.path().join("sessions");
        for (rel, cwd, originator, source) in rollouts {
            let path = sessions.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            let meta = serde_json::json!({
                "timestamp": "2026-07-21T12:00:00.000Z",
                "type": "session_meta",
                "payload": {
                    "session_id": "019f84af-3121-7950-ba14-b01db2dad6c7",
                    "cwd": cwd,
                    "originator": originator,
                    "source": source
                }
            });
            std::fs::write(&path, format!("{meta}\n")).unwrap();
        }
        (tmp, sessions)
    }

    #[test]
    fn default_codex_sessions_dir_honors_codex_home_override() {
        let _lock = crate::hooks::test_env_lock();
        let old = std::env::var("CODEX_HOME").ok();
        unsafe {
            std::env::set_var("CODEX_HOME", "/custom/codex/home");
        }
        let got = default_codex_sessions_dir();
        unsafe {
            match &old {
                Some(v) => std::env::set_var("CODEX_HOME", v),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
        assert_eq!(
            got,
            Some(std::path::PathBuf::from("/custom/codex/home/sessions"))
        );
    }

    #[test]
    fn resolve_codex_transcript_matches_by_cwd() {
        let clone = "/home/pippenz/Petrastella/ozer/.cas/worktrees/worker-android";
        let rel = "2026/07/21/rollout-2026-07-21T08-38-21-019f84af-3121-7950-ba14-b01db2dad6c7.jsonl";
        let (_tmp, sessions) = fake_codex_sessions_dir(&[(rel, clone)]);
        // CAS session id is NOT the rollout UUID — resolution must use cwd.
        let cas_session = "codex-worker-android-2f828ac6-deadbeefcafe";
        let got = resolve_transcript(
            Some(&sessions),
            Some(clone),
            cas_session,
            cas_mux::SupervisorCli::Codex,
        );
        let expected = sessions.join(rel);
        assert_eq!(got, TranscriptResolution::Resolved(expected));
    }

    /// Characterization for cas-fa69: worker_status resolves its live
    /// activity/context/in-flight path through `transcript_path_fast`.
    /// A real Codex rollout exists and is discoverable by the established
    /// cwd-aware resolver, so that production path must return it too.
    #[test]
    fn worker_status_transcript_path_resolves_codex_rollout_by_cwd() {
        let _lock = crate::hooks::test_env_lock();
        let clone = "/home/pippenz/Petrastella/ozer/.cas/worktrees/worker-android";
        let rel = "2026/07/21/rollout-2026-07-21T08-38-21-019f84af-3121-7950-ba14-b01db2dad6c7.jsonl";
        let (tmp, sessions) = fake_codex_sessions_dir(&[(rel, clone)]);
        let old = std::env::var("CODEX_HOME").ok();
        unsafe {
            std::env::set_var("CODEX_HOME", tmp.path());
        }
        let got = worker_status_transcript_path(
            Some(clone),
            "codex-worker-android-2f828ac6-deadbeefcafe",
            cas_mux::SupervisorCli::Codex,
        );
        unsafe {
            match old {
                Some(value) => std::env::set_var("CODEX_HOME", value),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
        assert_eq!(got, Some(sessions.join(rel)));
    }

    #[test]
    fn worker_status_transcript_path_rejects_synthesized_codex_path() {
        let (_tmp, sessions) = fake_codex_sessions_dir(&[]);
        let got = worker_status_codex_transcript_path_in(
            Some(&sessions),
            Some("/tmp/no-such-worker"),
            "codex-worker-missing",
        );
        assert_eq!(
            got, None,
            "a synthesized rollout path is not evidence that the rollout exists"
        );
    }

    #[test]
    fn worker_status_transcript_path_rejects_ambiguous_codex_cwd_matches() {
        let synthesized = "not-real".to_string();
        assert_eq!(
            worker_status_codex_path_from_resolution(TranscriptResolution::Ambiguous {
                matches: vec!["worker.jsonl".into(), "exec.jsonl".into()],
                synthesized,
                truncated: false,
            }),
            None,
            "worker_status must never invent a selection for Ambiguous resolution"
        );
    }

    #[test]
    fn resolve_codex_transcript_prefers_cli_rollout_over_exec_in_same_cwd() {
        let clone = "/tmp/codex-worker-with-exec";
        let worker_rel = "2026/07/28/rollout-2026-07-28T07-59-08-worker.jsonl";
        let exec_rel = "2026/07/28/rollout-2026-07-28T08-03-58-codex-exec.jsonl";
        let (_tmp, sessions) = fake_codex_sessions_dir_with_metadata(&[
            (worker_rel, clone, "codex-tui", "cli"),
            (exec_rel, clone, "codex_exec", "exec"),
        ]);

        assert_eq!(
            resolve_codex_transcript(
                Some(&sessions),
                Some(clone),
                "codex-worker-cas-session",
            ),
            TranscriptResolution::Resolved(sessions.join(worker_rel))
        );
    }

    #[test]
    fn resolve_codex_transcript_ignores_several_exec_rollouts() {
        let clone = "/tmp/codex-worker-with-several-execs";
        let worker_rel = "2026/07/28/rollout-2026-07-28T07-59-08-worker.jsonl";
        let (_tmp, sessions) = fake_codex_sessions_dir_with_metadata(&[
            (worker_rel, clone, "codex-tui", "cli"),
            (
                "2026/07/28/rollout-2026-07-28T08-03-58-exec-one.jsonl",
                clone,
                "codex_exec",
                "exec",
            ),
            (
                "2026/07/28/rollout-2026-07-28T08-05-12-exec-two.jsonl",
                clone,
                "codex_exec",
                "exec",
            ),
            (
                "2026/07/28/rollout-2026-07-28T08-08-44-exec-three.jsonl",
                clone,
                "codex_exec",
                "exec",
            ),
        ]);

        assert_eq!(
            resolve_codex_transcript(
                Some(&sessions),
                Some(clone),
                "codex-worker-cas-session",
            ),
            TranscriptResolution::Resolved(sessions.join(worker_rel))
        );
    }

    #[test]
    fn worker_status_activity_does_not_latch_onto_active_exec_rollout() {
        use std::io::Write;

        let _lock = crate::hooks::test_env_lock();
        let clone = "/tmp/codex-worker-active-exec";
        let worker_rel = "2026/07/28/rollout-2026-07-28T07-59-08-worker.jsonl";
        let exec_rel = "2026/07/28/rollout-2026-07-28T08-03-58-active-exec.jsonl";
        let (tmp, sessions) = fake_codex_sessions_dir_with_metadata(&[
            (worker_rel, clone, "codex-tui", "cli"),
            (exec_rel, clone, "codex_exec", "exec"),
        ]);
        let worker = sessions.join(worker_rel);
        let exec = sessions.join(exec_rel);
        let worker_mtime = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(2),
        );
        filetime::set_file_mtime(&worker, worker_mtime).unwrap();
        let mut active_exec = std::fs::OpenOptions::new()
            .append(true)
            .open(&exec)
            .unwrap();
        writeln!(
            active_exec,
            r#"{{"type":"response_item","payload":{{"type":"function_call","call_id":"exec-live","name":"shell"}}}}"#
        )
        .unwrap();

        let old = std::env::var("CODEX_HOME").ok();
        unsafe {
            std::env::set_var("CODEX_HOME", tmp.path());
        }
        let path = worker_status_transcript_path(
            Some(clone),
            "codex-worker-cas-session",
            cas_mux::SupervisorCli::Codex,
        )
        .expect("the worker rollout must resolve despite the active exec child");
        unsafe {
            match old {
                Some(value) => std::env::set_var("CODEX_HOME", value),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
        assert_eq!(path, worker, "worker_status must not latch onto codex_exec");

        let events = vec![make_event(
            cas_types::EventType::WorkerGitCommit,
            "codex-worker-cas-session",
            10 * 60,
        )];
        let (age, _) = last_worker_activity_secs_with_transcript(
            &events,
            "codex-worker-cas-session",
            cas_mux::SupervisorCli::Codex,
            Some(&path),
        )
        .expect("the worker rollout mtime is activity evidence");
        assert!(age <= 5, "activity must track the live worker rollout: {age}");
        assert!(
            !is_worker_stalled(true, Some(age), 300, false),
            "a worker active seconds ago must not be labelled STALLED"
        );
    }

    #[test]
    fn codex_rollout_kind_uses_source_then_originator_fallback() {
        assert_eq!(
            CodexRolloutMetadata {
                source: Some("cli".into()),
                originator: Some("codex-tui".into()),
                ..Default::default()
            }
            .kind(),
            CodexRolloutKind::InteractiveCli
        );
        assert_eq!(
            CodexRolloutMetadata {
                source: Some("exec".into()),
                originator: Some("codex_exec".into()),
                ..Default::default()
            }
            .kind(),
            CodexRolloutKind::Exec
        );
        assert_eq!(
            CodexRolloutMetadata {
                originator: Some("codex_exec".into()),
                ..Default::default()
            }
            .kind(),
            CodexRolloutKind::Exec,
            "legacy rollouts without source must still exclude codex_exec"
        );
        assert_eq!(
            CodexRolloutMetadata {
                originator: Some("codex_cli".into()),
                ..Default::default()
            }
            .kind(),
            CodexRolloutKind::InteractiveCli,
            "legacy codex_cli originator remains a worker candidate"
        );
    }

    #[test]
    fn worker_status_transcript_path_preserves_claude_resolution() {
        let home = tempfile::tempdir().unwrap();
        let clone = "/home/alice/project";
        let relative = synthesized_transcript_path(clone, TEST_SESSION)
            .strip_prefix("~/")
            .unwrap()
            .to_string();
        let expected = home.path().join(relative);
        std::fs::create_dir_all(expected.parent().unwrap()).unwrap();
        std::fs::write(&expected, b"").unwrap();
        let got = transcript_path_fast_in(home.path(), Some(clone), TEST_SESSION);
        assert_eq!(got, Some(expected));
    }

    #[test]
    fn worker_status_transcript_path_preserves_grok_fast_path_behavior() {
        let session = "grok-session-0000-0000-000000000000";
        let (_tmp, sessions) =
            fake_grok_sessions_dir(&[("%2Fhome%2Falice%2Fworkspace", &[session])]);
        let home = tempfile::tempdir().unwrap();
        let got = transcript_path_fast_in(
            home.path(),
            Some("/home/alice/workspace"),
            session,
        );
        assert_eq!(
            got, None,
            "the existing Grok worker_status fast path must remain unchanged"
        );
        assert!(
            sessions
                .join("%2Fhome%2Falice%2Fworkspace")
                .join(session)
                .join("updates.jsonl")
                .exists(),
            "the fixture must contain a real Grok transcript"
        );
    }

    #[test]
    fn worker_status_codex_rollout_drives_activity_and_in_flight_suppression() {
        use std::io::Write;

        let clone = "/tmp/codex-worker";
        let rel = "2026/07/28/rollout-2026-07-28T12-00-00-live.jsonl";
        let (_tmp, sessions) = fake_codex_sessions_dir(&[(rel, clone)]);
        let rollout = sessions.join(rel);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap();
        writeln!(
            file,
            r#"{{"type":"response_item","payload":{{"type":"function_call","call_id":"call-live","name":"apply_patch"}}}}"#
        )
        .unwrap();

        let path = resolve_worker_transcript_path_in(
            Some(&sessions),
            Some(clone),
            "codex-worker-live",
            cas_mux::SupervisorCli::Codex,
        )
        .expect("worker_status must resolve the live rollout");
        let events = vec![make_event(
            cas_types::EventType::WorkerGitCommit,
            "codex-worker-live",
            10 * 60,
        )];
        let (age, _) = last_worker_activity_secs_with_transcript(
            &events,
            "codex-worker-live",
            cas_mux::SupervisorCli::Codex,
            Some(&path),
        )
        .expect("rollout mtime is activity evidence");
        assert!(age <= 5, "fresh rollout must beat the stale CAS event: {age}");

        let in_flight = crate::cli::factory::wedged::transcript_has_in_flight_tool_call(
            &path,
            cas_mux::SupervisorCli::Codex,
        );
        assert!(in_flight, "unanswered Codex function call must be detected");
        assert!(
            !is_worker_stalled(true, Some(10 * 60), 300, in_flight),
            "in-flight rollout evidence must suppress the STALLED label"
        );
    }

    #[test]
    fn resolve_codex_transcript_matches_by_rollout_uuid_in_filename() {
        let clone = "/tmp/other-worktree";
        let uuid = "019f84af-3121-7950-ba14-b01db2dad6c7";
        let rel = format!("2026/07/21/rollout-2026-07-21T08-38-21-{uuid}.jsonl");
        let (_tmp, sessions) = fake_codex_sessions_dir(&[(&rel, clone)]);
        // Even with a mismatched/missing clone_path match path, filename
        // UUID lookup works when the caller has the rollout id.
        let got = resolve_transcript(
            Some(&sessions),
            Some("/tmp/unrelated"),
            uuid,
            cas_mux::SupervisorCli::Codex,
        );
        let expected = sessions.join(rel);
        assert_eq!(got, TranscriptResolution::Resolved(expected));
    }

    #[test]
    fn resolve_codex_transcript_synthesized_on_no_match() {
        let (_tmp, sessions) = fake_codex_sessions_dir(&[(
            "2026/07/21/rollout-other.jsonl",
            "/tmp/other",
        )]);
        let clone = "/tmp/missing-worker";
        let cas_session = "codex-worker-x-aaaa";
        let got = resolve_transcript(
            Some(&sessions),
            Some(clone),
            cas_session,
            cas_mux::SupervisorCli::Codex,
        );
        assert_eq!(
            got,
            TranscriptResolution::Synthesized(synthesized_codex_transcript_path(
                clone,
                cas_session
            ))
        );
    }

    #[test]
    fn resolve_codex_transcript_no_sessions_dir_is_synthesized() {
        let clone = "/tmp/w";
        let session = "codex-worker-x-aaaa";
        let got = resolve_transcript(None, Some(clone), session, cas_mux::SupervisorCli::Codex);
        assert_eq!(
            got,
            TranscriptResolution::Synthesized(synthesized_codex_transcript_path(clone, session))
        );
    }

    #[test]
    fn resolve_codex_transcript_no_clone_path_falls_back_to_placeholder() {
        let (_tmp, sessions) = fake_codex_sessions_dir(&[]);
        let session = "codex-worker-x-aaaa";
        let got = resolve_transcript(Some(&sessions), None, session, cas_mux::SupervisorCli::Codex);
        assert_eq!(
            got,
            TranscriptResolution::Synthesized(synthesized_unknown_codex_clone_path(session))
        );
    }

    #[test]
    fn codex_rollout_cwd_reads_session_meta() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout.jsonl");
        let meta = serde_json::json!({
            "type": "session_meta",
            "payload": { "cwd": "/work/tree", "session_id": "abc" }
        });
        std::fs::write(&path, format!("{meta}\n")).unwrap();
        assert_eq!(codex_rollout_cwd(&path).as_deref(), Some("/work/tree"));
    }

    #[test]
    fn codex_rollout_collection_is_bounded_and_reports_poll_cost() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let day = sessions.join("2026/07/28");
        std::fs::create_dir_all(&day).unwrap();
        let count = MAX_CODEX_ROLLOUT_SCAN + 25;
        for index in 0..count {
            let path = day.join(format!("rollout-{index:04}.jsonl"));
            std::fs::write(
                path,
                format!(
                    r#"{{"type":"session_meta","payload":{{"cwd":"/tmp/worker-{index}"}}}}"#
                ),
            )
            .unwrap();
        }

        let started = std::time::Instant::now();
        let candidates = collect_codex_rollouts(&sessions);
        let elapsed = started.elapsed();
        eprintln!(
            "cas-fa69 resolver poll: {count} rollouts on disk, {} candidates retained, {}µs",
            candidates.len(),
            elapsed.as_micros()
        );
        assert_eq!(candidates.len(), MAX_CODEX_ROLLOUT_SCAN);
    }

    #[test]
    fn worker_cli_from_agent_parses_grok_metadata() {
        let mut agent = cas_types::Agent::new("sess-1".to_string(), "grok-worker".to_string());
        agent
            .metadata
            .insert("worker_cli".to_string(), "grok".to_string());
        assert_eq!(worker_cli_from_agent(&agent), cas_mux::SupervisorCli::Grok);
    }

    #[test]
    fn worker_cli_from_agent_defaults_to_claude_when_missing_or_invalid() {
        let agent = cas_types::Agent::new("sess-1".to_string(), "legacy-worker".to_string());
        assert_eq!(
            worker_cli_from_agent(&agent),
            cas_mux::SupervisorCli::Claude,
            "no worker_cli metadata (legacy agent) must default to Claude"
        );

        let mut bad_agent = cas_types::Agent::new("sess-2".to_string(), "bad-worker".to_string());
        bad_agent
            .metadata
            .insert("worker_cli".to_string(), "not-a-real-cli".to_string());
        assert_eq!(
            worker_cli_from_agent(&bad_agent),
            cas_mux::SupervisorCli::Claude,
            "an unparseable worker_cli value must default to Claude, not panic"
        );
    }

    #[test]
    fn glob_candidates_returns_empty_on_missing_projects_dir() {
        // Glob on a nonexistent path must not panic — just return empty.
        let missing = std::path::Path::new("/tmp/does-not-exist-cas-900b");
        let (got, truncated) = glob_transcript_candidates(missing, TEST_SESSION);
        assert!(got.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn glob_candidates_escapes_session_id_metachars() {
        // cas-900b adversarial P1: a rogue session_id containing glob
        // metacharacters (`*`, `?`, `[`) must not broaden the match and
        // surface unrelated transcripts. We create a "real" file at
        // `*.jsonl` (by using a sentinel session_id for the fake dir)
        // plus noise files, and glob for the literal `*` session id;
        // only a file whose stem is literally `*` should come back, and
        // in this layout there is none, so the result is empty.
        let (_tmp, projects) = fake_projects_dir(&[
            ("-home-alice-one", &[TEST_SESSION, "another-session"]),
            ("-home-alice-two", &["yet-another"]),
        ]);
        // A malicious session_id: `*` would, if unescaped, match every
        // .jsonl under every project dir. With the fix, glob::Pattern::escape
        // turns it into `[*]` (glob literal) so it only matches a file
        // literally named `*.jsonl` — which doesn't exist here.
        let (got, _) = glob_transcript_candidates(&projects, "*");
        assert!(
            got.is_empty(),
            "escaped `*` must not match arbitrary .jsonl files; got {got:?}"
        );
    }

    #[test]
    fn glob_candidates_truncates_at_max() {
        // cas-900b adversarial P1: bound latency under high-cardinality
        // layouts. Build a layout with MAX+5 matches and confirm the
        // truncated flag fires and the vec length stops at MAX.
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        for i in 0..(MAX_TRANSCRIPT_CANDIDATES + 5) {
            let d = projects.join(format!("proj-{i}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("{TEST_SESSION}.jsonl")), b"").unwrap();
        }
        let (got, truncated) = glob_transcript_candidates(&projects, TEST_SESSION);
        assert_eq!(got.len(), MAX_TRANSCRIPT_CANDIDATES);
        assert!(truncated, "MAX+5 inputs must trip the truncated flag");
    }

    #[test]
    fn render_transcript_block_resolved_contains_session_and_path() {
        let path = std::path::PathBuf::from("/home/u/.claude/projects/x/ses.jsonl");
        let got = render_transcript_block(
            &TranscriptResolution::Resolved(path.clone()),
            TEST_SESSION,
            true,
        );
        assert!(got.contains("Transcript: /home/u/.claude/projects/x/ses.jsonl"));
        assert!(got.contains(&format!("Session: {TEST_SESSION}")));
        assert!(!got.contains("Likely"));
    }

    #[test]
    fn render_transcript_block_synthesized_labels_likely_and_surfaces_session() {
        let synth = "~/.claude/projects/-home-x/ses.jsonl".to_string();
        let got = render_transcript_block(
            &TranscriptResolution::Synthesized(synth.clone()),
            TEST_SESSION,
            true,
        );
        assert!(got.contains(&format!("Likely transcript: {synth}")));
        assert!(got.contains(&format!("Session: {TEST_SESSION}")));
    }

    #[test]
    fn render_transcript_block_synthesized_no_home_notes_skipped_glob() {
        // cas-900b adversarial P3: distinguish home-dir failure from
        // clone-path failure.
        let synth = synthesized_unknown_clone_path(TEST_SESSION);
        let got = render_transcript_block(
            &TranscriptResolution::Synthesized(synth),
            TEST_SESSION,
            false,
        );
        assert!(got.contains("home dir unresolvable"));
        assert!(got.contains("glob skipped"));
    }

    #[test]
    fn render_transcript_block_ambiguous_lists_candidates_with_session_and_fallback() {
        let matches = vec![
            std::path::PathBuf::from("/p/a/ses.jsonl"),
            std::path::PathBuf::from("/p/b/ses.jsonl"),
        ];
        let synthesized = "~/.claude/projects/-p-a/ses.jsonl".to_string();
        let got = render_transcript_block(
            &TranscriptResolution::Ambiguous {
                matches,
                synthesized: synthesized.clone(),
                truncated: false,
            },
            TEST_SESSION,
            true,
        );
        assert!(got.contains(&format!("candidates (session {TEST_SESSION})")));
        assert!(got.contains("- /p/a/ses.jsonl"));
        assert!(got.contains("- /p/b/ses.jsonl"));
        assert!(got.contains(&format!("Likely synthesized: {synthesized}")));
        assert!(!got.contains("truncated"));
    }

    #[test]
    fn render_transcript_block_ambiguous_truncated_notes_cap() {
        let got = render_transcript_block(
            &TranscriptResolution::Ambiguous {
                matches: vec![std::path::PathBuf::from("/p/a/ses.jsonl")],
                synthesized: "<s>".to_string(),
                truncated: true,
            },
            TEST_SESSION,
            true,
        );
        assert!(got.contains("truncated"));
        assert!(got.contains(&format!("{MAX_TRANSCRIPT_CANDIDATES}")));
    }

    /// cas-85bf: worker_status output must include session UUID alongside the
    /// friendly worker name so supervisors can cross-reference task-ownership
    /// errors ("owned by worker-backfill (0a7f2802-...)") without extra lookups.
    ///
    /// This test exercises the format string in `factory_worker_status` by
    /// manually building the string the same way the production code does and
    /// asserting the UUID is embedded.
    #[test]
    fn test_worker_status_format_includes_session_uuid() {
        const NAME: &str = "worker-backfill";
        const UUID: &str = "0a7f2802-e977-493b-965b-c620e99f04ef";

        // Reproduce the format! call from factory_worker_status (cas-85bf +
        // cas-573c + cas-844bf): git_info is the 4th positional arg,
        // context_info is the 6th, transcript_info is the 5th.
        let output = format!(
            "  • {} (heartbeat: {}){}{}{}{}{}{}{}\n    session: {}\n",
            NAME, "5s ago", "", "", "", "", "\n    model: sonnet\n    effort: medium", "", "", UUID
        );

        assert!(
            output.contains(NAME),
            "output must contain worker name: {output}"
        );
        assert!(
            output.contains(UUID),
            "output must contain session UUID: {output}"
        );
        assert!(
            output.contains("session:"),
            "output must have 'session:' label: {output}"
        );
        assert!(
            output.contains("model: sonnet") && output.contains("effort: medium"),
            "output must contain worker model and effort: {output}"
        );
    }

    // ---- cas-1ec7: active-IO stale suppression ----------------------------

    /// AC: a stale-heartbeat worker with a recent WorkerFileEdited event is
    /// NOT reported as having no recent I/O.
    #[test]
    fn has_recent_worker_io_activity_matches_file_edited() {
        use cas_types::{Event, EventEntityType, EventType};
        let mut e = Event::new(
            EventType::WorkerFileEdited,
            EventEntityType::Agent,
            "vivid-dolphin-10",
            "edited src/lib.rs",
        );
        e.session_id = Some("ses-abc-123".to_string());
        assert!(
            has_recent_worker_io_activity(&[e], "ses-abc-123"),
            "WorkerFileEdited with matching session_id must report active I/O"
        );
    }

    /// AC: a stale-heartbeat worker with a recent WorkerGitCommit event is
    /// NOT reported as having no recent I/O.
    #[test]
    fn has_recent_worker_io_activity_matches_git_commit() {
        use cas_types::{Event, EventEntityType, EventType};
        let mut e = Event::new(
            EventType::WorkerGitCommit,
            EventEntityType::Agent,
            "rapid-shark-56",
            "committed feat: add widget",
        );
        e.session_id = Some("ses-def-456".to_string());
        assert!(
            has_recent_worker_io_activity(&[e], "ses-def-456"),
            "WorkerGitCommit with matching session_id must report active I/O"
        );
    }

    /// AC: a worker with stale heartbeat AND no recent activity remains stale.
    /// Verified by: no matching events → function returns false → caller prunes.
    #[test]
    fn has_recent_worker_io_activity_no_match_wrong_session() {
        use cas_types::{Event, EventEntityType, EventType};
        let mut e = Event::new(
            EventType::WorkerFileEdited,
            EventEntityType::Agent,
            "other-worker",
            "edited file.rs",
        );
        e.session_id = Some("ses-other".to_string());
        assert!(
            !has_recent_worker_io_activity(&[e], "ses-mine"),
            "event for a different session_id must not suppress pruning for this agent"
        );
    }

    #[test]
    fn has_recent_worker_io_activity_no_match_wrong_event_type() {
        use cas_types::{Event, EventEntityType, EventType};
        let mut e = Event::new(
            EventType::TaskStarted,
            EventEntityType::Agent,
            "worker-x",
            "task started",
        );
        e.session_id = Some("ses-abc-123".to_string());
        assert!(
            !has_recent_worker_io_activity(&[e], "ses-abc-123"),
            "non-IO event type (TaskStarted) must not count as active I/O"
        );
    }

    #[test]
    fn has_recent_worker_io_activity_empty_events_returns_false() {
        assert!(
            !has_recent_worker_io_activity(&[], "ses-any"),
            "empty event list must report no active I/O"
        );
    }

    #[test]
    fn has_recent_worker_io_activity_no_session_id_does_not_match() {
        use cas_types::{Event, EventEntityType, EventType};
        // Event without session_id set — should not match any agent.
        let e = Event::new(
            EventType::WorkerFileEdited,
            EventEntityType::Agent,
            "ses-abc-123", // entity_id carries the name, but session_id is None
            "edited foo.rs",
        );
        // session_id is None by default from Event::new
        assert!(
            !has_recent_worker_io_activity(&[e], "ses-abc-123"),
            "event without session_id must not match (matching is by session_id, not entity_id)"
        );
    }

    // Process-alive unit tests live in agent_liveness::tests (cas-e98e).

    // ---- cas-573c: context-usage band + tail reader -----------------------

    #[test]
    fn context_band_ok_below_50_pct() {
        assert_eq!(context_band(0), "ok");
        assert_eq!(context_band(49_999), "ok");
        // 99_999 / 200_000 * 100 = 49 → ok
        assert_eq!(context_band(99_999), "ok");
    }

    #[test]
    fn context_band_approaching_50_to_79_pct() {
        // 100_000 / 200_000 * 100 = 50 → approaching
        assert_eq!(context_band(100_000), "approaching");
        assert_eq!(context_band(150_000), "approaching");
        // 159_999 / 200_000 * 100 = 79 → approaching
        assert_eq!(context_band(159_999), "approaching");
    }

    #[test]
    fn context_band_near_limit_at_80_pct_and_above() {
        // 160_000 / 200_000 * 100 = 80 → near-limit
        assert_eq!(context_band(160_000), "near-limit");
        assert_eq!(context_band(200_000), "near-limit");
        assert_eq!(context_band(210_000), "near-limit");
    }

    /// `read_context_usage_from_tail` must extract the correct total from a
    /// minimal JSONL snippet that matches the real CC session format.
    #[test]
    fn read_context_usage_from_tail_extracts_usage_sum() {
        use std::io::Write;

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path();

        // Write a minimal assistant entry matching the CC session JSONL format.
        // Total = 1000 (input) + 5000 (cache_create) + 2000 (cache_read) = 8000.
        writeln!(
            tmp.as_file(),
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":1000,"cache_creation_input_tokens":5000,"cache_read_input_tokens":2000,"output_tokens":50}}}}}}"#
        )
        .unwrap();

        let total = read_context_usage_from_tail(path).expect("should parse usage");
        assert_eq!(total, 8_000, "input+cache_create+cache_read = 8000");
    }

    #[test]
    fn read_context_usage_from_codex_rollout_extracts_latest_turn_input() {
        use std::io::Write;

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        writeln!(
            tmp.as_file(),
            r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":123456,"cached_input_tokens":100000}},"model_context_window":258400}}}}}}"#
        )
        .unwrap();

        let total =
            read_context_usage_from_tail_for_cli(tmp.path(), cas_mux::SupervisorCli::Codex)
                .expect("Codex token_count event should produce a context reading");
        assert_eq!(total, 123_456);
        assert_eq!(context_band(total), "approaching");
    }

    /// When the tail has multiple assistant entries, the LAST one wins.
    #[test]
    fn read_context_usage_from_tail_takes_last_entry() {
        use std::io::Write;

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path();

        // First entry: total 1000. Second entry: total 9000 → should win.
        writeln!(
            tmp.as_file(),
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":1000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":10}}}}}}"#
        )
        .unwrap();
        writeln!(
            tmp.as_file(),
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":3000,"cache_creation_input_tokens":4000,"cache_read_input_tokens":2000,"output_tokens":20}}}}}}"#
        )
        .unwrap();

        let total = read_context_usage_from_tail(path).expect("should parse usage");
        assert_eq!(total, 9_000, "last entry's total should win");
    }

    /// Non-assistant entries (user, attachment) are skipped.
    #[test]
    fn read_context_usage_from_tail_skips_non_assistant_entries() {
        use std::io::Write;

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path();

        writeln!(
            tmp.as_file(),
            r#"{{"type":"user","message":{{"content":"hello"}}}}"#
        )
        .unwrap();
        writeln!(tmp.as_file(), r#"{{"type":"attachment","content":"data"}}"#).unwrap();

        let total = read_context_usage_from_tail(path);
        assert!(total.is_none(), "no assistant entry → should return None");
    }

    /// Missing file returns None gracefully.
    #[test]
    fn read_context_usage_from_tail_missing_file_returns_none() {
        let path = std::path::Path::new("/tmp/cas_573c_nonexistent_fixture.jsonl");
        assert!(read_context_usage_from_tail(path).is_none());
    }

    // ---- cas-844bf: worker_status git introspection -------------------------

    /// Helper: create a minimal git repo in a temp dir and return the TempDir.
    /// Initialises `main`, adds an initial commit, then creates and checks out
    /// `factory/<worker>` with one additional commit.
    fn setup_git_repo_with_factory_branch(worker: &str) -> (tempfile::TempDir, String) {
        use std::process::Command;
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let dir = tmp.path();

        // Init
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@cas"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "CAS Test"])
            .current_dir(dir)
            .output()
            .unwrap();

        // Initial commit on main
        std::fs::write(dir.join("README"), "init").unwrap();
        Command::new("git")
            .args(["add", "README"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir)
            .output()
            .unwrap();

        // Create factory branch and make one commit
        let branch = format!("factory/{worker}");
        Command::new("git")
            .args(["checkout", "-b", &branch])
            .current_dir(dir)
            .output()
            .unwrap();
        std::fs::write(dir.join("work.rs"), "// task").unwrap();
        Command::new("git")
            .args(["add", "work.rs"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "feat: worker work"])
            .current_dir(dir)
            .output()
            .unwrap();

        // Get the short SHA for assertions
        let sha_out = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();

        (tmp, sha)
    }

    fn setup_factory_project_with_worker_worktrees(
        workers: &[&str],
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::process::Command;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&project)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@cas"])
            .current_dir(&project)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "CAS Test"])
            .current_dir(&project)
            .output()
            .unwrap();

        std::fs::write(project.join("README"), "init").unwrap();
        Command::new("git")
            .args(["add", "README"])
            .current_dir(&project)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&project)
            .output()
            .unwrap();

        let cas_root = project.join(".cas");
        let worktree_root = cas_root.join("worktrees");
        std::fs::create_dir_all(&worktree_root).unwrap();

        for worker in workers {
            let branch = format!("factory/{worker}");
            let worktree_path = worktree_root.join(worker);
            let status = Command::new("git")
                .args([
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    worktree_path.to_str().unwrap(),
                    "HEAD",
                ])
                .current_dir(&project)
                .status()
                .unwrap();
            assert!(
                status.success(),
                "git worktree add must succeed for {worker}"
            );
        }

        (tmp, project)
    }

    /// AC3 (cas-844bf): collect_worker_git_status returns the correct branch
    /// and HEAD SHA for a worktree that is on a real factory/<name> branch with
    /// at least one commit.
    ///
    /// FAILS with the stub (branch == "?", head_sha == "?").
    /// PASSES once the real implementation runs git commands.
    #[test]
    fn collect_git_status_returns_correct_branch_and_sha() {
        let (tmp, expected_sha) = setup_git_repo_with_factory_branch("test-worker");
        let status = collect_worker_git_status(tmp.path());
        assert_eq!(
            status.branch, "factory/test-worker",
            "branch must be 'factory/test-worker', got '{}'",
            status.branch
        );
        assert_eq!(
            status.head_sha, expected_sha,
            "head_sha must match git rev-parse --short HEAD"
        );
    }

    /// AC1 (cas-844bf): format_worker_git_status output must contain the
    /// structured fields the supervisor needs — branch, HEAD, ahead/behind,
    /// dirty state, pushed ref, and PR URL.
    ///
    /// FAILS with the stub (returns empty string).
    /// PASSES once the real formatter produces the labelled output.
    #[test]
    fn format_git_status_contains_required_fields() {
        let gs = WorkerGitStatus {
            branch: "factory/myworker".to_string(),
            head_sha: "abc1234".to_string(),
            ahead: 3,
            behind: 0,
            base_branch: "origin/main".to_string(),
            dirty: false,
            pushed_ref: "origin/factory/myworker".to_string(),
            pr_url: "https://github.com/org/repo/pull/42".to_string(),
        };
        let out = format_worker_git_status(&gs);
        assert!(
            !out.is_empty(),
            "format_worker_git_status must not return empty string"
        );
        assert!(
            out.contains("factory/myworker"),
            "must contain branch name: {out}"
        );
        assert!(out.contains("abc1234"), "must contain HEAD sha: {out}");
        assert!(out.contains("ahead"), "must contain 'ahead' label: {out}");
        assert!(out.contains("behind"), "must contain 'behind' label: {out}");
        assert!(out.contains("PR"), "must contain 'PR' label: {out}");
        assert!(
            out.contains("origin/factory/myworker"),
            "must contain pushed_ref: {out}"
        );
        assert!(
            out.contains("https://github.com"),
            "must contain PR URL: {out}"
        );
    }

    /// AC2 (cas-844bf): when gh is unavailable / not pushed, pr_url and
    /// pushed_ref degrade gracefully to "none" without panicking.
    #[test]
    fn collect_git_status_degrades_on_non_git_dir() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        // Plain dir with no git repo — all fields must be sentinels, no panic.
        let status = collect_worker_git_status(tmp.path());
        // branch and head_sha degrade to "?" (not empty, not a real branch name)
        assert!(
            status.branch == "?" || !status.branch.contains("fatal"),
            "non-git dir must not propagate git error messages: '{}'",
            status.branch
        );
        // No panics is the primary assertion — the above implicitly proves it.
    }

    /// AC1 (format): dirty/not-pushed state is conveyed clearly.
    #[test]
    fn format_git_status_dirty_not_pushed() {
        let gs = WorkerGitStatus {
            branch: "factory/dirty".to_string(),
            head_sha: "deadbee".to_string(),
            ahead: 0,
            behind: 2,
            base_branch: "origin/main".to_string(),
            dirty: true,
            pushed_ref: "none".to_string(),
            pr_url: "none".to_string(),
        };
        let out = format_worker_git_status(&gs);
        assert!(
            !out.is_empty(),
            "format must not return empty for dirty worker: {out}"
        );
        assert!(
            out.contains("dirty") || out.contains("[dirty]"),
            "dirty state must be visible in output: {out}"
        );
        assert!(
            out.contains("not pushed") || out.contains("none"),
            "unpushed state must be visible in output: {out}"
        );
    }

    // --- cas-f53c: sync_all_workers clone_path resolution -----------------

    #[test]
    fn resolve_worker_clone_path_uses_convention_when_metadata_absent_and_worktree_exists() {
        let (_tmp, project) = setup_factory_project_with_worker_worktrees(&["recipes-fixer"]);
        let cas_root = project.join(".cas");
        let expected = cas_root.join("worktrees/recipes-fixer");
        let agent = cas_types::Agent::new_with_role(
            "session-1".to_string(),
            "recipes-fixer".to_string(),
            AgentRole::Worker,
        );
        // No clone_path metadata — the post-spawn race in the bug report.
        assert!(!agent.metadata.contains_key("clone_path"));

        match resolve_worker_clone_path(&cas_root, &agent) {
            WorkerClonePathResolve::Ready(path) => {
                assert_eq!(path, expected);
            }
            other => panic!("expected Ready(convention path), got {other:?}"),
        }
    }

    #[test]
    fn resolve_worker_clone_path_prefers_existing_metadata_over_convention() {
        let (_tmp, project) =
            setup_factory_project_with_worker_worktrees(&["named-a", "named-b"]);
        let cas_root = project.join(".cas");
        let meta_path = cas_root.join("worktrees/named-b");
        let mut agent = cas_types::Agent::new_with_role(
            "session-1".to_string(),
            "named-a".to_string(),
            AgentRole::Worker,
        );
        agent.metadata.insert(
            "clone_path".to_string(),
            meta_path.to_string_lossy().to_string(),
        );

        match resolve_worker_clone_path(&cas_root, &agent) {
            WorkerClonePathResolve::Ready(path) => assert_eq!(path, meta_path),
            other => panic!("expected Ready(metadata path), got {other:?}"),
        }
    }

    #[test]
    fn resolve_worker_clone_path_not_on_disk_when_neither_exists() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let cas_root = tmp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let agent = cas_types::Agent::new_with_role(
            "session-1".to_string(),
            "ghost-worker".to_string(),
            AgentRole::Worker,
        );

        match resolve_worker_clone_path(&cas_root, &agent) {
            WorkerClonePathResolve::NotOnDisk {
                candidate,
                had_metadata,
            } => {
                assert!(!had_metadata);
                assert_eq!(candidate, cas_root.join("worktrees/ghost-worker"));
            }
            other => panic!("expected NotOnDisk, got {other:?}"),
        }
    }

    #[test]
    fn sync_skip_reason_is_retryable_when_metadata_missing_and_no_worktree() {
        let resolve = WorkerClonePathResolve::NotOnDisk {
            candidate: std::path::PathBuf::from("/proj/.cas/worktrees/w1"),
            had_metadata: false,
        };
        let msg = sync_skip_reason_for_clone_resolve("w1", &resolve).expect("skip reason");
        assert!(
            msg.contains("registration in progress") || msg.contains("retry"),
            "must be retryable, not silent missing-metadata: {msg}"
        );
        assert!(
            !msg.contains("missing clone_path metadata"),
            "old skip text must not return: {msg}"
        );
        assert!(sync_skip_reason_for_clone_resolve(
            "w1",
            &WorkerClonePathResolve::Ready(std::path::PathBuf::from("/x"))
        )
        .is_none());
    }

    #[test]
    fn worker_status_infers_missing_codex_clone_path_from_factory_worktree_layout() {
        let (_tmp, project) =
            setup_factory_project_with_worker_worktrees(&["claude-jester", "codex-jester"]);
        let cas_root = project.join(".cas");
        let claude_path = cas_root.join("worktrees/claude-jester");
        let codex_path = cas_root.join("worktrees/codex-jester");

        let mut claude = cas_types::Agent::new_with_role(
            "claude-session".to_string(),
            "claude-jester".to_string(),
            AgentRole::Worker,
        );
        claude.metadata.insert(
            "clone_path".to_string(),
            claude_path.to_string_lossy().to_string(),
        );
        let codex = cas_types::Agent::new_with_role(
            "codex-session".to_string(),
            "codex-jester".to_string(),
            AgentRole::Worker,
        );

        let claude_status = collect_worker_worktree_status(&cas_root, &claude);
        let codex_status = collect_worker_worktree_status(&cas_root, &codex);

        assert_eq!(
            codex_status.clone_path.as_deref(),
            Some(codex_path.to_string_lossy().as_ref()),
            "Codex worker must infer the on-disk worktree path when metadata is absent"
        );
        for (name, status) in [("claude", claude_status), ("codex", codex_status)] {
            assert!(
                status.clone_info.contains("Clone:"),
                "{name} record must include Clone info: {:?}",
                status
            );
            assert!(
                status.git_info.contains("git: factory/"),
                "{name} record must include git branch metadata: {:?}",
                status
            );
            assert!(
                status.git_info.contains("[clean]"),
                "{name} record must include cleanliness metadata: {:?}",
                status
            );
        }
    }

    #[test]
    fn worker_status_reports_missing_worktree_instead_of_omitting_clone_metadata() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let cas_root = tmp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let agent = cas_types::Agent::new_with_role(
            "codex-session".to_string(),
            "codex-jester".to_string(),
            AgentRole::Worker,
        );

        let status = collect_worker_worktree_status(&cas_root, &agent);
        let expected_path = cas_root.join("worktrees/codex-jester");
        let expected_path_str = expected_path.to_string_lossy().to_string();

        assert_eq!(
            status.clone_path.as_deref(),
            Some(expected_path_str.as_str())
        );
        assert!(
            status.clone_info.contains("[missing-worktree]"),
            "missing worktree state must be explicit: {:?}",
            status
        );
        assert!(
            status.clone_info.contains(&expected_path_str),
            "the expected worktree path must still be surfaced: {:?}",
            status
        );
        assert_eq!(status.git_info, "\n    git: missing-worktree");
    }
}
