use crate::mcp::tools::core::workflow::verification_tools::VERIFICATION_REJECTED_REOPEN_LABEL;
use crate::mcp::tools::service::imports::*;
use crate::opencode_preflight::{
    OpenCodeRoute, hosted_lane_for_selector, hosted_serving_identity, opencode_route_for_selector,
    preflight_hosted_from_env, require_supported_selector, validate_hosted_effort_for_lane,
};
use cas_factory::routing::{
    default_worker_effort_for_cli, default_worker_model_for_cli, resolve_lane_specs,
    validate_lane_request,
};

const HOSTED_PREFLIGHT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Validate every distinct hosted selector before it reaches the spawn queue.
/// Local selectors never enter this function, so a missing or invalid hosted
/// key cannot block the independently usable local route.
fn preflight_hosted_opencode_specs(specs: &[cas_mux::WorkerSpec]) -> Result<(), String> {
    let mut selectors = std::collections::BTreeSet::new();
    for spec in specs {
        if spec.cli != cas_mux::SupervisorCli::OpenCode {
            continue;
        }
        let Some(model) = spec.model.as_deref() else {
            continue;
        };
        require_supported_selector(model)?;
        match opencode_route_for_selector(model)? {
            OpenCodeRoute::HostedTokenPlan | OpenCodeRoute::HostedPayg => {
                selectors.insert(model.to_string());
            }
            OpenCodeRoute::Local => {}
            OpenCodeRoute::Hosted => {
                return Err(
                    "legacy OpenCode hosted route cannot pass the support-claim gate".to_string(),
                );
            }
        }
    }
    for selector in selectors {
        preflight_hosted_from_env(&selector, HOSTED_PREFLIGHT_TIMEOUT)?;
    }
    Ok(())
}

/// Whether this harness has account-directory plumbing at all.
///
/// Claude scopes accounts with `CLAUDE_CONFIG_DIR`, Codex with `CODEX_HOME`
/// (cas-9cc3). Grok has no equivalent yet, so a config_dir aimed at a grok
/// worker is reported rather than silently dropped.
fn account_dir_supported(cli: cas_mux::SupervisorCli) -> bool {
    matches!(
        cli,
        cas_mux::SupervisorCli::Claude | cas_mux::SupervisorCli::Codex
    )
}

/// The requesting supervisor's own account directory for this harness.
///
/// Provider-aware on purpose: applying a Claude supervisor's
/// `CLAUDE_CONFIG_DIR` to a codex worker would point `CODEX_HOME` at a
/// `.claude-*` directory, which is worse than no selection at all.
fn requester_account_dir(cli: cas_mux::SupervisorCli) -> Option<String> {
    match cli {
        cas_mux::SupervisorCli::Claude => std::env::var("CLAUDE_CONFIG_DIR").ok(),
        cas_mux::SupervisorCli::Codex => std::env::var("CODEX_HOME").ok(),
        _ => None,
    }
}

/// The requesting supervisor's independent Claude secure-storage selector.
/// This is intentionally captured separately from `CLAUDE_CONFIG_DIR`: an
/// operator may select a config profile while retaining a credential store
/// elsewhere, and unset/empty/set must survive the queue boundary distinctly.
fn requester_secure_storage_dir(cli: cas_mux::SupervisorCli) -> Option<String> {
    match cli {
        cas_mux::SupervisorCli::Claude => std::env::var("CLAUDE_SECURESTORAGE_CONFIG_DIR").ok(),
        _ => None,
    }
}

/// Resolve and validate an explicit Claude config directory before its spawn
/// request reaches the daemon.  A partial profile otherwise starts a PTY that
/// cannot load Cassy' worker contract, then fails sixty seconds later with no
/// actionable cause (GH #270).
fn preflight_claude_config_dir(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("config_dir is empty; expected a Claude configuration directory".to_string());
    }
    let path = trimmed.strip_prefix('~').map_or_else(
        || std::path::PathBuf::from(trimmed),
        |suffix| {
            dirs::home_dir()
                .map(|home| home.join(suffix.trim_start_matches('/')))
                .unwrap_or_else(|| std::path::PathBuf::from(trimmed))
        },
    );
    let required_files = [
        ("settings.json", path.join("settings.json")),
        (".credentials.json", path.join(".credentials.json")),
    ];
    for (name, candidate) in required_files {
        std::fs::File::open(&candidate).map_err(|error| {
            format!("config_dir preflight failed: missing or unreadable {name}: {error}")
        })?;
    }
    for name in ["agents", "skills"] {
        let candidate = path.join(name);
        let resolved = candidate.canonicalize().map_err(|error| {
            format!("config_dir preflight failed: missing or unresolved {name}: {error}")
        })?;
        if !resolved.is_dir() {
            return Err(format!(
                "config_dir preflight failed: {name} must resolve to a directory"
            ));
        }
    }
    Ok(())
}

/// Resolve and validate an explicit Codex account directory (a `CODEX_HOME`
/// override) before its spawn request reaches the daemon (cas-4a5e).
///
/// The Codex analog of [`preflight_claude_config_dir`], but deliberately a
/// SEPARATE, harder failure than [`cas_factory::apply_codex_fallback`]'s
/// generic "codex isn't available anywhere, fall back to claude" path: an
/// explicit `config_dir` is the caller naming ONE specific account, not
/// asking "is codex usable at all" — a typo'd or wrong directory must fail
/// here, by name, rather than being silently absorbed into a rewrite to
/// `claude`. Before this existed, that silent rewrite is exactly how the
/// original incident produced a triply misleading error: a codex account
/// directory surviving onto a claude-rewritten spec, checked by
/// [`preflight_claude_config_dir`] against files (`settings.json`,
/// `.credentials.json`) a codex directory never has — wrong provider, wrong
/// file, wrong cause. Call this BEFORE `apply_codex_fallback` so a bad
/// explicit codex dir is rejected outright instead of ever reaching that
/// rewrite.
///
/// Only checks `auth.json` directly under the (tilde-expanded) directory —
/// the `CODEX_HOME` layout `push_codex_home_env` (cas-pty) actually spawns
/// with, NOT the `~/.codex` default-home layout.
fn preflight_codex_config_dir(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("config_dir is empty; expected a Codex CODEX_HOME directory".to_string());
    }
    let path = trimmed.strip_prefix('~').map_or_else(
        || std::path::PathBuf::from(trimmed),
        |suffix| {
            dirs::home_dir()
                .map(|home| home.join(suffix.trim_start_matches('/')))
                .unwrap_or_else(|| std::path::PathBuf::from(trimmed))
        },
    );
    let auth_path = path.join("auth.json");
    // `File::open` on a plain directory succeeds on Linux (you can open a
    // directory fd read-only) — an explicit `is_file()` metadata check is
    // required here, matching `codex_auth_present_at`'s probe semantics, or
    // a directory literally named `auth.json` would misread as "logged in".
    match std::fs::metadata(&auth_path) {
        Ok(meta) if meta.is_file() => Ok(()),
        Ok(_) => Err(format!(
            "config_dir preflight failed: {} exists but is not a file (expected Codex's auth.json)",
            auth_path.display()
        )),
        Err(error) => Err(format!(
            "config_dir preflight failed: missing or unreadable auth.json at {}: {error} \
             (expected a Codex CODEX_HOME directory — run `codex login` under this \
             CODEX_HOME, or check for a typo in config_dir)",
            auth_path.display()
        )),
    }
}

/// Bound on the account probes run before a spawn cuts a worktree. Two
/// harnesses at two seconds each is the worst case, and a spawn that waits
/// that long still beats a worker that heartbeats for half an hour without
/// ever taking a turn.
const ACCOUNT_PREFLIGHT_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// Refuse a spawn whose harness account is logged out, before any worktree is
/// cut (cas-8a55).
///
/// `preflight_codex_config_dir` proves an `auth.json` exists; it cannot prove
/// the credential inside it still works. In the incident this exists for the
/// file was present and the refresh token behind it had been revoked, so four
/// worktrees were cut, four workers registered, and every first turn died in
/// about a second.
///
/// Only an affirmative `Unavailable` refuses. `Unknown` — no CLI on PATH, a
/// probe that timed out, a harness with no account plumbing — must not block a
/// spawn: a preflight that fails closed on its own unreliability would ground
/// the factory over a slow binary.
fn preflight_account_auth(specs: &[cas_mux::WorkerSpec]) -> Result<(), String> {
    let deadline = crate::bounded_process::Deadline::after(ACCOUNT_PREFLIGHT_BUDGET);
    preflight_account_auth_with(specs, |cli, account_dir| {
        crate::capability::probe_account_auth(cli, account_dir, deadline)
    })
}

/// Probe-injected core of [`preflight_account_auth`], so the refusal policy is
/// testable without a CLI on PATH.
fn preflight_account_auth_with(
    specs: &[cas_mux::WorkerSpec],
    probe: impl Fn(cas_mux::SupervisorCli, Option<&str>) -> cas_factory::CapabilityEvidence,
) -> Result<(), String> {
    let mut seen: std::collections::BTreeSet<(cas_mux::SupervisorCli, Option<String>)> =
        std::collections::BTreeSet::new();
    for spec in specs {
        if !account_dir_supported(spec.cli) {
            continue;
        }
        // The account a worker will actually use: its own override first, the
        // requesting supervisor's captured account otherwise.
        let account_dir = spec
            .config_dir
            .clone()
            .or_else(|| spec.requester_config_dir.clone())
            .map(|dir| dir.trim().to_string())
            .filter(|dir| !dir.is_empty());
        if !seen.insert((spec.cli, account_dir.clone())) {
            continue;
        }
        let evidence = probe(spec.cli, account_dir.as_deref());
        if evidence.availability != cas_factory::CapabilityAvailability::Unavailable {
            continue;
        }
        let account = account_dir
            .as_deref()
            .map_or_else(|| default_account_label(spec.cli), str::to_string);
        let reason = evidence
            .reason
            .unwrap_or_else(|| "the account probe reported it unavailable".to_string());
        return Err(format!(
            "spawn refused: the {} account at {account} is not usable — {reason}. {} \
             No worktree was created and no task was assigned.",
            spec.cli.backend().name(),
            crate::factory_auth_health::auth_failure_remedy(spec.cli, account_dir.as_deref()),
        ));
    }
    Ok(())
}

/// What to call the account when the caller named no directory.
fn default_account_label(cli: cas_mux::SupervisorCli) -> String {
    match cli {
        cas_mux::SupervisorCli::Codex => "the default CODEX_HOME (~/.codex)".to_string(),
        cas_mux::SupervisorCli::Claude => "the default Claude configuration directory".to_string(),
        other => format!("the default {} account", other.backend().name()),
    }
}

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

/// Shared event-store freshness window for the supervisor's worker views.
///
/// `worker_status` and `worker_activity` both answer whether a worker has
/// done anything recently, so they must read the same bounded source rather
/// than diverging on an arbitrary event count or activity class.
const WORKER_ACTIVITY_WINDOW_SECS: i64 = 600;
const WORKER_FILE_WRITE_SCAN_LIMIT: usize = 2_000;
pub(crate) const SPAWN_BOOT_VERIFICATION_WINDOW_SECS: i64 = 60;

/// Mark a just-registered spawn as failed when its harness PID has already
/// disappeared and the daemon missed the pane-exit event. The state-at age
/// keeps this scoped to the post-registration boot window; an old, unrelated
/// registered lifecycle row must not be rewritten because a later generation
/// of the same worker name died.
fn mark_recent_registered_spawn_failed(
    cas_root: &std::path::Path,
    factory_session: &str,
    worker_name: &str,
    detail: &str,
) {
    use cas_store::SpawnLifecycleState;

    let Ok(queue) = crate::store::open_spawn_queue_store(cas_root) else {
        return;
    };
    let Ok(rows) = queue.recent_spawn_lifecycle(factory_session, 50) else {
        return;
    };
    let now = chrono::Utc::now();
    let boot_window_secs = SPAWN_BOOT_VERIFICATION_WINDOW_SECS;
    for row in rows {
        if row.worker_name.as_deref() != Some(worker_name)
            || row.state != SpawnLifecycleState::Registered
        {
            continue;
        }
        let Some(state_at) = row.state_at else {
            continue;
        };
        let age_secs = (now - state_at).num_seconds();
        if (0..=boot_window_secs).contains(&age_secs) {
            let _ = queue.record_spawn_state(
                row.id,
                SpawnLifecycleState::Failed,
                Some(worker_name),
                Some(detail),
            );
        }
    }
}

/// Whether an event is evidence of work a worker actually performed.
///
/// Registration and lifecycle bookkeeping share the event store but must not
/// mask a fresher transcript/tool signal. Keep this predicate shared by the
/// activity feed and `worker_status` so their freshness claims agree.
fn is_worker_activity_event(event: &cas_types::Event) -> bool {
    use cas_types::EventType;

    matches!(
        event.event_type,
        EventType::WorkerSubagentSpawned
            | EventType::WorkerSubagentCompleted
            | EventType::WorkerFileEdited
            | EventType::WorkerGitCommit
            | EventType::WorkerVerificationBlocked
            | EventType::VerificationStarted
            | EventType::VerificationAdded
            | EventType::TaskNoteAdded
    )
}

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

/// First 8 characters of a session UUID — enough to compare two sessions at a
/// glance without wrapping the line (cas-dffe).
fn short_session(session_id: &str) -> &str {
    &session_id[..8.min(session_id.len())]
}

fn factory_rehome_label(agent: &cas_types::Agent) -> String {
    agent
        .metadata
        .get("factory_session_rehomed_from")
        .map(|prior| {
            format!(
                "\n    registry: re-homed from prior factory session {}",
                prior
            )
        })
        .unwrap_or_default()
}

/// Collapse accidental duplicate registrations for one logical factory
/// identity.
///
/// The registry can contain an old row and a fresh registration for the same
/// worker after a harness restart. `worker_status` is a roster, so its stable
/// identity is the visible role + worker name, not the registration's factory
/// session. Prefer the freshest heartbeat: it is the strongest evidence of
/// which same-name process is currently answering. This keeps a live respawn
/// from being hidden behind its older ghost row.
fn dedupe_authoritative_agents(agents: Vec<cas_types::Agent>) -> (Vec<cas_types::Agent>, usize) {
    let original_len = agents.len();
    let mut by_identity = std::collections::BTreeMap::<(String, String), cas_types::Agent>::new();

    for candidate in agents {
        let key = (candidate.role.to_string(), candidate.name.clone());
        match by_identity.get_mut(&key) {
            None => {
                by_identity.insert(key, candidate);
            }
            Some(current) => {
                let candidate_wins = candidate.last_heartbeat > current.last_heartbeat
                    || (candidate.last_heartbeat == current.last_heartbeat
                        && candidate.registered_at > current.registered_at);
                if candidate_wins {
                    *current = candidate;
                }
            }
        }
    }

    let mut deduped: Vec<_> = by_identity.into_values().collect();
    deduped.sort_by_key(|agent| agent.registered_at);
    let removed = original_len.saturating_sub(deduped.len());
    (deduped, removed)
}

fn worker_effort_from_agent(agent: &cas_types::Agent) -> Option<cas_mux::Effort> {
    agent
        .metadata
        .get("worker_effort")
        .and_then(|effort| effort.parse::<cas_mux::Effort>().ok())
}

fn parse_spawn_cli(cli: Option<&str>) -> Result<Option<cas_mux::SupervisorCli>, String> {
    cli.map(|s| {
        s.parse::<cas_mux::SupervisorCli>().map_err(|_| {
            format!("invalid cli value {s:?}: expected 'claude', 'codex', 'grok', or 'opencode'")
        })
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

const OPENCODE_ACCEPTED_EFFORTS_ENV: &str = "CAS_OPENCODE_ACCEPTED_EFFORTS";

#[derive(Debug, Default, serde::Deserialize)]
struct OpenCodeFactoryDefaultsToml {
    /// Preferred spelling for the endpoint's accepted shared effort values.
    opencode_accepted_efforts: Option<Vec<String>>,
    /// Short alias retained for ergonomic project-local configuration.
    opencode_efforts: Option<Vec<String>>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct OpenCodeFactoryToml {
    defaults: Option<OpenCodeFactoryDefaultsToml>,
    opencode: Option<OpenCodeEndpointToml>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct OpenCodeEndpointToml {
    accepted_efforts: Option<Vec<String>>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct OpenCodeConfigToml {
    factory: Option<OpenCodeFactoryToml>,
}

fn parse_opencode_effort_set(raw: &str, source: &str) -> Result<Vec<cas_mux::Effort>, String> {
    let mut efforts = Vec::new();
    for value in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let effort = value.parse::<cas_mux::Effort>().map_err(|error| {
            format!(
                "invalid OpenCode accepted effort {value:?} from {source}: {error}; use Cassy's effort values"
            )
        })?;
        if !efforts.contains(&effort) {
            efforts.push(effort);
        }
    }
    if efforts.is_empty() {
        return Err(format!(
            "OpenCode accepted effort set from {source} is empty; configure/probe at least one endpoint value"
        ));
    }
    Ok(efforts)
}

/// Read the endpoint's model-aware effort contract without inventing a
/// DashScope/Qwen default. A local serving stack may support any subset of
/// Cassy's shared effort vocabulary; preflight or operator configuration
/// supplies the exact set. The environment override is useful for a live
/// probe, while project/user TOML keeps the result durable for later spawns.
fn configured_opencode_efforts(
    project_config: Option<&std::path::Path>,
) -> Result<Option<Vec<cas_mux::Effort>>, String> {
    if let Ok(raw) = std::env::var(OPENCODE_ACCEPTED_EFFORTS_ENV) {
        return parse_opencode_effort_set(&raw, OPENCODE_ACCEPTED_EFFORTS_ENV).map(Some);
    }

    let user_config = dirs::home_dir().map(|home| home.join(".cas").join("config.toml"));
    let mut configured = None;
    for (source, path) in [
        ("user OpenCode config", user_config.as_deref()),
        ("project OpenCode config", project_config),
    ] {
        let Some(path) = path else {
            continue;
        };
        if !path.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|error| format!("could not read {source} {}: {error}", path.display()))?;
        let config = toml::from_str::<OpenCodeConfigToml>(&raw)
            .map_err(|error| format!("could not parse {source} {}: {error}", path.display()))?;
        let values = config.factory.and_then(|factory| {
            factory
                .opencode
                .and_then(|endpoint| endpoint.accepted_efforts)
                .or_else(|| {
                    factory.defaults.and_then(|defaults| {
                        defaults
                            .opencode_accepted_efforts
                            .or(defaults.opencode_efforts)
                    })
                })
        });
        if let Some(values) = values {
            let raw_values = values.join(",");
            configured = Some(parse_opencode_effort_set(
                &raw_values,
                &format!("{source} {}", path.display()),
            )?);
        }
    }
    Ok(configured)
}

fn format_opencode_efforts(efforts: &[cas_mux::Effort]) -> String {
    efforts
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_opencode_effort(
    model: &str,
    effort: Option<cas_mux::Effort>,
    accepted_efforts: Option<&[cas_mux::Effort]>,
) -> Result<(), String> {
    let Some(effort) = effort else {
        return Ok(());
    };
    let Some(accepted_efforts) = accepted_efforts else {
        return Err(format!(
            "OpenCode model {model:?} requested effort {effort}, but the endpoint accepted-effort set is unavailable; configure/probe {OPENCODE_ACCEPTED_EFFORTS_ENV}=<comma-separated values> or [factory.defaults].opencode_accepted_efforts. No effort remapping is performed."
        ));
    };
    if accepted_efforts.contains(&effort) {
        return Ok(());
    }
    Err(format!(
        "OpenCode model {model:?} rejects effort {effort}; endpoint accepted efforts: [{}]. No effort remapping is performed.",
        format_opencode_efforts(accepted_efforts)
    ))
}

/// Request-time shutdown state carried into the supervisor receipt.
///
/// The queue remains intentionally small and transport-only, so the safety
/// decision and its human-readable evidence stay together here at the MCP
/// boundary. The daemon receives exact names rather than a selector it could
/// accidentally broaden.
#[derive(Debug)]
struct ShutdownWorkerSnapshot {
    worker_name: String,
    worker_id: String,
    task_states: Vec<String>,
    has_in_progress_task: bool,
    worktree_state: String,
    unsafe_worktree: bool,
}

impl ShutdownWorkerSnapshot {
    fn requires_force(&self) -> bool {
        self.has_in_progress_task || self.unsafe_worktree
    }

    fn render(&self) -> String {
        let tasks = if self.task_states.is_empty() {
            "none".to_string()
        } else {
            self.task_states.join(", ")
        };
        format!(
            "{} (id={}): tasks=[{}]; {}",
            self.worker_name, self.worker_id, tasks, self.worktree_state
        )
    }
}

fn shutdown_worker_snapshot(
    cas_root: &std::path::Path,
    worker: &cas_types::Agent,
    tasks: &[cas_types::Task],
) -> ShutdownWorkerSnapshot {
    let assigned: Vec<&cas_types::Task> = tasks
        .iter()
        .filter(|task| {
            task.assignee
                .as_deref()
                .is_some_and(|assignee| assignee == worker.name || assignee == worker.id)
                && !task.is_terminal()
        })
        .collect();
    let has_in_progress_task = assigned
        .iter()
        .any(|task| task.status == cas_types::TaskStatus::InProgress);
    let task_states = assigned
        .iter()
        .map(|task| format!("{} [{}]", task.id, task.status))
        .collect();

    let (worktree_state, unsafe_worktree) = match resolve_worker_clone_path(cas_root, worker) {
        WorkerClonePathResolve::Ready(path) => {
            let dirty = dirty_file_count(&path);
            let unpushed = run_git(
                &path,
                &["rev-list", "--count", "HEAD", "--not", "--remotes"],
            )
            .and_then(|count| {
                count
                    .parse::<usize>()
                    .map_err(|error| format!("invalid unpushed count {count:?}: {error}"))
            });
            match (dirty, unpushed) {
                (Ok(dirty), Ok(unpushed)) => (
                    format!(
                        "worktree={} (dirty_files={dirty}, unpushed_commits={unpushed})",
                        path.display()
                    ),
                    dirty > 0 || unpushed > 0,
                ),
                (dirty, unpushed) => {
                    let mut errors = Vec::new();
                    if let Err(error) = dirty {
                        errors.push(format!("dirty-state probe failed: {error}"));
                    }
                    if let Err(error) = unpushed {
                        errors.push(format!("unpushed-state probe failed: {error}"));
                    }
                    (
                        format!(
                            "worktree={} (STATE UNKNOWN: {})",
                            path.display(),
                            errors.join("; ")
                        ),
                        true,
                    )
                }
            }
        }
        WorkerClonePathResolve::NotOnDisk { candidate, .. } => (
            format!("worktree={} (not present)", candidate.display()),
            false,
        ),
    };

    ShutdownWorkerSnapshot {
        worker_name: worker.name.clone(),
        worker_id: worker.id.clone(),
        task_states,
        has_in_progress_task,
        worktree_state,
        unsafe_worktree,
    }
}

/// cas-28a4 (GH #71): which worker CLI a model slug belongs to.
///
/// Deliberately conservative — an unrecognized slug returns `None` and is
/// accepted as-is, because this gate exists to catch obviously-crossed wires
/// (a Claude slug queued onto Codex), not to police the model catalog and
/// reject a model the day it ships.
fn cli_for_model_slug(model: &str) -> Option<cas_mux::SupervisorCli> {
    let model = model.trim().to_ascii_lowercase();
    if model.is_empty() {
        return None;
    }
    if model.starts_with("grok") {
        return Some(cas_mux::SupervisorCli::Grok);
    }
    if model.starts_with("claude")
        || model.starts_with("opus")
        || model.starts_with("sonnet")
        || model.starts_with("haiku")
        || model.starts_with("fable")
    {
        return Some(cas_mux::SupervisorCli::Claude);
    }
    if model.starts_with("gpt") || model.starts_with("codex") || model.starts_with("o3") {
        return Some(cas_mux::SupervisorCli::Codex);
    }
    // OpenCode preserves provider/model selectors such as
    // `local/qwen3.8` and `alibaba-cn/qwen3.8-max` verbatim. A slash is the
    // selector boundary used by OpenCode's multi-provider catalog; recognize
    // it only for harness inference, while explicit cli=opencode remains the
    // authoritative path for arbitrary provider strings.
    if model.contains('/') {
        return Some(cas_mux::SupervisorCli::OpenCode);
    }
    None
}

/// cas-28a4 (GH #71): reject a model slug that belongs to a different CLI than
/// the one being spawned, naming both sides and the fix.
fn validate_model_matches_cli(cli: cas_mux::SupervisorCli, model: &str) -> Result<(), String> {
    match cli_for_model_slug(model) {
        Some(model_cli) if model_cli != cli => Err(format!(
            "invalid spawn_workers combination: model {model:?} is a {} model but \
             cli={} was requested. \
             Pass cli={} to spawn it on its own harness, or choose a {} model (e.g. {}).",
            model_cli.backend().name(),
            cli.backend().name(),
            model_cli.backend().name(),
            cli.backend().name(),
            default_worker_model_for_cli(cli),
        )),
        _ => Ok(()),
    }
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
    let mut specs =
        build_spawn_specs_with_project_config(1, cli, model, effort, None, None, project_config)?;
    let spec = specs
        .pop()
        .ok_or_else(|| "failed to resolve worker spec: no worker slots returned".to_string())?;
    serde_json::to_string(&spec).map_err(|e| format!("failed to serialize WorkerSpec: {e}"))
}

/// Resolve a registry lane into one immutable recipe decision per spawn slot.
///
/// Lane mode may still carry per-worker identity/account metadata, but it may
/// not smuggle in a partial explicit recipe through `workers=[...]`. Reject
/// those fields before queueing so the lane's fallback policy remains
/// unambiguous and explicit recipes retain their fail-closed behavior.
fn build_lane_spawn_specs(
    slots: usize,
    lane: &str,
    config_dir: Option<&str>,
    workers_json: Option<&str>,
    snapshot: &cas_factory::CapabilitySnapshot,
) -> Result<(Vec<cas_mux::WorkerSpec>, String, Vec<String>), String> {
    let worker_spec_jsons = parse_spawn_worker_specs(workers_json, slots)?;
    if slots == 0 {
        return Err("lane spawn requires at least one worker slot".to_string());
    }
    validate_lane_request(lane, false, false, false).map_err(|error| error.to_string())?;

    let decisions = resolve_lane_specs(lane, slots, snapshot).map_err(|error| error.to_string())?;
    let recipe_id = decisions
        .first()
        .ok_or_else(|| "lane spawn has no worker slots".to_string())?
        .recipe_id
        .clone();
    let mut warnings = decisions
        .iter()
        .flat_map(|decision| decision.warnings.iter().cloned())
        .collect::<Vec<_>>();
    let mut specs = decisions
        .into_iter()
        .map(|decision| decision.spec)
        .collect::<Vec<_>>();

    for spec in &mut specs {
        if let Some(config_dir) = config_dir {
            spec.config_dir = Some(config_dir.to_string());
        }
    }

    for (slot, json) in worker_spec_jsons.iter().enumerate() {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|error| format!("invalid workers JSON entry: {error}"))?;
        let Some(object) = value.as_object() else {
            return Err("invalid workers JSON: every entry must be an object".to_string());
        };
        let explicit_fields = ["cli", "model", "effort"]
            .into_iter()
            .filter(|field| object.get(*field).is_some_and(|value| !value.is_null()))
            .collect::<Vec<_>>();
        if !explicit_fields.is_empty() {
            return Err(format!(
                "lane={:?} cannot be combined with explicit {} recipe field(s) in workers[{}]; choose lane= or an explicit cli/model/effort recipe",
                lane.trim(),
                explicit_fields.join(", "),
                slot
            ));
        }
        if let Some(name) = object.get("name") {
            let name = name
                .as_str()
                .ok_or_else(|| format!("invalid workers[{slot}].name: expected a string"))?;
            specs[slot].name = Some(name.to_string());
        }
        if let Some(config_dir) = object.get("config_dir") {
            let config_dir = config_dir
                .as_str()
                .ok_or_else(|| format!("invalid workers[{slot}].config_dir: expected a string"))?;
            specs[slot].config_dir = Some(config_dir.to_string());
        }
    }

    // Each decision currently has the same static warning set per slot. Keep
    // the receipt concise while retaining every distinct warning from a
    // capability-aware resolver.
    warnings.sort();
    warnings.dedup();
    Ok((specs, recipe_id, warnings))
}

/// Resolve one worker spec per spawn slot, applying the batch fields as
/// defaults and the optional `workers=[{...}]` entries as the final cascade
/// layer. This deliberately reuses `cas_factory::resolve_specs` rather than
/// introducing an MCP-only interpretation of worker configuration.
fn build_spawn_specs_with_project_config(
    slots: usize,
    cli: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    config_dir: Option<&str>,
    workers_json: Option<&str>,
    project_config: Option<std::path::PathBuf>,
) -> Result<Vec<cas_mux::WorkerSpec>, String> {
    let worker_spec_jsons = parse_spawn_worker_specs(workers_json, slots)?;
    let parsed_cli = parse_spawn_cli(cli)?;
    let parsed_effort = parse_spawn_effort(effort)?;

    // cas-28a4 (GH #71): an explicitly requested cli/model pair that crosses
    // harnesses is rejected here — before anything is queued — instead of
    // surfacing as workers that boot on the wrong CLI.
    if let (Some(requested_cli), Some(model)) = (parsed_cli, model) {
        validate_model_matches_cli(requested_cli, model)?;
    }

    let sources = cas_factory::ConfigSources {
        project_config: project_config.clone(),
        cli_flag: parsed_cli,
        model_flag: model.map(String::from),
        effort_flag: parsed_effort,
        config_dir_flag: config_dir.map(String::from),
        worker_spec_jsons: worker_spec_jsons.clone(),
        ..Default::default()
    };
    let configured: Vec<(bool, bool)> = (0..slots)
        .map(|slot| {
            Ok((
                cas_factory::worker_slot_cli_configured(slot, &sources)
                    .map_err(|e| format!("failed to inspect worker cli config: {e}"))?,
                cas_factory::worker_slot_effort_configured(slot, &sources)
                    .map_err(|e| format!("failed to inspect worker effort config: {e}"))?,
            ))
        })
        .collect::<Result<_, String>>()?;
    let mut specs = cas_factory::resolve_specs(slots, sources)
        .map_err(|e| format!("failed to resolve worker spec: {e}"))?;
    let opencode_accepted_efforts = configured_opencode_efforts(project_config.as_deref())?;

    // EPIC cas-8888 (cas-9a31, Phase 1) SILENT SITE — audited, left AS-IS
    // per the task's own guidance: this default-cli auto-upgrade only ever
    // fires when the resolved default happens to be Claude (never Grok, since
    // nothing defaults TO Grok yet — it isn't a stock/default CLI at this
    // phase), so no Grok arm is needed here.
    for (slot, spec) in specs.iter_mut().enumerate() {
        let override_value = worker_spec_jsons
            .get(slot)
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok());
        let override_has = |field| {
            override_value
                .as_ref()
                .and_then(|value| value.get(field))
                .is_some()
        };
        let cli_explicit = cli.is_some() || override_has("cli");
        let model_explicit = model.is_some() || override_has("model");
        let effort_explicit = effort.is_some() || override_has("effort");
        let (configured_cli, configured_effort) = configured[slot];

        if !cli_explicit && !configured_cli && spec.cli == cas_mux::SupervisorCli::Claude {
            spec.cli = cas_mux::SupervisorCli::Codex;
        }
        // cas-28a4 (GH #71): an unambiguous per-worker model slug is the
        // strongest per-slot statement of intent when cli was omitted.
        let requested_model = override_value
            .as_ref()
            .and_then(|value| value.get("model"))
            .and_then(serde_json::Value::as_str)
            .or(model);
        if !cli_explicit {
            if let Some(model_cli) = requested_model.and_then(cli_for_model_slug) {
                if model_cli != spec.cli {
                    tracing::info!(
                        target: "cas::factory",
                        requested_model = %requested_model.unwrap_or_default(),
                        resolved_cli = %spec.cli.backend().name(),
                        model_cli = %model_cli.backend().name(),
                        slot = slot + 1,
                        "cas-28a4: explicit model slug overrides the resolved default cli"
                    );
                    spec.cli = model_cli;
                }
            }
        }
        if !model_explicit && spec.model.is_none() {
            spec.model = Some(default_worker_model_for_cli(spec.cli).to_string());
        }
        if !effort_explicit && !configured_effort && spec.cli == cas_mux::SupervisorCli::OpenCode {
            // The shared resolver's High placeholder is not an OpenCode
            // endpoint contract. Omitted effort means let the configured
            // local provider choose its own default; only explicit/configured
            // effort values go through model-aware validation below.
            spec.effort = None;
        } else if !effort_explicit
            && !configured_effort
            && spec.effort == Some(cas_mux::Effort::High)
            && spec.cli != cas_mux::SupervisorCli::OpenCode
        {
            spec.effort = Some(default_worker_effort_for_cli(spec.cli));
        }

        // Static registry policy is authoritative and always runs before the
        // route-specific OpenCode support/receipt checks below. This makes a
        // malformed recipe fail with one stable policy reason while keeping
        // the hosted pre-queue gate authoritative for otherwise valid routes.
        cas_factory::validate_explicit(spec, &cas_factory::CapabilitySnapshot::default())
            .map_err(|error| error.to_string())?;
        if spec.cli == cas_mux::SupervisorCli::OpenCode {
            let model = spec.model.as_deref().ok_or_else(|| {
                "cli=opencode requires a configured provider/model selector".to_string()
            })?;
            match opencode_route_for_selector(model)? {
                OpenCodeRoute::Local => validate_opencode_effort(
                    model,
                    spec.effort,
                    opencode_accepted_efforts.as_deref(),
                )?,
                OpenCodeRoute::HostedTokenPlan | OpenCodeRoute::HostedPayg => {
                    let lane = hosted_lane_for_selector(model)?;
                    hosted_serving_identity(model)?;
                    validate_hosted_effort_for_lane(lane, spec.effort)?;
                }
                // `hosted` is a T8-only receipt spelling and cannot be
                // selected for a new spawn because it does not pin billing.
                OpenCodeRoute::Hosted => {
                    return Err(
                        "OpenCode hosted selector uses a legacy route identity; choose explicit qwencloud/qwen3.8-max or alibaba/qwen3.8-max"
                            .to_string(),
                    );
                }
            }
        }
        if let Some(model) = spec.model.as_deref() {
            validate_model_matches_cli(spec.cli, model)?;
        }
    }
    Ok(specs)
}

/// Decode MCP's JSON-array field while keeping the shared resolver responsible
/// for the actual WorkerSpec field schema and validation.
fn parse_spawn_worker_specs(raw: Option<&str>, slots: usize) -> Result<Vec<String>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let values: Vec<serde_json::Value> = serde_json::from_str(raw).map_err(|error| {
        format!("invalid workers JSON: expected an array of worker specs: {error}")
    })?;
    if values.len() > slots {
        return Err(format!(
            "workers has {} entries but this spawn has only {slots} worker slot(s)",
            values.len()
        ));
    }
    values
        .into_iter()
        .map(|value| {
            if !value.is_object() {
                return Err("invalid workers JSON: every entry must be an object".to_string());
            }
            serde_json::to_string(&value)
                .map_err(|error| format!("failed to serialize worker override: {error}"))
        })
        .collect()
}

fn spawn_worker_entries_len(raw: Option<&str>) -> Result<Option<usize>, String> {
    let Some(raw) = raw else { return Ok(None) };
    let values: Vec<serde_json::Value> = serde_json::from_str(raw).map_err(|error| {
        format!("invalid workers JSON: expected an array of worker specs: {error}")
    })?;
    if values.is_empty() {
        return Err("workers must contain at least one worker spec".to_string());
    }
    Ok(Some(values.len()))
}

fn spawn_specs_summary(specs: &[cas_mux::WorkerSpec], worker_names: &[String]) -> String {
    specs
        .iter()
        .enumerate()
        .map(|(slot, spec)| {
            let name = spec
                .name
                .as_deref()
                .or_else(|| worker_names.get(slot).map(String::as_str))
                .map_or_else(|| format!("slot {}", slot + 1), str::to_string);
            let account = spec
                .config_dir
                .as_deref()
                .or(spec.requester_config_dir.as_deref())
                .unwrap_or("(default/inherited)");
            format!(
                "{name}: {} model={} effort={} account={account}",
                spec.cli.backend().name(),
                spec.model.as_deref().unwrap_or("(backend default)"),
                format_effort(spec.effort),
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn spawn_spec_warning(model_explicit: bool, effort_explicit: bool, spec_json: &str) -> String {
    let mut warnings = Vec::new();
    if !model_explicit || !effort_explicit {
        let omitted = match (model_explicit, effort_explicit) {
            (false, false) => "model=/effort=",
            (false, true) => "model=",
            (true, false) => "effort=",
            (true, true) => unreachable!("warning requires at least one omitted field"),
        };
        match serde_json::from_str::<cas_mux::WorkerSpec>(spec_json) {
            Ok(spec) => {
                let model_uses_policy = model_explicit
                    || spec.model.as_deref() == Some(default_worker_model_for_cli(spec.cli));
                let effort_uses_policy = effort_explicit
                    || (spec.cli == cas_mux::SupervisorCli::OpenCode && spec.effort.is_none())
                    || spec.effort == Some(default_worker_effort_for_cli(spec.cli));
                let fallback = if model_uses_policy && effort_uses_policy {
                    "policy default"
                } else {
                    "configured fallback"
                };
                warnings.push(format!(
                    "Warning: spawn_workers omitted {omitted}; resolved to {fallback} {}/{}/{} — pass model=/effort= explicitly to tier the spawn.",
                    spec.cli.backend().name(),
                    spec.model.as_deref().unwrap_or("(backend default)"),
                    format_effort(spec.effort)
                ));
            }
            Err(_) => warnings.push(format!(
                "Warning: spawn_workers omitted {omitted}; pass model=/effort= explicitly to tier the spawn."
            )),
        }
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

/// Render the existing omitted model/effort notice for every resolved worker
/// spec. A multi-worker request may resolve to different harnesses, so keeping
/// this per-spec prevents the receipt from silently dropping the policy
/// fallback that the legacy single-spec response exposed.
fn spawn_specs_warning(
    model_explicit: bool,
    effort_explicit: bool,
    specs: &[cas_mux::WorkerSpec],
) -> String {
    specs
        .iter()
        .map(|spec| {
            let spec_json = serde_json::to_string(spec)
                .expect("WorkerSpec must serialize after queue payload serialization");
            spawn_spec_warning(model_explicit, effort_explicit, &spec_json)
        })
        .collect()
}

/// Lane resolution supplies a complete recipe, including model and effort.
/// Do not emit the legacy omitted-field warning for that request shape: it
/// falsely tells operators that a lane's model was an accidental fallback and
/// obscures the lane receipt they actually selected.
fn spawn_warning_for_request(
    lane_requested: bool,
    model_explicit: bool,
    effort_explicit: bool,
    legacy_single_spec_payload: bool,
    spec_json: &str,
    specs: &[cas_mux::WorkerSpec],
) -> String {
    if lane_requested {
        String::new()
    } else if legacy_single_spec_payload {
        spawn_spec_warning(model_explicit, effort_explicit, spec_json)
    } else {
        spawn_specs_warning(model_explicit, effort_explicit, specs)
    }
}

fn current_factory_session() -> Option<String> {
    std::env::var("CAS_FACTORY_SESSION")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// How long a spawn request may sit in a non-terminal state before
/// `worker_status` calls it out as unconfirmed (GH #60).
///
/// Deliberately longer than a provisioning pass (git worktree add + submodule
/// init + hook install) and shorter than a supervisor's patience. A request
/// still `queued` past this point means nothing is consuming the queue at all —
/// the original incident, where two requests returned success-shaped receipts
/// and both daemon logs stayed zero bytes.
const SPAWN_UNCONFIRMED_SECS: i64 = 90;

/// Window of spawn history worth showing. Older terminal rows are noise.
const SPAWN_HISTORY_WINDOW_SECS: i64 = 1800;

/// Render the task a worker is holding, for `worker_status` (GH #67).
///
/// The roster already knew the assignment; it just never said so, and the
/// documented workaround was `git -C .cas/worktrees/<name> log/status` plus a
/// task lookup. An idle-looking worker with `cas-1234 (in progress)` next to it
/// is a different conversation from one with nothing assigned — that
/// distinction is the whole of GH #67 item 1.
///
/// Pure over its inputs so every branch is testable without a store.
fn format_assigned_task_info(
    in_progress: Option<(&str, &str)>,
    assigned_open: Option<(&str, &str, bool)>,
    parked: Option<(&str, &str, cas_types::TaskStatus)>,
    parked_merge_already_integrated: bool,
) -> String {
    const TITLE_CAP: usize = 60;
    let truncate = |title: &str| -> String {
        let title = title.trim();
        if title.chars().count() <= TITLE_CAP {
            return title.to_string();
        }
        let short: String = title.chars().take(TITLE_CAP).collect();
        format!("{}…", short.trim_end())
    };

    match (in_progress, assigned_open, parked) {
        (Some((id, title)), _, _) => {
            format!("\n    task: {id} (in progress) — {}", truncate(title))
        }
        // Assigned but not started: the dispatch grace window, or a worker that
        // never picked the task up. Naming it lets the supervisor tell those
        // apart without opening anything.
        (None, Some((id, title, true)), _) => format!(
            "\n    task: {id} (verification rejected; assigned worker inactive) — {} → WAITING ON YOU: resume the existing worker or replace it",
            truncate(title)
        ),
        (None, Some((id, title, false)), _) => {
            format!(
                "\n    task: {id} (assigned, not started) — {}",
                truncate(title)
            )
        }
        // cas-e728 (GH #105): finished and waiting on the SUPERVISOR. This
        // rendered as "none assigned" — identical to a worker with nothing to
        // do — so the one state that genuinely needs supervisor action looked
        // like the one that needs none. It is also why the stall flag "missed
        // the real anomaly": there was nothing in the row to miss.
        (None, None, Some((id, title, status))) => {
            // Matched exhaustively on purpose: a catch-all would silently
            // relabel any status added to the parked set later.
            let (label, waiting) = match status {
                cas_types::TaskStatus::AwaitingMerge if parked_merge_already_integrated => (
                    "delivered-and-merged, awaiting worker re-close",
                    "WAITING ON WORKER: the assigned worker must retry task close",
                ),
                cas_types::TaskStatus::AwaitingMerge => (
                    "finished, awaiting merge",
                    "WAITING ON YOU: merge its branch, then it can close",
                ),
                cas_types::TaskStatus::Blocked => (
                    "blocked",
                    "WAITING ON YOU: clear the blocker or reassign — the worker cannot proceed",
                ),
                cas_types::TaskStatus::Open
                | cas_types::TaskStatus::InProgress
                | cas_types::TaskStatus::Closed => ("parked", "WAITING ON YOU: check the task"),
                cas_types::TaskStatus::Cancelled => (
                    "cancelled without delivery",
                    "WAITING ON YOU: no delivery or merge action is pending",
                ),
            };
            format!(
                "\n    task: {id} ({label}) — {} → {waiting}",
                truncate(title)
            )
        }
        (None, None, None) => "\n    task: none assigned".to_string(),
    }
}

/// Render the recent spawn lifecycle for `worker_status` (GH #60).
///
/// The enqueue receipt proves only that a row was inserted. This section is
/// what makes the documented supervisor guard — "call `worker_status` after
/// every `spawn_workers` and don't report dispatch complete until the worker
/// appears" — structural rather than a habit, and it names which worker each
/// request produced so two in-flight anonymous spawns can never be
/// cross-attributed.
///
/// Pure over its inputs so the interesting states are unit-testable without a
/// daemon, a queue, or a clock.
/// Render the undelivered-lifecycle-relay banner for `worker_status`
/// (cas-7787, GH #160).
///
/// Each row is a moment Cassy told the supervisor a lane was parked behind them
/// and the message did not arrive. In the reported session four of these went
/// completely unrecorded, so the supervisor's `worker_status` looked healthy
/// while three finished lanes waited on a human to notice.
///
/// Empty input renders NOTHING — a banner that appears when there is no
/// problem is a banner people learn to skip, and this one has to be believed
/// the one time it fires.
///
/// Pure over its input so the wording and the empty case are unit-testable
/// without a daemon, a queue, or a clock.
fn format_undelivered_relay_section(rows: &[cas_store::UndeliveredLifecycleRelay]) -> String {
    let actionable = rows
        .iter()
        .filter(|row| {
            crate::prompt_revalidation::parse_worker_died_envelope(&row.prompt).is_none_or(
                |envelope| !envelope.held_tasks.is_empty() || !envelope.recovered_tasks.is_empty(),
            )
        })
        .collect::<Vec<_>>();
    if actionable.is_empty() {
        return String::new();
    }
    let total = actionable.len();
    let displayed = actionable.len().min(10);
    let mut out = String::new();
    out.push_str(&format!(
        "⚠ UNDELIVERED SUPERVISOR RELAY ({total} total; displaying {displayed}) — these never reached you:\n",
    ));
    for row in actionable.into_iter().take(displayed) {
        let what = row
            .summary
            .as_deref()
            .unwrap_or("task lifecycle transition");
        let acknowledge = row
            .source
            .rsplit(':')
            .next()
            .and_then(|id| id.parse::<i64>().ok())
            .map(|id| {
                format!(
                    "; after reconciling: `coordination action=message_ack notification_id={id}`"
                )
            })
            .unwrap_or_default();
        out.push_str(&format!(
            "  • {what} (queued {}, source {}{acknowledge})\n",
            row.created_at.to_rfc3339(),
            row.source
        ));
    }
    out.push_str(
        "  These lanes may still be waiting on you. Open each task directly \
         (`task action=show id=<id>`) — a missing relay is not evidence the work was handled.\n\n",
    );
    out
}

/// Select launch records that can be stopped before their PTY has registered
/// an Agent row. `id` accepts either the eventual worker name or the durable
/// spawn-request id included in the launch receipt.
fn select_launched_shutdown_targets(
    launched_workers: &[cas_store::SpawnLifecycle],
    requested_id: Option<&str>,
    requested_names: &[String],
    count: Option<i32>,
) -> Vec<cas_store::SpawnLifecycle> {
    if let Some(id) = requested_id {
        launched_workers
            .iter()
            .filter(|spawn| spawn.worker_name.as_deref() == Some(id) || spawn.id.to_string() == id)
            .cloned()
            .collect()
    } else if !requested_names.is_empty() {
        launched_workers
            .iter()
            .filter(|spawn| {
                spawn
                    .worker_name
                    .as_deref()
                    .is_some_and(|name| requested_names.iter().any(|requested| requested == name))
            })
            .cloned()
            .collect()
    } else if count.unwrap_or(0) == 0 {
        launched_workers.to_vec()
    } else {
        launched_workers
            .iter()
            .take(count.unwrap_or_default() as usize)
            .cloned()
            .collect()
    }
}

fn format_spawn_lifecycle_section(
    rows: &[cas_store::SpawnLifecycle],
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    use cas_store::SpawnLifecycleState as State;

    let recent: Vec<&cas_store::SpawnLifecycle> = rows
        .iter()
        .filter(|row| (now - row.created_at).num_seconds() <= SPAWN_HISTORY_WINDOW_SECS)
        .collect();
    if recent.is_empty() {
        return String::new();
    }

    let mut out = String::from("\nRecent spawn requests:\n");
    let mut unconfirmed = 0usize;
    let mut failed = 0usize;

    for row in recent {
        let age = (now - row.created_at).num_seconds().max(0);
        let stale = !row.state.is_terminal() && age >= SPAWN_UNCONFIRMED_SECS;
        let who = row
            .worker_name
            .as_deref()
            .map(|name| format!(" → {name}"))
            .unwrap_or_else(|| {
                if row.requested_names.is_empty() {
                    String::new()
                } else {
                    format!(" → (requested {})", row.requested_names.join(", "))
                }
            });

        let status = match row.state {
            State::Registered => "registered".to_string(),
            State::Failed => "FAILED".to_string(),
            other if stale => format!("{} — UNCONFIRMED", other.as_str()),
            other => other.as_str().to_string(),
        };
        if row.state == State::Failed {
            failed += 1;
        } else if stale {
            unconfirmed += 1;
        }

        let reason = row
            .detail
            .as_deref()
            .filter(|d| !d.is_empty())
            .map(|d| format!(" — {d}"))
            .unwrap_or_default();
        let task = row
            .task_id
            .as_deref()
            .map(|t| format!(" [task {t}]"))
            .unwrap_or_default();

        out.push_str(&format!(
            "  • request {}{}: {} ({age}s ago){}{}\n",
            row.id, who, status, task, reason
        ));
    }

    if failed > 0 || unconfirmed > 0 {
        out.push_str(&format!(
            "  ⚠ {failed} failed, {unconfirmed} unconfirmed after {SPAWN_UNCONFIRMED_SECS}s. \
             An UNCONFIRMED request means the worker never registered — treat it as not \
             dispatched, and check the daemon logs before re-spawning.\n"
        ));
    }

    out
}

fn worker_hold_role_gate(is_supervisor: bool, action: &str) -> Result<(), String> {
    if is_supervisor {
        Ok(())
    } else {
        Err(format!(
            "coordination {action} rejected: only supervisors may change a worker's director hold state"
        ))
    }
}

/// Resolve the ref used by `sync_all_workers` without touching worker clones.
///
/// Resolution order is intentionally strict:
/// 1. A supplied epic id is authoritative and must resolve to a live Epic with
///    a branch in this project's task store. Any failure is terminal.
/// 2. An explicit branch is used as-is.
/// 3. The current session's pinned/default epic is used only after its
///    `project_dir` is proven to match this Cassy root's project.
/// 4. With no focused epic, use the local repository's default branch.
///
/// There is deliberately no "first ready/in-progress epic" fallback: task
/// store listing may include synchronized/global records from other projects,
/// and choosing one would turn ambient state into a branch mutation target.
fn resolve_sync_all_workers_target(
    cas_root: &std::path::Path,
    req: &FactoryRequest,
) -> std::result::Result<String, String> {
    use crate::store::open_task_store;
    use cas_types::TaskType;

    fn epic_parent_branch(
        task_store: &dyn crate::store::TaskStore,
        epic_id: &str,
        source: &str,
    ) -> std::result::Result<String, String> {
        let epic = task_store
            .get(epic_id)
            .map_err(|e| format!("sync_all_workers: {source} epic {epic_id} not found: {e}"))?;
        if epic.task_type != TaskType::Epic {
            return Err(format!(
                "sync_all_workers: {source} task {epic_id} is not an Epic (task_type={:?})",
                epic.task_type
            ));
        }
        if epic.is_terminal() {
            return Err(format!(
                "sync_all_workers: {source} epic {epic_id} is Closed"
            ));
        }
        epic.deliverables
            .work_target
            .as_ref()
            .map(|target| target.target_branch.trim())
            .filter(|branch| !branch.is_empty())
            .map(str::to_string)
            .or_else(|| epic.branch.filter(|branch| !branch.trim().is_empty()))
            .ok_or_else(|| {
                format!("sync_all_workers: {source} epic {epic_id} has no parent branch")
            })
    }

    if let Some(raw_id) = req.id.as_deref() {
        let epic_id = raw_id.trim();
        if epic_id.is_empty() {
            return Err("sync_all_workers: explicit epic id cannot be blank".to_string());
        }
        let task_store = open_task_store(cas_root)
            .map_err(|e| format!("sync_all_workers: failed to open task store: {e}"))?;
        return epic_parent_branch(task_store.as_ref(), epic_id, "explicit");
    }

    if let Some(branch) = req
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
    {
        return Ok(branch.to_string());
    }

    if let Some(factory_session) = current_factory_session() {
        use crate::ui::factory::{SessionMetadata, metadata_path};

        let path = metadata_path(&factory_session);
        let data = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "sync_all_workers: cannot validate factory session {factory_session} metadata: {e}"
            )
        })?;
        let metadata: SessionMetadata = serde_json::from_str(&data).map_err(|e| {
            format!("sync_all_workers: invalid factory session {factory_session} metadata: {e}")
        })?;

        let project_root = cas_root.parent().unwrap_or(cas_root);
        let metadata_project = metadata
            .project_dir
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                format!(
                    "sync_all_workers: factory session {factory_session} has no project_dir; refusing unscoped epic focus"
                )
            })?;
        let metadata_project = std::fs::canonicalize(metadata_project).map_err(|e| {
            format!(
                "sync_all_workers: cannot resolve factory session {factory_session} project_dir {metadata_project}: {e}"
            )
        })?;
        let project_root = std::fs::canonicalize(project_root).map_err(|e| {
            format!(
                "sync_all_workers: cannot resolve current project {}: {e}",
                project_root.display()
            )
        })?;
        if metadata_project != project_root {
            return Err(format!(
                "sync_all_workers: factory session {factory_session} belongs to {}, not current project {}; refusing cross-project epic focus",
                metadata_project.display(),
                project_root.display()
            ));
        }

        let focused_epic_id = metadata
            .pinned_epic_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .or_else(|| {
                metadata
                    .epic_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
            });
        if let Some(epic_id) = focused_epic_id {
            let task_store = open_task_store(cas_root)
                .map_err(|e| format!("sync_all_workers: failed to open task store: {e}"))?;
            return epic_parent_branch(task_store.as_ref(), epic_id, "focused");
        }
    }

    // Use local main branch, not origin/main. In factory mode the supervisor
    // merges worker branches into the local default branch, so workers should
    // rebase onto it directly.
    use crate::worktree::GitOperations;
    Ok(GitOperations::detect_repo_root(cas_root)
        .ok()
        .map(GitOperations::new)
        .map(|git| git.detect_default_branch())
        .unwrap_or_else(|| "main".to_string()))
}

fn parse_worker_name_filter(filter: Option<&String>) -> std::collections::HashSet<String> {
    filter
        .into_iter()
        .flat_map(|names| names.split(','))
        .map(strip_target_wrapping)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Trim one element of a target list down to the bare identifier.
///
/// The parameter is typed as a comma-separated string, but its plural name
/// invites a JSON array and callers write one (cas-2c05: a supervisor called
/// `worker_names=["quick-dolphin-15"]`). Left alone, the literal `["name"]`
/// text is compared against `name`, never matches, and the tool reports the
/// worker as unknown while printing it in the known list of the same message.
/// Accepting both shapes is bounded and explicit — brackets and quotes are
/// stripped, nothing else is guessed.
fn strip_target_wrapping(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

/// The single definition of "does this worker answer to this target".
///
/// cas-2c05: the shutdown resolver matched `worker_names` against the name
/// ONLY, while the `id` selector matched name-or-id and the error message
/// printed a known-workers list built from a third filter. Those three
/// disagreeing about identity is the bug itself, so identity now has exactly
/// one definition and every caller uses it.
///
/// Names are compared case-insensitively; ids are not, because an id is opaque
/// and a case-folded comparison of opaque tokens invites false positives.
fn worker_answers_to(worker: &cas_types::Agent, target: &str) -> bool {
    let target = target.trim();
    worker.name.eq_ignore_ascii_case(target)
        || worker.id == target
        || worker
            .cc_session_id
            .as_deref()
            .is_some_and(|session| session == target)
}

/// How many undrained spawn-queue rows `spawn_workers` scans when checking
/// whether a task_id already authorized a spawn (cas-549c, GH #96). The queue
/// is drained every daemon tick, so the pending set is small; this is a bound
/// against a pathologically backed-up queue, not a correctness knob.
const SPAWN_QUEUE_DUPLICATE_SCAN: usize = 100;

fn no_active_epic_guidance(why: &str) -> String {
    format!(
        "No active EPIC found, and {why} Either name the work directly or open an EPIC:\n\
         0. Spawn for one existing open task (no EPIC needed): \
         mcp__cas__coordination action=spawn_workers count=1 task_id=<task-id>\n\
         1. Create EPIC: mcp__cas__task action=create task_type=epic title=\"...\" description=\"...\"\n\
         2. Or assign existing EPIC: mcp__cas__task action=start id=<epic-id>\n\
         3. Optionally gather requirements using the cas-supervisor skill's planning references\n\
         4. Break into tasks using the cas-supervisor skill's planning references\n\
         5. Then spawn workers to work on the tasks"
    )
}

impl CasService {
    pub(super) async fn factory_spawn_workers(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::types::validate_delivery_mode;
        use crate::store::{open_agent_store, open_spawn_queue_store, open_task_store};
        use crate::ui::factory::{metadata_path, persist_session_metadata_delivery_mode_at};
        use cas_types::{TaskStatus, TaskType};

        let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open task store: {e}"),
            )
        })?;
        let requested_delivery_mode = validate_delivery_mode(req.delivery_mode.as_deref())
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;

        let workers_len = spawn_worker_entries_len(req.workers.as_deref())
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e))?;
        let count = req.count.unwrap_or_else(|| workers_len.unwrap_or(1) as i32);
        let isolate = req.isolate.unwrap_or(false);
        let mut worker_names: Vec<String> = req
            .worker_names
            .as_ref()
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
        //
        // cas-549c (GH #96): this runs BEFORE the epic gate, because a
        // dispatchable task_id is itself sufficient authorization to spawn.
        // `None` = a task_id was supplied but may not stand in for an epic; the
        // string says why, and is only surfaced when there is also no epic, so
        // epic-present behaviour is unchanged.
        let mut task_id_authorization: Option<Result<(), String>> = None;
        let mut stale_assignee_notice = String::new();
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
                    // cas-549c: `get` cannot distinguish "no such row" from a
                    // store fault, and this is now the first fallible read on
                    // the task_id path — so do not assert "not found".
                    format!(
                        "task_id {task_id} could not be read (no such task, or the task store is \
                         unavailable): {e}"
                    ),
                )
            })?;
            if task.is_terminal() {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "task_id {task_id} is terminal ({}) — cannot pre-assign it to a spawned worker.",
                        task.status
                    ),
                ));
            }

            // cas-2327 (GH #170): a display-name assignee can outlive its
            // worker session. Apply the same dual liveness rule that the
            // factory roster uses: a fresh heartbeat OR a live harness process
            // owns the task; a missing/dead row is stale and will be reset at
            // pre-assignment time. Never boot a worker just to discover this.
            let stale_holder = match task.assignee.as_deref() {
                Some(assignee) => {
                    let agents = open_agent_store(&self.inner.cas_root).map_err(|e| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!(
                                "Failed to inspect assignee {assignee} for task {task_id}: {e}"
                            ),
                        )
                    })?;
                    // `Task.assignee` is a worker display name, whereas
                    // `AgentStore::get` is keyed by opaque agent ID. Resolve
                    // every same-name row: a live respawn must not be hidden
                    // by an older stale registration. A registry read failure
                    // is uncertain ownership, so fail closed rather than
                    // stealing the task.
                    let registered_agents = agents.list(None).map_err(|e| {
                        Self::error(
                            ErrorCode::INTERNAL_ERROR,
                            format!(
                                "Failed to list assignee {assignee} for task {task_id}: {e}; \
                                 refusing replacement spawn while liveness is uncertain"
                            ),
                        )
                    })?;
                    let alive = crate::mcp::tools::service::agent_liveness::has_live_agent_named(
                        &registered_agents,
                        assignee,
                    );
                    if alive {
                        return Err(Self::error(
                            ErrorCode::INVALID_REQUEST,
                            format!(
                                "task {task_id} is already assigned to live worker '{assignee}'; \\
                                 refusing to spawn another worker for it"
                            ),
                        ));
                    }
                    stale_assignee_notice = format!(
                        "\nStale assignee '{assignee}' was detected; its task binding will be \\
                         force-released with reset semantics before pre-assignment."
                    );
                    true
                }
                None => false,
            };

            // Standing in for an epic is a stronger claim than being a legal
            // pre-assignment target, so it is held to a stricter bar: the task
            // must be work a NEW worker can actually pick up. A task parked in
            // AwaitingMerge is finished and waiting on
            // the supervisor; a Blocked task cannot be started; a task that
            // already has an assignee belongs to another worker, and
            // `assign_task_to_new_worker` will refuse to steal it — the spawned
            // worker would boot a pane and worktree only to sit permanently
            // idle. Everything below only ever *withholds* the epic bypass; the
            // pre-assignment rules above are unchanged, so a request made with
            // an open epic behaves exactly as before.
            task_id_authorization = Some(match (&task.status, &task.assignee) {
                (TaskStatus::Open | TaskStatus::InProgress, None) => Ok(()),
                (TaskStatus::Open | TaskStatus::InProgress, Some(_)) if stale_holder => Ok(()),
                (TaskStatus::Open | TaskStatus::InProgress, Some(assignee)) => Err(format!(
                    "task {task_id} is already assigned to '{assignee}', so it cannot stand in \
                     for an EPIC — that worker owns it and the pre-assignment would be refused."
                )),
                (status, _) => Err(format!(
                    "task {task_id} is {status:?}, which is not work a newly spawned worker can \
                     pick up, so it cannot stand in for an EPIC."
                )),
            });
        }

        // The epic gate exists to stop *unscoped* spawning — workers summoned
        // with no stated work, which is how a factory ends up with idle panes
        // and no plan. A concrete open task_id already states the work, so it
        // satisfies that intent on its own (cas-549c, GH #96). Requiring an
        // epic anyway forced a ceremonial single-child epic for every
        // post-epic follow-up, which distorts epic reporting and the
        // "all subtasks closed → verify and close the epic" flow.
        let queue = open_spawn_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spawn queue: {e}"),
            )
        })?;

        // A task authorizes ONE spawn. Nothing here mutates the task — binding
        // happens later, at worker registration — so without this check the same
        // open task_id could authorize an unbounded burst of epic-free spawns,
        // where only the first worker ever binds and the rest boot into
        // permanent idleness. Scoped to the bypass path so an epic-backed
        // factory keeps its existing (re-issuable) behaviour.
        if matches!(task_id_authorization, Some(Ok(())))
            && let Some(ref task_id) = req.task_id
        {
            let already_queued = queue
                .peek(SPAWN_QUEUE_DUPLICATE_SCAN)
                .map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to inspect spawn queue: {e}"),
                    )
                })?
                .into_iter()
                .any(|entry| entry.task_id.as_deref() == Some(task_id.as_str()));
            if already_queued {
                task_id_authorization = Some(Err(format!(
                    "a spawn for task {task_id} is already queued and has not been consumed yet, \
                     so it cannot authorize a second worker. Wait for that worker to register, or \
                     open an EPIC if you really want more workers."
                )));
            }
        }

        if !matches!(task_id_authorization, Some(Ok(()))) {
            let has_open_epic = task_store
                .list(None)
                .map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to list tasks: {e}"),
                    )
                })?
                .into_iter()
                .any(|t| t.task_type == TaskType::Epic && !t.is_terminal());

            if !has_open_epic {
                let why = match task_id_authorization {
                    Some(Err(reason)) => reason,
                    _ => "no task_id was supplied.".to_string(),
                };
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    no_active_epic_guidance(&why),
                ));
            }
        }

        let slots = if worker_names.is_empty() {
            count as usize
        } else {
            worker_names.len()
        };
        // Resolve a concrete WorkerSpec per queued worker. Batch-level fields
        // remain the resolver defaults; `workers=[{...}]` is its final,
        // per-slot layer.
        let (mut specs, lane_recipe, lane_warnings) = if let Some(lane) = req.lane.as_deref() {
            validate_lane_request(
                lane,
                req.cli.is_some(),
                req.model.is_some(),
                req.effort.is_some(),
            )
            .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
            let snapshot = tokio::task::spawn_blocking(
                crate::factory_preflight::collect_live_capability_snapshot,
            )
            .await
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("failed to collect lane capability snapshot: {error}"),
                )
            })?;
            build_lane_spawn_specs(
                slots,
                lane,
                req.config_dir.as_deref(),
                req.workers.as_deref(),
                &snapshot,
            )
            .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error))?
        } else {
            let specs = build_spawn_specs_with_project_config(
                slots,
                req.cli.as_deref(),
                req.model.as_deref(),
                req.effort.as_deref(),
                req.config_dir.as_deref(),
                req.workers.as_deref(),
                Some(self.inner.cas_root.join("config.toml")),
            )
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e))?;
            (specs, String::new(), Vec::new())
        };

        // Hosted DashScope is an explicit route with its own auth/model
        // preflight.  This is intentionally after spec resolution and before
        // queue insertion: an invalid hosted key cannot leave an apparently
        // runnable queue row, while local OpenCode rows remain unaffected.
        preflight_hosted_opencode_specs(&specs)
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e))?;

        // `workers[].name` is optional, but when every slot names itself it
        // is equivalent to worker_names= and must become the actual visible
        // worker identity rather than merely receipt decoration.
        if worker_names.is_empty() && specs.iter().all(|spec| spec.name.is_some()) {
            worker_names = specs.iter().filter_map(|spec| spec.name.clone()).collect();
        }

        // Capture the requesting account separately for each resolved
        // harness; a Claude supervisor's profile must never become a Codex
        // worker's CODEX_HOME (or vice versa).
        for spec in &mut specs {
            spec.requester_config_dir = requester_account_dir(spec.cli);
            spec.requester_secure_storage_dir = requester_secure_storage_dir(spec.cli);
            if spec.cli == cas_mux::SupervisorCli::Codex
                && let Some(config_dir) = spec.config_dir.as_deref()
            {
                preflight_codex_config_dir(config_dir)
                    .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error))?;
            }
        }

        // A caller who requested Codex must either receive Codex or an
        // actionable refusal. Rewriting the provider/tier to Claude here is
        // a cost and capability change the supervisor did not approve.
        let strict_cli = true;
        let notices = if req.lane.is_some() {
            // Lane resolution owns capability-aware substitution and warning
            // semantics. The legacy fallback must not rewrite a selected lane
            // route after its receipt has been constructed.
            Vec::new()
        } else {
            cas_factory::apply_codex_fallback(
                &mut specs,
                strict_cli,
                Some(default_worker_model_for_cli(cas_mux::SupervisorCli::Claude)),
            )
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e.to_string()))?
        };
        let mut config_dir_warnings = Vec::new();
        for spec in &specs {
            if spec.cli == cas_mux::SupervisorCli::Claude
                && let Some(config_dir) = spec.config_dir.as_deref()
            {
                preflight_claude_config_dir(config_dir)
                    .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error))?;
            }
            if (spec.config_dir.is_some() || spec.requester_config_dir.is_some())
                && !account_dir_supported(spec.cli)
            {
                config_dir_warnings.push(format!(
                    "config_dir has no account plumbing for {}; the resolved worker will not receive an account directory",
                    spec.cli.backend().name()
                ));
            }
        }
        // cas-8a55: the last gate before this request becomes worktrees. A
        // logged-out account is refused here, by name, instead of producing
        // workers that register, take their task and die on the first turn
        // while still heartbeating.
        preflight_account_auth(&specs).map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e))?;
        for notice in notices.iter().chain(config_dir_warnings.iter()) {
            tracing::warn!(target: "cas::factory", "{notice}");
        }
        let codex_fallback_notice = (!notices.is_empty())
            .then(|| format!("\nWarning: {}", notices.join("\nWarning: ")))
            .unwrap_or_default();
        let config_dir_notice = (!config_dir_warnings.is_empty())
            .then(|| format!("\nWarning: {}", config_dir_warnings.join("\nWarning: ")))
            .unwrap_or_default();
        // Keep the legacy single-WorkerSpec queue payload for a uniform batch
        // that did not request MCP per-worker overrides. Besides preserving
        // existing consumers, this lets the daemon retain its inexpensive
        // clone-at-launch path. A project `[[factory.workers]]` difference
        // (including a configured name/account) requires the resolved vector.
        let legacy_single_spec_payload = req.workers.is_none()
            && specs
                .first()
                .is_some_and(|first| specs.iter().all(|spec| spec == first));
        let spec_json_owned = if legacy_single_spec_payload {
            serde_json::to_string(&specs[0])
        } else {
            serde_json::to_string(&specs)
        }
        .map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("failed to serialize worker specs: {e}"),
            )
        })?;

        let spec_summary = spawn_specs_summary(&specs, &worker_names);
        let lane_notice = req
            .lane
            .as_deref()
            .map(|lane| {
                let mut notice =
                    format!("\nLane request: lane={lane} resolved recipe={lane_recipe}");
                for warning in &lane_warnings {
                    notice.push_str(&format!("\nWarning: {warning}"));
                }
                notice
            })
            .unwrap_or_default();
        let spec_warning = spawn_warning_for_request(
            req.lane.is_some(),
            req.model.is_some(),
            req.effort.is_some(),
            legacy_single_spec_payload,
            &spec_json_owned,
            &specs,
        );
        let isolation_warning = if isolate {
            String::new()
        } else {
            "\nWARNING — NON-ISOLATED SHARED-CHECKOUT RISK: every spawned worker uses the same working directory and mutable HEAD. Another worker can switch HEAD between tool calls, causing commits to land on a foreign factory branch; an explicit HEAD:<mine> push can graft that worker's commits onto the caller's remote branch; and SKILL.md guidance can change on disk mid-session. Prefer isolate=true. Commit/merge/push guards will refuse unless the checkout is still on the calling worker's exact factory/<name> branch.".to_string()
        };

        // GH #699: spawning while a second supervisor session is live on this
        // clone puts two fleets on one `.cas/` state, where either supervisor's
        // reset/merge/shutdown can reap the other's workers. Say so in the
        // receipt, before the workers exist rather than after one is reaped.
        //
        // A warning, not a refusal: an operator may deliberately run two panes
        // on one checkout, and grounding their spawn on a heuristic would be a
        // worse failure than an unmissable notice.
        let shared_clone_notice = open_agent_store(&self.inner.cas_root)
            .ok()
            .and_then(|store| store.list(None).ok())
            .and_then(|agents| {
                crate::factory_supervisor_overlap::shared_clone_warning(
                    &agents,
                    &self.inner.cas_root,
                    chrono::Utc::now(),
                )
            })
            .map(|warning| format!("\nWARNING — SHARED-CLONE SUPERVISOR OVERLAP: {warning}"))
            .unwrap_or_default();

        let factory_session = current_factory_session();
        if let Some(delivery_mode) = requested_delivery_mode {
            let session = factory_session.as_deref().ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "delivery_mode requires an active factory session (CAS_FACTORY_SESSION is not set)",
                )
            })?;
            persist_session_metadata_delivery_mode_at(&metadata_path(session), delivery_mode)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("failed to persist factory delivery mode: {error}"),
                    )
                })?;
        }
        let delivery_mode_notice = requested_delivery_mode
            .map(|mode| {
                format!(
                    "\nDelivery mode: {mode} (workers commit locally; supervisor merges their factory branches)"
                )
            })
            .unwrap_or_default();
        // Legacy rows carry requester selectors in their own columns. New
        // multi-worker rows persist the provider-correct selectors on every
        // WorkerSpec, so the daemon never flattens distinct accounts again.
        let requester_config_dir = (specs.len() == 1)
            .then(|| specs[0].requester_config_dir.as_deref())
            .flatten();
        let requester_secure_storage_dir = (specs.len() == 1)
            .then(|| specs[0].requester_secure_storage_dir.as_deref())
            .flatten();
        let request_id = queue
            .enqueue_spawn_with_requester_account_dirs(
                count,
                &worker_names,
                isolate,
                Some(spec_json_owned.as_str()),
                factory_session.as_deref(),
                req.task_id.as_deref(),
                requester_config_dir,
                requester_secure_storage_dir,
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
            .map(|id| {
                format!(
                    "\nTask: {id} will be pre-assigned once the worker boots{stale_assignee_notice}"
                )
            })
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
                (
                    "delivery_mode",
                    requested_delivery_mode
                        .map(|mode| mode.to_string())
                        .as_deref()
                        .unwrap_or(""),
                ),
            ],
        );

        // GH #60: the receipt confirms queue insertion and NOTHING about
        // liveness — it had the same shape whether a worker registered or the
        // queue was never consumed at all. Say so, and name the request id as
        // the handle to resolve it with, so a caller cannot read "Queued" as
        // "dispatched".
        let liveness_note = format!(
            "\nNOT YET CONFIRMED: this only means the request was queued. Call worker_status \
             to resolve request {request_id} to a worker and a state (registered / FAILED); \
             do not report dispatch complete until it shows registered."
        );
        // Recall is response-only: use the active epic as the task-planning
        // query and add nothing when the shared BM25 index has no useful
        // project-local memory/epic result.
        let related_context = task_store
            .list(None)
            .ok()
            .and_then(|tasks| {
                tasks.into_iter().find(|task| {
                    task.task_type == TaskType::Epic && task.status != TaskStatus::Closed
                })
            })
            .and_then(|epic| {
                self.inner
                    .related_recall(&format!("{} {}", epic.title, epic.description))
            })
            .unwrap_or_default();

        let msg = if worker_names.is_empty() {
            format!(
                "Queued spawn request for {count} worker(s) (request ID: {request_id})\nWorker spec: {spec_summary}{lane_notice}{spec_warning}{codex_fallback_notice}{config_dir_notice}{isolation_warning}{shared_clone_notice}{delivery_mode_notice}{task_id_note}{liveness_note}{related_context}"
            )
        } else {
            format!(
                "Queued spawn request for worker(s): {} (request ID: {})\nWorker spec: {spec_summary}{lane_notice}{spec_warning}{codex_fallback_notice}{config_dir_notice}{isolation_warning}{shared_clone_notice}{delivery_mode_notice}{task_id_note}{liveness_note}{related_context}",
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
        use crate::store::{open_agent_store, open_spawn_queue_store, open_task_store};
        use cas_store::SpawnLifecycleState;
        use cas_types::{AgentRole, AgentStatus};

        let requested_names: Vec<String> = req
            .worker_names
            .as_deref()
            .map(|names| {
                names
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let requested_id = req.id.as_deref().map(str::trim).filter(|id| !id.is_empty());
        if requested_id.is_some() && (!requested_names.is_empty() || req.count.is_some()) {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "shutdown_workers accepts exactly one target form: id=, worker_names=, or count=. Nothing was queued.",
            ));
        }
        if req.count.is_some_and(|count| count < 0) {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "shutdown_workers count must be >= 0. Nothing was queued.",
            ));
        }

        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;
        let owned = supervisor_owned_workers();
        let factory_session = current_factory_session();
        let queue = open_spawn_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open spawn queue: {e}"),
            )
        })?;
        // A launched PTY has a durable lifecycle row before it has an Agent
        // row. Keep that control-plane identity available to shutdown instead
        // of making a wedged pre-registration process untargetable.
        let launched_workers = factory_session
            .as_deref()
            .map(|session| queue.recent_spawn_lifecycle(session, 200))
            .transpose()
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list launched workers: {e}"),
                )
            })?
            .unwrap_or_default()
            .into_iter()
            .filter(|spawn| {
                spawn.state == SpawnLifecycleState::Launched && spawn.worker_name.is_some()
            })
            .collect::<Vec<_>>();
        let all_agents = agent_store.list(None).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list agents: {e}"),
            )
        })?;
        // Every registered worker, before scope/ownership filtering. Used only
        // to tell "unknown" apart from "refused" in the error path (cas-2c05).
        let all_registry_workers: Vec<cas_types::Agent> = all_agents
            .iter()
            .filter(|agent| agent.role == AgentRole::Worker)
            .cloned()
            .collect();
        let (known_workers, _) = dedupe_authoritative_agents(
            all_agents
                .into_iter()
                .filter(|agent| {
                    agent.role == AgentRole::Worker
                        && matches!(
                            agent.status,
                            AgentStatus::Active | AgentStatus::Idle | AgentStatus::Stale
                        )
                        && agent.visible_to_factory_session(factory_session.as_deref())
                        && owned
                            .as_ref()
                            .is_none_or(|names| names.contains(&agent.name))
                })
                .collect(),
        );

        // Resolve every target now and queue exact names. An omitted/ignored
        // selector can therefore never broaden later inside the daemon.
        let selected: Vec<cas_types::Agent> = if let Some(id) = requested_id {
            known_workers
                .iter()
                .find(|worker| worker_answers_to(worker, id))
                .cloned()
                .into_iter()
                .collect()
        } else if !requested_names.is_empty() {
            // cas-2c05: name OR agent id OR session id, through the one shared
            // definition. Matching names only here — while the id selector
            // accepted both — is why a session id was reported as unknown.
            requested_names
                .iter()
                .filter_map(|name| {
                    known_workers
                        .iter()
                        .find(|worker| worker_answers_to(worker, name))
                        .cloned()
                })
                .collect()
        } else {
            let limit = req.count.unwrap_or(0) as usize;
            known_workers
                .iter()
                .take(if limit == 0 {
                    known_workers.len()
                } else {
                    limit
                })
                .cloned()
                .collect()
        };

        let selected_launched = select_launched_shutdown_targets(
            &launched_workers,
            requested_id,
            &requested_names,
            req.count,
        );
        let selected_launched_names: Vec<String> = selected_launched
            .iter()
            .filter_map(|spawn| spawn.worker_name.clone())
            .filter(|name| !selected.iter().any(|worker| worker.name == *name))
            .collect();

        let missing: Vec<String> = if let Some(id) = requested_id {
            if selected.is_empty() && selected_launched.is_empty() {
                vec![id.to_string()]
            } else {
                Vec::new()
            }
        } else {
            requested_names
                .iter()
                .filter(|name| {
                    !selected.iter().any(|worker| worker_answers_to(worker, name))
                        && !selected_launched_names
                            .iter()
                            .any(|launched| launched.eq_ignore_ascii_case(name))
                })
                .cloned()
                .collect()
        };
        if !missing.is_empty() || (selected.is_empty() && selected_launched_names.is_empty()) {
            let known = known_workers
                .iter()
                .map(|worker| format!("{} ({})", worker.name, worker.id))
                .chain(selected_launched.iter().filter_map(|spawn| {
                    spawn
                        .worker_name
                        .as_ref()
                        .map(|name| format!("{name} (launched; spawn request {})", spawn.id))
                }))
                .collect::<Vec<_>>();
            // cas-2c05: never assert that a target is unknown while listing it
            // as known in the same breath. A target that IS in the registry but
            // was not selectable was refused by a policy (scope/ownership), and
            // saying so is the difference between a fixable message and one that
            // costs a supervisor an investigation.
            let refused: Vec<String> = missing
                .iter()
                .filter(|target| {
                    all_registry_workers
                        .iter()
                        .any(|worker| worker_answers_to(worker, target))
                })
                .cloned()
                .collect();
            let genuinely_absent: Vec<String> = missing
                .iter()
                .filter(|target| !refused.contains(target))
                .cloned()
                .collect();
            if !refused.is_empty() {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Worker target(s) refused, not unknown: {}. They are registered but \
                         outside this session's shutdown scope (a different factory session \
                         owns them, or they are not workers of yours). Nothing was queued. \
                         Known workers in scope: {}.",
                        refused.join(", "),
                        if known.is_empty() {
                            "(none)".to_string()
                        } else {
                            known.join(", ")
                        }
                    ),
                ));
            }
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "Worker target(s) not found: {}. Known workers: {}. Nothing was queued.",
                    if genuinely_absent.is_empty() {
                        "(none selected)".to_string()
                    } else {
                        genuinely_absent.join(", ")
                    },
                    if known.is_empty() {
                        "(none)".to_string()
                    } else {
                        known.join(", ")
                    }
                ),
            ));
        }

        let task_store = open_task_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open task store for shutdown safety check: {e}"),
            )
        })?;
        let tasks = task_store.list(None).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list tasks for shutdown safety check: {e}"),
            )
        })?;
        let snapshots: Vec<ShutdownWorkerSnapshot> = selected
            .iter()
            .map(|worker| shutdown_worker_snapshot(&self.inner.cas_root, worker, &tasks))
            .collect();
        let force = req.force.unwrap_or(false);
        let unsafe_snapshots: Vec<&ShutdownWorkerSnapshot> = snapshots
            .iter()
            .filter(|snapshot| snapshot.requires_force())
            .collect();
        if !force && !unsafe_snapshots.is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "shutdown_workers refused: selected worker state requires force=true. Nothing was queued.\n{}",
                    unsafe_snapshots
                        .iter()
                        .map(|snapshot| format!("- {}", snapshot.render()))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            ));
        }

        let worker_names: Vec<String> = selected
            .iter()
            .map(|worker| worker.name.clone())
            .chain(selected_launched_names.iter().cloned())
            .collect();

        // Validation passed: queue only the exact resolved names. `count=None`
        // is intentional — the daemon must not re-expand this decision.

        let request_id = queue
            .enqueue_shutdown(None, &worker_names, force, factory_session.as_deref())
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to queue shutdown request: {e}"),
                )
            })?;
        let request_id_text = request_id.to_string();
        let count_text = req.count.map(|value| value.to_string()).unwrap_or_default();
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

        let request_scope = if requested_id.is_none()
            && requested_names.is_empty()
            && req.count.unwrap_or(0) == 0
        {
            "ALL workers".to_string()
        } else {
            format!("worker(s): {}", worker_names.join(", "))
        };
        let msg = format!(
            "Queued shutdown request for {request_scope} (request ID: {request_id})\nExact affected workers and request-time state:\n{}",
            snapshots
                .iter()
                .map(|snapshot| format!("- {}", snapshot.render()))
                .collect::<Vec<_>>()
                .join("\n")
        );

        Ok(Self::success(msg))
    }

    /// Arm or release the director's session-scoped worker hold gate.
    ///
    /// The environment-derived role check is the same workflow guardrail used
    /// by other supervisor-only operations. It is not an adversarial security
    /// boundary; factory process ownership remains the trust boundary.
    pub(super) async fn factory_set_worker_hold(
        &self,
        req: FactoryRequest,
        held: bool,
    ) -> Result<CallToolResult, McpError> {
        use crate::harness_policy::is_supervisor_from_env;
        use crate::store::{open_agent_store, open_reminder_store};
        use crate::ui::factory::{metadata_path, persist_session_metadata_worker_hold_at};
        use cas_types::{AgentRole, AgentStatus};

        let action = if held {
            "hold_worker"
        } else {
            "release_worker"
        };
        worker_hold_role_gate(is_supervisor_from_env(), action)
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;

        let factory_session = current_factory_session().ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                format!(
                    "{action} requires an active factory session (CAS_FACTORY_SESSION is not set)"
                ),
            )
        })?;
        let worker_name = req
            .target
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("{action} requires target=<worker-name>"),
                )
            })?;

        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {error}"),
            )
        })?;
        let owned = supervisor_owned_workers();
        let worker = agent_store
            .list(None)
            .map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list workers: {error}"),
                )
            })?
            .into_iter()
            .find(|agent| {
                agent.role == AgentRole::Worker
                    && matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
                    && agent.name == worker_name
                    && agent.visible_to_factory_session(Some(&factory_session))
                    && owned
                        .as_ref()
                        .is_none_or(|workers| workers.contains(worker_name))
            });
        let Some(worker) = worker else {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "Worker {worker_name:?} is not a live member of factory session {factory_session}"
                ),
            ));
        };

        // Hold semantics deliberately cancel rather than suspend: reminders
        // are one-shot watchdogs whose original timing and event context are
        // stale once a supervisor has explicitly parked the worker. Release
        // restores normal future signaling but never resurrects old waits.
        let cancelled_reminders = if held {
            let reminders = open_reminder_store(&self.inner.cas_root).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open reminder store: {error}"),
                )
            })?;
            reminders
                .cancel_pending_for_target(&worker.id)
                .map_err(|error| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to cancel pending worker reminders: {error}"),
                    )
                })?
        } else {
            0
        };

        let path = metadata_path(&factory_session);
        persist_session_metadata_worker_hold_at(&path, worker_name, held).map_err(|error| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to update worker hold state: {error}"),
            )
        })?;

        let event_type = if held {
            "worker_hold_armed"
        } else {
            "worker_hold_released"
        };
        let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
            &self.inner.cas_root,
            event_type,
            &[
                ("worker", worker_name),
                ("factory_session", &factory_session),
            ],
        );

        let verb = if held { "Held" } else { "Released" };
        let reminder_summary = if held {
            format!(
                " Cancelled {cancelled_reminders} pending reminder(s) targeted to this worker; release does not revive cancelled one-shot reminders."
            )
        } else {
            String::new()
        };
        Ok(Self::success(format!(
            "{verb} worker {worker_name} for factory session {factory_session}. Hold state survives a daemon restart of this session and is cleared on worker removal or session shutdown.{reminder_summary}"
        )))
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
        let activity_cutoff =
            chrono::Utc::now() - chrono::Duration::seconds(WORKER_ACTIVITY_WINDOW_SECS);
        let activity_events: Vec<cas_types::Event> = {
            use cas_store::{EventStore, SqliteEventStore};
            SqliteEventStore::open(&self.inner.cas_root)
                .and_then(|es| es.list_since(activity_cutoff, 200))
                .map(|events| {
                    events
                        .into_iter()
                        .filter(is_worker_activity_event)
                        .collect()
                })
                .unwrap_or_default()
        };
        // File-write telemetry is rendered as an absolute per-worker
        // timestamp, so it must not disappear merely because the broader
        // activity line uses a ten-minute freshness window. Read a bounded
        // recent slice of that event type separately.
        let file_write_events: Vec<cas_types::Event> = {
            use cas_store::{EventStore, SqliteEventStore};
            SqliteEventStore::open(&self.inner.cas_root)
                .and_then(|events| {
                    events.list_by_type(
                        cas_types::EventType::WorkerFileEdited,
                        WORKER_FILE_WRITE_SCAN_LIMIT,
                    )
                })
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
        let held_workers = factory_session
            .as_deref()
            .and_then(crate::ui::factory::worker_holds_from_session_metadata_named)
            .unwrap_or_default();
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
                    if let Some(session) = factory_session.as_deref() {
                        mark_recent_registered_spawn_failed(
                            &self.inner.cas_root,
                            session,
                            &agent.name,
                            "Harness process exited during post-registration boot verification; worker_status observed a dead PID.",
                        );
                    }
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

        // A shared `cas serve` process can keep heartbeating after the
        // interactive harness has exited. Fresh heartbeat data must not make
        // that dead harness look active until the daemon's next heartbeat
        // tick; worker_status is an operator poll and can resolve the typed
        // PID fingerprint immediately. Legacy rows without a PID retain the
        // heartbeat-only behavior.
        if let Ok(active_agents) = store.list(Some(AgentStatus::Active)) {
            for agent in active_agents {
                if !agent.visible_to_factory_session(factory_session.as_deref())
                    || agent.role != AgentRole::Worker
                    || agent.pid.is_none()
                    || agent_process_is_alive(&agent)
                {
                    continue;
                }
                let held: Vec<String> = store
                    .list_agent_leases(&agent.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|lease| lease.task_id)
                    .collect();
                if store.mark_stale(&agent.id).is_ok() {
                    stale_pruned += 1;
                    if let Some(session) = factory_session.as_deref() {
                        mark_recent_registered_spawn_failed(
                            &self.inner.cas_root,
                            session,
                            &agent.name,
                            "Harness process exited during post-registration boot verification; worker_status observed a dead PID.",
                        );
                    }
                    let _ = super::orphan_recovery::recover_worker_vanished(
                        &self.inner.cas_root,
                        store.as_ref(),
                        &agent,
                        &held,
                        "worker_status process liveness check (harness exited)",
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
        let mut duplicate_names = std::collections::BTreeSet::new();
        let mut seen_identities = std::collections::BTreeSet::new();
        for agent in &agents {
            let identity = (agent.role.to_string(), agent.name.clone());
            if !seen_identities.insert(identity) {
                duplicate_names.insert(agent.name.clone());
            }
        }
        let (agents, duplicate_registrations_filtered) = dedupe_authoritative_agents(agents);

        // cas-2e81: always surface recently-died-while-leased even when the
        // Active roster is empty — "None active" must not hide a mid-P0 crash.
        let live_worker_names: std::collections::HashSet<String> = agents
            .iter()
            .filter(|agent| agent.role == AgentRole::Worker)
            .map(|agent| agent.name.clone())
            .collect();
        let died_section = super::orphan_recovery::format_recently_died_while_leased(
            &self.inner.cas_root,
            store.as_ref(),
            factory_session.as_deref(),
            3600, // 1h window
            &live_worker_names,
        );

        // GH #60: recent spawn lifecycle, resolved once for both render paths.
        // Load it BEFORE the empty-roster early return — "no agents" is
        // precisely the case where a failed or unconsumed spawn is the answer,
        // and the old output said only "None active", which reads like an
        // empty fleet rather than a spawn that died.
        let spawn_section = current_factory_session()
            .and_then(|session| {
                crate::store::open_spawn_queue_store(&self.inner.cas_root)
                    .ok()
                    .and_then(|queue| queue.recent_spawn_lifecycle(&session, 10).ok())
            })
            .map(|rows| format_spawn_lifecycle_section(&rows, chrono::Utc::now()))
            .unwrap_or_default();

        // cas-7787 (GH #160): lifecycle relays that expired without ever
        // reaching the supervisor. This goes FIRST, above the roster, and is
        // rendered even when no agents are registered — the whole failure mode
        // being fixed is that a lost relay left no trace anywhere, so the
        // fleet read silence as "nothing is waiting on me".
        let undelivered_relays = crate::store::open_prompt_queue_store(&self.inner.cas_root)
            .ok()
            .map(|queue| {
                // A terminal task is positive evidence that the lifecycle
                // relay's requested supervisor action is moot. Reconcile it
                // durably (while retaining the forensic prompt row) before
                // sampling the remaining actionable backlog. Scan the same
                // bounded maximum as worker-death coalescing so task-free
                // informational deaths cannot occupy the ten-row display cap.
                let _ = queue.reconcile_terminal_lifecycle_relays();
                queue
                    .list_undelivered_lifecycle_relays(
                        super::orphan_recovery::WORKER_DEATH_COALESCE_SCAN_LIMIT,
                    )
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let undelivered_section = format_undelivered_relay_section(&undelivered_relays);

        // GH #699: `agents` is scoped to the caller's factory session, so a
        // second supervisor live on this same clone never appeared here — yet
        // both share this `.cas/` state and either one's reset, merge or
        // shutdown can reap the other's workers. Re-read the roster unscoped
        // so every surface below can name every live supervisor session on the
        // checkout, including the "no agents of my own" path that the second
        // supervisor lands on first.
        let now = chrono::Utc::now();
        let live_supervisor_sessions: Vec<_> = store
            .list(None)
            .map(|all| crate::factory_supervisor_overlap::live_supervisor_sessions(&all, now))
            .unwrap_or_default();
        let shared_clone_warning = (live_supervisor_sessions.len() > 1).then(|| {
            format!(
                "⚠ {}\n\n",
                crate::factory_supervisor_overlap::render_shared_clone_warning(
                    &live_supervisor_sessions,
                    &self.inner.cas_root,
                )
            )
        });

        if agents.is_empty() {
            let mut msg = String::from(
                "No active agents registered.\n\nNote: Factory TUI must be running for agents to be registered.",
            );
            if let Some(warning) = shared_clone_warning.as_deref() {
                msg.push_str("\n\n");
                msg.push_str(warning);
            }
            msg.push_str(&undelivered_section);
            msg.push_str(&spawn_section);
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
        output.push_str(&undelivered_section);
        if duplicate_registrations_filtered > 0 {
            output.push_str(&format!(
                "Collapsed {duplicate_registrations_filtered} superseded registry row(s) for: {}\n\n",
                duplicate_names.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }

        // cas-d165 (Finding 2): assignees of currently InProgress tasks,
        // resolved ONCE for the whole roster. `has_in_progress_task` below
        // must not rely on lease presence alone — leases are a fixed-
        // duration claim nothing in production renews (see the long
        // comment at the `has_in_progress_task` computation), so they
        // silently expire under a genuinely working agent. Real task
        // assignment is the ground truth; a lease is corroborating (and
        // currently the only) evidence *before* that expiry.
        // GH #67: keep the whole task, not just the assignee name. The
        // supervisor had to open the task store (or the worktree) to learn
        // WHICH task a worker was holding; the roster knew all along.
        //
        // cas-e728 (GH #105): every task list here is read at RENDER time, and
        // the lease check below is cross-referenced against them, so a task
        // that closed a second ago can never keep reading as in-progress.
        // cas-e728: a failed read must not masquerade as "nobody has a task".
        // Every worker's task line and stall verdict derives from these lists,
        // so silently defaulting to empty would turn one SQLITE_BUSY into a
        // whole status page of false reassurance.
        let mut task_read_failed = false;
        let all_in_progress_tasks: Vec<cas_types::Task> = {
            use crate::store::open_task_store;
            match open_task_store(&self.inner.cas_root)
                .map_err(|e| e.to_string())
                .and_then(|ts| {
                    ts.list(Some(cas_types::TaskStatus::InProgress))
                        .map_err(|e| e.to_string())
                }) {
                Ok(tasks) => tasks,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "cas-e728: worker_status could not read in-progress tasks"
                    );
                    task_read_failed = true;
                    Vec::new()
                }
            }
        };
        // cas-e728: the set of tasks that are in progress *right now*. A lease
        // is a fixed-duration row that outlives the work — closing a task
        // through any path that does not explicitly release it (a direct status
        // update, a supervisor-side close, a crashed worker) leaves the lease
        // on the books for the rest of its duration (default 30 minutes). Before
        // this set existed, that stale lease alone made `has_in_progress_task`
        // true, so `worker_status` told supervisors a finished worker had a task
        // "in progress" and rendered `⚠ STALLED ... while task in progress` at
        // it — the stale attribution behind the wrongful stall accusations in
        // the report.
        let mut unfinished_task_ids: std::collections::HashSet<String> =
            all_in_progress_tasks.iter().map(|t| t.id.clone()).collect();
        let in_progress_tasks: Vec<cas_types::Task> = all_in_progress_tasks
            .into_iter()
            .filter(|t| t.assignee.is_some())
            .collect();
        let in_progress_assignees: std::collections::HashSet<String> = in_progress_tasks
            .iter()
            .filter_map(|t| t.assignee.clone())
            .collect();
        // cas-e728 (GH #105): the finished-awaiting-merge state is the anomaly
        // the stall flag structurally cannot see — the worker is healthy, its
        // work is done, and it is waiting on the SUPERVISOR. It rendered as
        // "task: none assigned", indistinguishable from an idle worker with
        // nothing to do, so it read as "free" when it was actually blocking.
        let parked_tasks: Vec<cas_types::Task> = {
            use crate::store::open_task_store;
            open_task_store(&self.inner.cas_root)
                .ok()
                .map(|ts| {
                    [
                        cas_types::TaskStatus::AwaitingMerge,
                        // cas-e728: Blocked literally means "waiting on
                        // something". It is not in progress, so it must not
                        // count toward the stall verdict — but it must still be
                        // NAMED, or a blocked worker renders as idle-with-
                        // nothing-to-do, the same hole this fix closes for
                        // awaiting-merge.
                        cas_types::TaskStatus::Blocked,
                    ]
                    .into_iter()
                    .flat_map(|status| ts.list(Some(status)).unwrap_or_default())
                    .filter(|task| task.assignee.is_some())
                    .collect()
                })
                .unwrap_or_default()
        };
        // cas-78bf: retain assigned Open tasks (including their assignment
        // timestamp) so worker_status can distinguish the normal dispatch
        // grace window from a worker that has held work without ever
        // starting it past the configured stall threshold.
        let all_open_tasks: Vec<cas_types::Task> = {
            use crate::store::open_task_store;
            match open_task_store(&self.inner.cas_root)
                .map_err(|e| e.to_string())
                .and_then(|ts| {
                    ts.list(Some(cas_types::TaskStatus::Open))
                        .map_err(|e| e.to_string())
                }) {
                Ok(tasks) => tasks,
                Err(error) => {
                    tracing::warn!(%error, "cas-e728: worker_status could not read open tasks");
                    task_read_failed = true;
                    Vec::new()
                }
            }
        };
        // cas-e728: a lease taken between `claim` and `start` points at a task
        // that is still Open — that IS work in flight, so Open counts as
        // unfinished here. What must NOT count is a task that has reached a
        // terminal-for-the-worker state (Closed, AwaitingMerge,
        // AwaitingMerge): the work is over, and only the lease row
        // outlived it.
        unfinished_task_ids.extend(all_open_tasks.iter().map(|t| t.id.clone()));
        let assigned_open_tasks: Vec<cas_types::Task> = all_open_tasks
            .into_iter()
            .filter(|task| task.assignee.is_some())
            .collect();

        let workers: Vec<_> = agents
            .iter()
            .filter(|a| {
                a.role == AgentRole::Worker
                    && owned.as_ref().is_none_or(|set| set.contains(&a.name))
            })
            .collect();
        let outbound_targets: std::collections::BTreeSet<String> = agents
            .iter()
            .filter(|agent| {
                agent.role == AgentRole::Supervisor || agent.role == AgentRole::Director
            })
            .map(|agent| agent.name.clone())
            .chain(["supervisor".to_string(), "director".to_string()])
            .collect();
        let outbound_target_refs: Vec<&str> = outbound_targets.iter().map(String::as_str).collect();
        // cas-e728 (GH #105): per-worker inbox state — how many messages the
        // worker has NOT consumed, and how old the oldest one is.
        //
        // Keyed on recipient-seen state, NOT `processed_at`: the daemon stamps
        // `processed_at` the instant it hands a row to the transport, so that
        // column answers "has the daemon ticked", not "has the worker read
        // it" — counting it would report an empty inbox for precisely the case
        // this signal exists to catch (message delivered, worker never woke).
        // The shared store query also fans `all_workers` broadcasts out to
        // every recipient, so a broadcast nobody acted on can no longer render
        // as "inbox empty".
        //
        // Read-only by construction: a supervisor polling status must never
        // mark a worker's mail seen.
        //
        // cas-f08d (GH #147): the count alone was not enough. A fired reminder
        // is delivered through this same queue, and the wait pattern workers
        // are REQUIRED to follow (act once, arm a reminder, end the turn) never
        // calls `inbox_poll` — so a reminder the worker demonstrably acted on
        // stayed "unread" forever and read as a wedged harness. The rows are
        // therefore also read (never consumed) and joined against pending
        // reminders so the two can be told apart.
        let now = chrono::Utc::now();
        let pending_reminder_waits = pending_reminder_waits(&self.inner.cas_root, now);
        let prompt_queue = crate::store::open_prompt_queue_store(&self.inner.cas_root).ok();
        let inbox_state: std::collections::HashMap<String, WorkerInbox> = prompt_queue
            .as_ref()
            .map(|queue| {
                workers
                    .iter()
                    .map(|agent| {
                        let count = queue
                            .count_unseen_for_recipient(&agent.name, factory_session.as_deref())
                            .unwrap_or(0);
                        let oldest = queue
                            .oldest_unseen_age_secs_for_recipient(
                                &agent.name,
                                factory_session.as_deref(),
                            )
                            .unwrap_or(None);
                        let rows: Vec<UnseenDelivery> = queue
                            .peek_unseen_for_recipient(
                                &agent.name,
                                factory_session.as_deref(),
                                WORKER_INBOX_PEEK_LIMIT,
                            )
                            .unwrap_or_default()
                            .iter()
                            .map(UnseenDelivery::from_queued)
                            .collect();
                        let inbox = classify_worker_inbox(
                            count,
                            oldest,
                            &rows,
                            pending_reminder_waits.get(agent.id.as_str()).copied(),
                            now,
                        );
                        (agent.name.clone(), inbox)
                    })
                    .collect()
            })
            .unwrap_or_default();
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

        // The roster above is session-scoped; the unscoped read hoisted before
        // the empty-agents branch supplies every other live supervisor on this
        // clone (GH #699), so this section names them instead of hiding them.
        let rendered_names: std::collections::BTreeSet<&str> =
            supervisors.iter().map(|a| a.name.as_str()).collect();

        // cas-5087: naming the other live supervisors was only half the answer.
        // Before a gate an operator needs to know WHAT each of them is in the
        // middle of, because that is what decides whether a merge, reset or
        // shutdown collides with somebody's release.
        //
        // Read once for the whole block. A failed read is reported as
        // `Unavailable` on every row rather than silently rendering "no epic":
        // this surface is checked immediately before destructive actions, so a
        // missing signal must never read as a reassuring one — and it must
        // never fail the whole report either (cas-e728's rule for this page).
        let (supervisor_epics, supervisor_epic_read_failed) = {
            use crate::store::open_task_store;
            match open_task_store(&self.inner.cas_root).map_err(|e| e.to_string()) {
                Ok(ts) => {
                    let mut epics = Vec::new();
                    let mut failed = false;
                    for status in [
                        cas_types::TaskStatus::Open,
                        cas_types::TaskStatus::InProgress,
                        cas_types::TaskStatus::Blocked,
                    ] {
                        match ts.list(Some(status)) {
                            Ok(tasks) => epics.extend(
                                tasks
                                    .into_iter()
                                    .filter(|t| t.task_type == cas_types::TaskType::Epic),
                            ),
                            Err(error) => {
                                tracing::warn!(
                                    %error,
                                    ?status,
                                    "cas-5087: worker_status could not read epics for supervisor attribution"
                                );
                                failed = true;
                            }
                        }
                    }
                    (epics, failed)
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "cas-5087: worker_status could not open the task store for supervisor attribution"
                    );
                    (Vec::new(), true)
                }
            }
        };
        let supervisor_epic = |agent_id: &str, agent_name: &str, session: Option<&str>| {
            if supervisor_epic_read_failed {
                return crate::factory_supervisor_overlap::SupervisorEpic::Unavailable;
            }
            let focus = session
                .map(str::trim)
                .filter(|session| !session.is_empty())
                .and_then(crate::ui::factory::preferred_epic_id_from_session_metadata_named);
            crate::factory_supervisor_overlap::resolve_supervisor_epic(
                agent_id,
                agent_name,
                focus.as_deref(),
                &supervisor_epics,
            )
        };
        let actionable_idle_label = |session: Option<&str>| {
            session
                .and_then(crate::ui::factory::supervisor_progress_from_session_metadata_named)
                .map(|(_, tracker)| {
                    format!(
                        " [actionable-idle: {}m]",
                        tracker.actionable_idle_minutes_at(now)
                    )
                })
                .unwrap_or_default()
        };

        if !supervisors.is_empty() || live_supervisor_sessions.len() > 1 {
            output.push_str("Supervisors:\n");
            for agent in &supervisors {
                let elapsed = (now - agent.last_heartbeat).num_seconds();
                let since = format!("{elapsed}s ago");
                let epic = supervisor_epic(
                    &agent.id,
                    &agent.name,
                    agent
                        .factory_session
                        .as_deref()
                        .or(factory_session.as_deref()),
                );
                output.push_str(&format!(
                    "  • {} (heartbeat: {}){}{}\n",
                    &agent.name,
                    since,
                    epic.render(),
                    actionable_idle_label(
                        agent.factory_session.as_deref().or(factory_session.as_deref())
                    )
                ));
            }
            for session in &live_supervisor_sessions {
                if rendered_names.contains(session.name.as_str()) {
                    continue;
                }
                let elapsed = (now - session.last_heartbeat).num_seconds();
                let epic = supervisor_epic(&session.id, &session.name, session.session.as_deref());
                output.push_str(&format!(
                    "  • {} (heartbeat: {elapsed}s ago) [other session — shares this clone]{}{}\n",
                    session.label(),
                    epic.render(),
                    actionable_idle_label(session.session.as_deref())
                ));
            }
            output.push('\n');
        }

        if let Some(warning) = shared_clone_warning.as_deref() {
            output.push_str(warning);
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
                let held_label = if held_workers.contains(&agent.name) {
                    " [HELD]"
                } else {
                    ""
                };
                let worktree_status = collect_worker_worktree_status(&self.inner.cas_root, agent);
                let clone_path = worktree_status.clone_path;
                let clone_info = worktree_status.clone_info;
                // cas-844bf: git introspection — branch/HEAD/ahead-behind/dirty/PR
                let git_info = worktree_status.git_info;
                let session_uuid = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
                let worker_cli = worker_cli_from_agent(agent);
                // Claude can suspend a team worker before writing its
                // `tool_use` to the transcript. Read the native lead inbox
                // independently so status remains actionable for both the
                // destructive Bash classifier and heredoc/file-rewrite
                // cases observed in the factory.
                let pending_permission = (worker_cli == cas_mux::SupervisorCli::Claude)
                    .then(|| {
                        crate::cli::factory::wedged::pending_permission_for_worker(
                            &agent.name,
                            agent.factory_session.as_deref().or(factory_session.as_deref()),
                            agent.metadata.get("worker_account_dir").map(String::as_str),
                        )
                    })
                    .flatten();
                let opencode_observation = (worker_cli == cas_mux::SupervisorCli::OpenCode)
                    .then(|| {
                        crate::mcp::tools::service::opencode_liveness::observe(
                            &self.inner.cas_root,
                            session_uuid,
                            now.timestamp_millis().max(0) as u64,
                            agent_process_is_alive(agent),
                        )
                    })
                    .flatten();
                let liveness_label = if worker_cli == cas_mux::SupervisorCli::OpenCode {
                    match opencode_observation
                        .as_ref()
                        .map(|observation| observation.verdict)
                    {
                        Some(cas_mux::OpenCodeLivenessVerdict::Signal(
                            cas_mux::OpenCodeLiveness::Busy,
                        ))
                        | Some(cas_mux::OpenCodeLivenessVerdict::Signal(
                            cas_mux::OpenCodeLiveness::Idle,
                        ))
                        | Some(cas_mux::OpenCodeLivenessVerdict::ProcessAliveFallback) => {
                            " [alive — OpenCode mapped session]"
                        }
                        Some(cas_mux::OpenCodeLivenessVerdict::Signal(
                            cas_mux::OpenCodeLiveness::Error,
                        )) => " [OpenCode error signal]",
                        Some(cas_mux::OpenCodeLivenessVerdict::Signal(
                            cas_mux::OpenCodeLiveness::Deleted,
                        )) => " [OpenCode session deleted]",
                        _ => liveness_label,
                    }
                } else {
                    liveness_label
                };
                // Scan-based harness resolution is a bounded-TTL lookup after
                // the first poll for this worker. Keep the rich result so the
                // same lookup feeds context/activity/in-flight evidence and
                // hard-dead salvage diagnostics. Claude retains its single
                // stat fast path.
                let transcript_resolution_for_worker =
                    worker_status_uses_scanned_transcript(worker_cli).then(|| {
                        worker_status_cached_transcript_resolution_for_account(
                            clone_path.as_deref(),
                            session_uuid,
                            worker_cli,
                            agent.metadata.get("worker_account_dir").map(String::as_str),
                        )
                    });
                let transcript_path_for_worker = transcript_resolution_for_worker
                    .as_ref()
                    .and_then(|cached| {
                        worker_status_path_from_resolution(cached.resolution.clone(), worker_cli)
                    })
                    .or_else(|| {
                        (worker_cli == cas_mux::SupervisorCli::Claude)
                            .then(|| transcript_path_fast(clone_path.as_deref(), session_uuid))
                            .flatten()
                    });
                // A Codex MCP server can keep the registry heartbeat fresh
                // after the interactive harness has terminally rejected every
                // turn for exhausted account credits. Prefer the harness's
                // own terminal record over that process-only heartbeat.
                let usage_limited = codex_rollout_reports_usage_limit(
                    transcript_path_for_worker.as_deref(),
                    worker_cli,
                );
                // cas-4fb9 / cas-71d9: checkpoint/heartbeat freshness cannot
                // answer whether the harness actually started a turn. Render
                // the harness's own artifact-backed turn observation
                // separately; Claude's transcript is now authoritative too.
                let harness_turn_info = if worker_cli == cas_mux::SupervisorCli::OpenCode {
                    String::new()
                } else {
                    format_harness_turn_observation(
                        worker_cli,
                        transcript_path_for_worker.as_deref(),
                    )
                };
                let turn_start_observable = if worker_cli == cas_mux::SupervisorCli::OpenCode {
                    opencode_observation
                        .as_ref()
                        .is_some_and(|observation| observation.state.last_activity_at.is_some())
                } else {
                    harness_publishes_turn_start(worker_cli) && transcript_path_for_worker.is_some()
                };
                // Surface transcript path only for hard-dead workers so the
                // supervisor can salvage whatever was in-flight when the CC
                // client died (cas-2749 AC: transcript-path-surfacing on
                // crash). The `[stale]` tier does NOT emit the transcript —
                // a worker lagging past 30s under scheduler jitter does not
                // need its transcript surfaced yet, and emitting it there
                // would produce the false-positive noise cas-8240 is fixing.
                //
                // cas-900b: when we do emit, render the rich harness-aware
                // resolution. It surfaces the real path, a labelled likely
                // path, or every ambiguous candidate, plus session_id so a
                // supervisor can independently search the transcript tree.
                //
                // In factory mode, `agent.id` is the CC SessionStart UUID
                // (daemon.rs + server/mod.rs both construct the Agent via
                // `Agent::new(session_id, name)`). If `cc_session_id` is
                // ever populated separately in the future we prefer it —
                // but for now `id` is the right key and has been correct
                // since cas-2749.
                let transcript_info = if worker_cli == cas_mux::SupervisorCli::OpenCode {
                    let mapped_live = opencode_observation.as_ref().is_some_and(|observation| {
                        matches!(
                            observation.verdict,
                            cas_mux::OpenCodeLivenessVerdict::Signal(
                                cas_mux::OpenCodeLiveness::Busy
                            ) | cas_mux::OpenCodeLivenessVerdict::Signal(
                                cas_mux::OpenCodeLiveness::Idle
                            ) | cas_mux::OpenCodeLivenessVerdict::ProcessAliveFallback
                        )
                    });
                    if elapsed < WORKER_DEAD_SECS || mapped_live {
                        String::new()
                    } else {
                        opencode_observation.as_ref().map_or_else(
                            || "\n    OpenCode export: mapping unavailable/delayed".to_string(),
                            |observation| {
                                crate::mcp::tools::service::opencode_liveness::export_session(
                                    observation,
                                    agent.metadata.get("worker_account_dir").map(String::as_str),
                                )
                                .map(|export| {
                                    format!(
                                        "\n    OpenCode export (bounded, mapped session):\n        {}",
                                        export.replace('\n', "\n        ")
                                    )
                                })
                                .unwrap_or_else(|error| {
                                    format!("\n    OpenCode export: unavailable ({error})")
                                })
                            },
                        )
                    }
                } else if elapsed >= WORKER_DEAD_SECS {
                    hard_dead_worker_transcript_block(
                        transcript_resolution_for_worker.as_ref(),
                        clone_path.as_deref(),
                        session_uuid,
                        worker_cli,
                    )
                } else {
                    String::new()
                };
                // Surface session UUID alongside the friendly name so the
                // supervisor can cross-reference task-ownership errors
                // ("owned by worker-backfill (0a7f2802-...)") without manual
                // table-lookup. cas-85bf.
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
                let account_info = agent.metadata.get("worker_account_dir").map_or_else(
                    || "\n    account: default/inherited".to_string(),
                    |account| format!("\n    account: {account}"),
                );
                // Context usage (cas-573c): cheap tail-read of the session
                // transcript to surface a coarse band so the supervisor can
                // proactively preserve work before compaction. Falls back
                // silently when the transcript isn't found yet (new workers).
                //
                // cas-d165: hoisted out of the `context_info` block so the
                // same resolved path can also feed the in-flight-tool-call
                // check below — one resolution, two consumers, instead of
                // globbing/reading the transcript twice per worker.
                let context_info = {
                    match transcript_path_for_worker
                        .as_deref()
                        .and_then(|path| read_context_usage_from_tail_for_cli(path, worker_cli))
                    {
                        Some(usage) => format_context_usage(usage),
                        None => String::new(),
                    }
                };
                let compaction_info = format_context_checkpoint_status(&agent.metadata);
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
                //
                // cas-e728 (GH #105): a lease only corroborates in-progress
                // work while the task it points at is STILL in progress.
                // Leases are never renewed and are not always released on
                // close, so an unqualified `!leases.is_empty()` kept a
                // finished worker looking busy for the rest of the lease
                // duration (default 30m) and drove `⚠ STALLED ... while task
                // in progress` at workers with nothing assigned.
                let has_in_progress_task = store
                    .list_agent_leases(&agent.id)
                    .map(|leases| {
                        leases
                            .iter()
                            .any(|lease| unfinished_task_ids.contains(&lease.task_id))
                    })
                    .unwrap_or(false)
                    || in_progress_assignees.contains(agent.name.as_str())
                    || in_progress_assignees.contains(agent.id.as_str());
                // cas-a653: hook-less harnesses (Codex) only emit Cassy events
                // on their own MCP calls — fold in the transcript's own mtime
                // so this doesn't freeze at the age of the last Cassy call
                // while the worker keeps working via exec_command/apply_patch.
                // Reuses the same transcript path already resolved above for
                // context_info/in_flight_tool_call, and the same
                // wedged::transcript_mtime_age primitive `is-wedged` trusts.
                let artifact_last_activity = last_worker_activity_secs_with_harness_turn(
                    &activity_events,
                    &agent.id,
                    worker_cli,
                    transcript_path_for_worker.as_deref(),
                    now,
                );
                let last_activity = if worker_cli == cas_mux::SupervisorCli::OpenCode {
                    opencode_observation
                        .as_ref()
                        .and_then(|observation| {
                            crate::mcp::tools::service::opencode_liveness::last_activity_secs(
                                observation,
                                now.timestamp_millis().max(0) as u64,
                            )
                        })
                        .or_else(|| last_worker_activity_secs(&activity_events, &agent.id))
                } else {
                    artifact_last_activity
                };
                let progress_timestamps = format_worker_progress_timestamps(
                    prompt_queue.as_ref().and_then(|queue| {
                        queue
                            .latest_created_at_for_targets_from_sources(
                                &[agent.name.as_str()],
                                &outbound_target_refs,
                                factory_session.as_deref(),
                            )
                            .ok()
                            .and_then(|timestamps| timestamps.into_values().max())
                    }),
                    latest_worker_file_write_at(&file_write_events, &agent.id),
                    now,
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
                let in_flight_tool_call = if worker_cli == cas_mux::SupervisorCli::OpenCode {
                    opencode_observation
                        .as_ref()
                        .is_some_and(crate::mcp::tools::service::opencode_liveness::active_tool)
                } else {
                    transcript_path_for_worker.as_deref().is_some_and(|p| {
                        crate::cli::factory::wedged::transcript_has_in_flight_tool_call(
                            p, worker_cli,
                        )
                    })
                };
                // cas-058e: a completed tool call may have left a real cargo
                // build running under the worker pane. Resolve the harness PID
                // exactly as `is-wedged` does; an unreadable process tree is
                // deliberately not positive evidence and therefore cannot
                // suppress a real stall.
                let worker_pid = crate::cli::factory::wedged::find_worker_pid(
                    &crate::cli::factory::wedged::RealProcessTable,
                    &agent.name,
                )
                .or(agent.pid);
                let worker_pid_alive = worker_pid.is_some_and(crate::mcp::daemon::pid_alive);
                let background_processes = worker_pid
                    .map(crate::cli::factory::wedged::background_processes_for)
                    .unwrap_or(crate::cli::factory::wedged::BackgroundProcessState::Unavailable);
                let approval_hang = crate::cli::factory::wedged::is_leader_approval_hang(
                    worker_pid_alive,
                    pending_permission.as_ref(),
                    &background_processes,
                ) && !in_flight_tool_call;
                let mut has_active_work = crate::cli::factory::wedged::has_active_work(
                    in_flight_tool_call,
                    &background_processes,
                );
                if let Some(observation) = opencode_observation.as_ref() {
                    has_active_work |=
                        crate::mcp::tools::service::opencode_liveness::has_active_work(observation);
                }
                let assigned_open_task = assigned_open_tasks.iter().find(|task| {
                    task.assignee.as_deref() == Some(agent.name.as_str())
                        || task.assignee.as_deref() == Some(agent.id.as_str())
                });
                let effective_stall_threshold = crate::ui::factory::effective_stall_threshold_secs(
                    stall_threshold_secs as u64,
                    worker_effort_from_agent(agent),
                ) as i64;
                let assigned_unstarted_elapsed = assigned_open_task.and_then(|task| {
                    assigned_unstarted_elapsed_secs(
                        task.updated_at,
                        last_activity.map(|(secs, _)| secs),
                        effective_stall_threshold,
                        in_flight_tool_call,
                        chrono::Utc::now(),
                    )
                });
                let stalled = !approval_hang
                    && is_worker_stalled(
                        has_in_progress_task,
                        last_activity.map(|(secs, _)| secs),
                        stall_threshold_secs,
                        has_active_work,
                    );
                // cas-e728 (GH #105): inbox depth is rendered on EVERY row, not
                // only on a row that already tripped the stall path. The most
                // common "handed work and did not wake" shape is a worker with
                // a parked or freshly assigned task — which is never "stalled"
                // — so gating this on the alert hid it exactly where it was
                // needed.
                let worker_inbox = inbox_state
                    .get(agent.name.as_str())
                    .copied()
                    .unwrap_or_default();
                let inbox_info = match worker_inbox.unread {
                    0 => String::new(),
                    count => {
                        let plural = if count == 1 { "" } else { "s" };
                        match worker_inbox.oldest_unread_secs {
                            Some(age) => format!(
                                "\n    inbox: {count} unread message{plural} (oldest {age}s)"
                            ),
                            None => format!("\n    inbox: {count} unread message{plural}"),
                        }
                    }
                };
                let rehome_info = factory_rehome_label(agent);
                let priority_alert = format_priority_worker_status_alert(
                    stalled,
                    last_activity,
                    stall_threshold_secs,
                    assigned_open_task
                        .zip(assigned_unstarted_elapsed)
                        .map(|(task, elapsed)| {
                            (task.id.as_str(), elapsed, effective_stall_threshold)
                        }),
                    turn_start_observable,
                    elapsed,
                    worker_inbox,
                );
                let approval_info = pending_permission.as_ref().map(|pending| {
                    if approval_hang {
                        format!(
                            "\n    awaiting leader approval: {} ({}s pending; command: {}) — no active child process",
                            pending.tool_name, pending.age_secs, pending.command_excerpt
                        )
                    } else {
                        format!(
                            "\n    leader approval pending: {} ({}s; command: {})",
                            pending.tool_name, pending.age_secs, pending.command_excerpt
                        )
                    }
                });
                let activity_info = if let Some(approval) = approval_info {
                    approval
                } else if let Some(alert) = priority_alert {
                    alert
                } else if !has_in_progress_task {
                    match last_activity {
                        Some((secs, phase)) => {
                            format!("\n    last activity: {secs}s ago ({phase})")
                        }
                        None => {
                            "\n    last activity: none in last 10m (may be investigating or idle)"
                                .to_string()
                        }
                    }
                } else if has_active_work {
                    // Has an assignment, would otherwise read as stalled
                    // (old/absent checkpoint), but an in-flight tool call
                    // is direct evidence of real work in progress — never
                    // render the ambiguous "may be investigating or idle"
                    // hedge for a worker holding an assignment (AC3).
                    match last_activity {
                        Some((secs, phase)) => format!(
                            "\n    last activity: {secs}s ago ({phase}) — live background work (busy, not stalled)"
                        ),
                        None => "\n    live background work in progress (busy, not stalled — no checkpoint-class activity yet)"
                            .to_string(),
                    }
                } else {
                    match last_activity {
                        Some((secs, phase)) => {
                            format!("\n    last activity: {secs}s ago ({phase})")
                        }
                        None => {
                            // Unreachable in practice: has_in_progress_task
                            // && !has_active_work && last_activity==None
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
                // GH #67: name the assignment on the roster row itself.
                let matches_agent = |assignee: Option<&str>| {
                    assignee == Some(agent.name.as_str()) || assignee == Some(agent.id.as_str())
                };
                let parked_task = parked_tasks
                    .iter()
                    .find(|t| matches_agent(t.assignee.as_deref()));
                let task_info = format_assigned_task_info(
                    in_progress_tasks
                        .iter()
                        .find(|t| matches_agent(t.assignee.as_deref()))
                        .map(|t| (t.id.as_str(), t.title.as_str())),
                    assigned_open_task.map(|t| {
                        (
                            t.id.as_str(),
                            t.title.as_str(),
                            t.labels
                                .iter()
                                .any(|label| label == VERIFICATION_REJECTED_REOPEN_LABEL),
                        )
                    }),
                    parked_task.map(|t| (t.id.as_str(), t.title.as_str(), t.status)),
                    parked_task.is_some_and(|task| {
                        awaiting_merge_delivery_is_already_integrated(
                            &self.inner.cas_root,
                            task,
                            clone_path.as_deref(),
                        )
                    }),
                );
                output.push_str(&format!(
                    "  • {} (heartbeat: {}){}{}{}{}{}{}{}{}{}{}{}{}{}{}{}\n    session: {}\n",
                    &agent.name,
                    since,
                    if usage_limited {
                        " [UNAVAILABLE: Codex usage limit]"
                    } else {
                        liveness_label
                    },
                    held_label,
                    clone_info,
                    git_info,
                    transcript_info,
                    model_info,
                    account_info,
                    context_info,
                    compaction_info,
                    activity_info,
                    progress_timestamps,
                    harness_turn_info,
                    task_info,
                    inbox_info,
                    rehome_info,
                    session_uuid
                ));
                if worker_cli == cas_mux::SupervisorCli::OpenCode {
                    let opencode_info =
                        crate::mcp::tools::service::opencode_liveness::render_status(
                            opencode_observation.as_ref(),
                        );
                    output.push_str(opencode_info.trim_start_matches('\n'));
                    output.push('\n');
                }
            }
        }

        // GH #60: spawn lifecycle — which request produced which worker, and
        // which never registered.
        output.push_str(&spawn_section);

        // cas-2e81: died-while-leased section (empty-fleet vs crash distinction).
        output.push_str(&died_section);

        // cas-e728: say it out loud rather than rendering a confidently empty
        // roster of task state.
        if task_read_failed {
            output.push_str(
                "\n⚠ TASK STATE UNAVAILABLE: the task store could not be read for this poll, so \
                 every 'task:' line and stall verdict above is incomplete. Re-run worker_status \
                 before acting on it.\n",
            );
        }
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
        use crate::store::{open_agent_store, open_task_store};
        use cas_store::{EventStore, SqliteEventStore};
        use cas_types::{AgentRole, AgentStatus, TaskStatus};

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
        let all_agents = agent_store.list(None).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list agents: {e}"),
            )
        })?;
        let mut dead_workers = Vec::new();
        let mut visible_workers = Vec::new();
        for agent in all_agents.into_iter().filter(|agent| {
            agent.role == AgentRole::Worker
                && matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
                && agent.visible_to_factory_session(factory_session.as_deref())
                && owned.as_ref().is_none_or(|set| set.contains(&agent.name))
        }) {
            if agent.pid.is_some() && !agent_process_is_alive(&agent) {
                dead_workers.push(agent);
            } else {
                visible_workers.push(agent);
            }
        }

        if !requested_names.is_empty() {
            visible_workers
                .retain(|a| {
                    requested_names
                        .iter()
                        .any(|target| worker_answers_to(a, target))
                });
            dead_workers
                .retain(|a| {
                    requested_names
                        .iter()
                        .any(|target| worker_answers_to(a, target))
                });
        }

        let visible_worker_ids: std::collections::HashSet<String> =
            visible_workers.iter().map(|a| a.id.clone()).collect();
        let visible_worker_names: std::collections::HashSet<String> =
            visible_workers.iter().map(|a| a.name.clone()).collect();
        let worker_names_by_id: std::collections::HashMap<String, String> = visible_workers
            .iter()
            .map(|agent| (agent.id.clone(), agent.name.clone()))
            .collect();

        // Use the same bounded event window as `worker_status`'s
        // `last_activity` signal. The activity feed is an operator-facing
        // corroborating view, so filtering it to a smaller, hook-only subset
        // would make an active worker disappear from this surface while the
        // status roster correctly reports fresh work.
        let activity_cutoff =
            chrono::Utc::now() - chrono::Duration::seconds(WORKER_ACTIVITY_WINDOW_SECS);
        let activity_events: Vec<cas_types::Event> = event_store
            .list_since(activity_cutoff, 200)
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to list events: {e}"),
                )
            })?
            .into_iter()
            .filter(is_worker_activity_event)
            .collect();

        // cas-9f6f: terminal task events are durable forensic data, not live
        // worker activity. Keep one compact count for supervisor awareness
        // instead of letting old closed-task rows crowd out active workers.
        let closed_task_ids: std::collections::HashSet<String> =
            open_task_store(&self.inner.cas_root)
                .ok()
                .and_then(|store| store.list(Some(TaskStatus::Closed)).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|task| task.id)
                .collect();
        let mut terminal_event_count = 0usize;
        let mut live_worker_events = Vec::new();
        for event in &activity_events {
            let belongs_to_visible_worker = event
                .session_id
                .as_ref()
                .is_some_and(|id| visible_worker_ids.contains(id))
                || visible_worker_ids.contains(&event.entity_id)
                || visible_worker_names.contains(&event.entity_id);
            if !belongs_to_visible_worker {
                continue;
            }
            if closed_task_ids.contains(&event.entity_id) {
                terminal_event_count += 1;
            } else {
                live_worker_events.push(event);
            }
        }
        let worker_events: Vec<_> = live_worker_events.into_iter().take(20).collect();

        // cas-a568: `worker_activity` is the supervisor's corroborating view
        // for worker_status's STALLED verdict, so it must consume the same
        // corrected signal. In particular, Codex tool calls update the rollout
        // but usually do not emit a Cassy event. Resolve the same concrete path
        // as worker_status (including cas-fa69's Resolved-only Codex rule) and
        // add a transcript-backed row only when it is fresher than that
        // worker's event-store signal. This augments the event feed rather
        // than introducing a second activity detector.
        let transcript_activity: Vec<_> = visible_workers
            .iter()
            .filter_map(|agent| {
                let cli = worker_cli_from_agent(agent);
                let clone_path = agent.metadata.get("clone_path").map(String::as_str);
                let session_id = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
                let transcript_path = worker_status_transcript_path_for_account(
                    clone_path,
                    session_id,
                    cli,
                    agent.metadata.get("worker_account_dir").map(String::as_str),
                )?;
                let event_activity = last_worker_activity_secs(&activity_events, &agent.id);
                let effective_activity = last_worker_activity_secs_with_transcript(
                    &activity_events,
                    &agent.id,
                    cli,
                    Some(&transcript_path),
                )?;
                if event_activity.is_some_and(|(event_age, _)| event_age <= effective_activity.0) {
                    return None;
                }
                let in_flight = crate::cli::factory::wedged::transcript_has_in_flight_tool_call(
                    &transcript_path,
                    cli,
                );
                Some((agent.name.clone(), effective_activity.0, in_flight))
            })
            .collect();

        // OpenCode has no transcript path. Read the plugin projection by CAS
        // session identity instead; event timestamps are the activity source,
        // and a live process is only rendered as an explicit fallback.
        let opencode_activity: Vec<_> = visible_workers
            .iter()
            .filter_map(|agent| {
                (worker_cli_from_agent(agent) == cas_mux::SupervisorCli::OpenCode).then_some(agent)
            })
            .filter_map(|agent| {
                let session_id = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
                let observation = crate::mcp::tools::service::opencode_liveness::observe(
                    &self.inner.cas_root,
                    session_id,
                    chrono::Utc::now().timestamp_millis().max(0) as u64,
                    agent_process_is_alive(agent),
                )?;
                let label = crate::mcp::tools::service::opencode_liveness::verdict_label(
                    observation.verdict,
                );
                let age = crate::mcp::tools::service::opencode_liveness::last_activity_secs(
                    &observation,
                    chrono::Utc::now().timestamp_millis().max(0) as u64,
                )
                .map(|(age, _)| age)
                .or_else(|| {
                    (observation.verdict == cas_mux::OpenCodeLivenessVerdict::ProcessAliveFallback)
                        .then_some(0)
                })?;
                (age <= WORKER_ACTIVITY_WINDOW_SECS).then_some((
                    agent.name.clone(),
                    age,
                    crate::mcp::tools::service::opencode_liveness::active_tool(&observation),
                    label,
                ))
            })
            .collect();

        // GH #255 round 2: hooks do not reliably observe Codex file edits, so
        // the event/transcript view above must never make a dirty worker look
        // idle. Snapshot each visible worktree at query time as a floor under
        // those asynchronous signals. This deliberately reuses worker_status's
        // resolver and collector rather than inventing a third git path.
        let worktree_activity: Vec<_> = visible_workers
            .iter()
            .filter_map(|agent| {
                collect_worker_activity_worktree_snapshot(&self.inner.cas_root, agent)
            })
            .collect();

        if worker_activity_has_no_rows(
            worker_events.len(),
            transcript_activity.len(),
            worktree_activity.len(),
            terminal_event_count,
            dead_workers.len(),
        ) && opencode_activity.is_empty()
        {
            return Ok(Self::success(
                "No recent worker activity.\n\nworker_activity uses the same 10-minute event window as worker_status, plus resolved worker transcript/rollout freshness and a query-time dirty-worktree floor. Transcript activity is unavailable when a worker's transcript cannot be resolved.",
            ));
        }

        let mut output = String::from("Worker Activity\n===============\n\n");
        if terminal_event_count > 0 {
            output.push_str(&format!(
                "⚠ {terminal_event_count} terminal-task activity row{} suppressed; closed work is not live worker activity.\n",
                if terminal_event_count == 1 { "" } else { "s" }
            ));
        }
        for agent in dead_workers {
            output.push_str(&format!(
                "• {} - harness process gone (dead; process liveness check)\n",
                agent.name
            ));
        }
        for event in worker_events {
            let ago = format_relative_time(event.created_at);
            let worker_name = event
                .session_id
                .as_ref()
                .and_then(|id| worker_names_by_id.get(id))
                .or_else(|| worker_names_by_id.get(&event.entity_id))
                .map(String::as_str)
                .or_else(|| {
                    visible_worker_names
                        .contains(&event.entity_id)
                        .then_some(event.entity_id.as_str())
                })
                .unwrap_or("unknown worker");
            output.push_str(&format!(
                "• {} - {} ({})\n",
                worker_name, event.summary, ago
            ));
        }
        for (worker_name, age_secs, in_flight) in transcript_activity {
            let activity = if in_flight {
                "in-flight tool call"
            } else {
                "transcript/tool activity"
            };
            output.push_str(&format!(
                "• {worker_name} - {activity} ({age_secs}s ago; transcript-backed)\n"
            ));
        }
        for (worker_name, age_secs, in_flight, verdict) in opencode_activity {
            let activity = if in_flight {
                "OpenCode mapped session: in-flight tool call"
            } else {
                "OpenCode mapped session activity"
            };
            output.push_str(&format!(
                "• {worker_name} - {activity} ({age_secs}s ago; {verdict}; no transcript path)\n"
            ));
        }
        for snapshot in &worktree_activity {
            output.push_str(&format_worker_activity_worktree_snapshot(snapshot));
        }

        Ok(Self::success(output))
    }

    /// cas-dffe (GH #145): reset a worker's context for real, and prove it —
    /// or fail loudly.
    ///
    /// The old implementation enqueued the four characters `/clear` as an
    /// ordinary message. A Claude worker under Agent Teams therefore received
    /// the *string* "/clear" in its inbox, acknowledged it as a teammate note,
    /// and carried on with its whole conversation loaded — while this tool
    /// reported `Queued /clear for <worker>`. Six such calls across four
    /// workers in one session all "succeeded" and none reset anything.
    ///
    /// What happens now, per target:
    ///
    /// 1. Resolve the recipient's harness. A harness with no verified in-place
    ///    reset command is refused here, before anything is queued
    ///    ([`crate::factory_context_reset::context_reset_command`]).
    /// 2. Snapshot the recipient's existing session transcripts.
    /// 3. Queue a control command — the
    ///    [`crate::factory_context_reset::CONTEXT_RESET_CONTROL`] sentinel, not
    ///    readable text — which the daemon hard-routes to the PTY and delivers
    ///    as the harness's own command.
    /// 4. Wait (bounded) for the post-condition: a NEW session transcript whose
    ///    head records the `/clear`. That is the "new conversation id" evidence
    ///    the supervisor could never get before, and it is measured, not
    ///    assumed.
    /// 5. On confirmation, record the new session id on the agent so
    ///    `worker_status` follows the live transcript (its context band resets
    ///    with it) instead of reading the dead pre-reset file forever.
    ///
    /// If step 4 does not land inside the window the call returns an **error**
    /// naming exactly what was and was not observed. A reset Cassy cannot prove
    /// is never reported as a success.
    pub(super) async fn factory_clear_context(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::factory_context_reset as reset;
        use crate::store::{open_agent_store, open_prompt_queue_store};
        use cas_types::{AgentRole, AgentStatus};

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

        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;
        let factory_session = current_factory_session();
        let owned = supervisor_owned_workers();
        let live_agents = agent_store.list(Some(AgentStatus::Active)).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list agents: {e}"),
            )
        })?;

        // Resolve the concrete recipients. `all_workers` fans out over this
        // supervisor's live workers; every other target names exactly one.
        let recipients: Vec<cas_types::Agent> = if target == "all_workers" {
            live_agents
                .into_iter()
                .filter(|agent| {
                    agent.role == AgentRole::Worker
                        && agent.visible_to_factory_session(factory_session.as_deref())
                        && owned.as_ref().is_none_or(|set| set.contains(&agent.name))
                })
                .collect()
        } else {
            let found = live_agents
                .into_iter()
                .find(|agent| agent.name == target)
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!(
                            "No live agent named '{target}' — cannot reset the context of an \
                             agent Cassy cannot see. Check `worker_status`."
                        ),
                    )
                })?;
            vec![found]
        };

        if recipients.is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "No live workers to reset.".to_string(),
            ));
        }

        let queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open message queue: {e}"),
            )
        })?;
        let source = self
            .inner
            .get_agent_id()
            .unwrap_or_else(|_| "unknown".to_string());
        // cas-d9a8: stamp the reset control row with the registry id of the
        // caller CAS actually resolved. `source` above falls back to the string
        // "unknown", which is not an identity, so the stamp is taken from a
        // real agent row or omitted entirely.
        let queue_origin = crate::store::open_agent_store(&self.inner.cas_root)
            .ok()
            .and_then(|store| store.get(&source).ok())
            .map(|agent| cas_store::QueueOrigin::RegisteredAgent {
                agent_id: agent.id,
            });

        // Pre-flight every recipient BEFORE queueing anything: an unsupported
        // harness or an unlocatable transcript directory means Cassy could never
        // confirm the reset, so it must not claim one.
        struct PendingReset {
            agent: cas_types::Agent,
            dirs: Vec<std::path::PathBuf>,
            before: std::collections::BTreeSet<std::path::PathBuf>,
        }
        let mut pending: Vec<PendingReset> = Vec::new();
        let mut refusals: Vec<String> = Vec::new();

        for agent in recipients {
            let cli = worker_cli_from_agent(&agent);
            if reset::context_reset_command(cli).is_none() {
                refusals.push(format!(
                    "{}: {}",
                    agent.name,
                    reset::unsupported_reason(cli)
                ));
                continue;
            }
            let Some(clone_path) = agent.metadata.get("clone_path").cloned() else {
                refusals.push(format!(
                    "{}: no clone_path recorded for this worker, so Cassy cannot locate its session \
                     transcripts and could not verify a reset. Refusing rather than reporting an \
                     unverifiable success.",
                    agent.name
                ));
                continue;
            };
            let dirs = reset::transcript_dirs_for(&clone_path);
            if dirs.is_empty() {
                refusals.push(format!(
                    "{}: no Claude project directory found for {clone_path} under {}. Cassy could \
                     not verify a reset, so it did not attempt one.",
                    agent.name,
                    reset::claude_config_roots()
                        .iter()
                        .map(|root| root.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                continue;
            }
            let before = reset::snapshot_transcripts(&dirs);
            pending.push(PendingReset {
                agent,
                dirs,
                before,
            });
        }

        if pending.is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!(
                    "No context reset was attempted:\n  • {}",
                    refusals.join("\n  • ")
                ),
            ));
        }

        for item in &pending {
            queue
                .enqueue_urgent_with_outcome(
                    &source,
                    &item.agent.name,
                    reset::CONTEXT_RESET_CONTROL,
                    factory_session.as_deref(),
                    Some(reset::CONTEXT_RESET_SUMMARY),
                    Some(cas_store::NotificationPriority::Critical),
                    // Control rows take the interrupt-and-inject lane: a worker
                    // mid-turn must have its turn broken before the reset
                    // command can be typed, and two resets in a row must never
                    // be content-deduped into one.
                    true,
                    queue_origin.as_ref(),
                )
                .map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to queue context reset for {}: {e}", item.agent.name),
                    )
                })?;
        }

        // Wait for the post-condition. Polling the filesystem is what makes the
        // result verifiable: the daemon can only prove it typed bytes into a
        // pane, and "bytes typed" is precisely the evidence that was never
        // enough (GH #145).
        let deadline = std::time::Instant::now() + reset::confirmation_timeout();
        let mut confirmed: Vec<(String, reset::ContextResetEvidence, String)> = Vec::new();
        let mut outstanding: Vec<PendingReset> = pending;
        while !outstanding.is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(reset::CONFIRMATION_POLL).await;
            outstanding.retain(|item| {
                match reset::detect_context_reset(&item.dirs, &item.before) {
                    Some(evidence) => {
                        let previous = item
                            .agent
                            .cc_session_id
                            .clone()
                            .unwrap_or_else(|| item.agent.id.clone());
                        // Point Cassy's transcript resolution at the live session
                        // so worker_status/worker_activity/is-wedged stop
                        // reading the pre-reset file (AC4).
                        let mut updated = item.agent.clone();
                        updated.cc_session_id = Some(evidence.session_id.clone());
                        if let Err(e) = agent_store.update(&updated) {
                            tracing::warn!(
                                agent = %item.agent.name,
                                error = %e,
                                "cas-dffe: context reset confirmed but recording the new session \
                                 id failed; worker_status will keep resolving the old transcript"
                            );
                        }
                        confirmed.push((item.agent.name.clone(), evidence, previous));
                        false
                    }
                    None => true,
                }
            });
        }

        let mut output = String::new();
        for (name, evidence, previous) in &confirmed {
            output.push_str(&format!(
                "✅ {name}: context reset CONFIRMED\n    session: {} → {} (new conversation)\n    \
                 evidence: {}\n    preserved: registration, worktree, model/effort settings (the \
                 worker process was not restarted)\n",
                short_session(previous),
                short_session(&evidence.session_id),
                evidence.transcript.display(),
            ));
        }
        for item in &outstanding {
            output.push_str(&format!(
                "❌ {}: reset UNCONFIRMED after {}s — no new session transcript recording a \
                 /clear appeared under {}. The command is still queued and may yet land; verify \
                 with worker_status, and if the worker is wedged use shutdown_workers + \
                 spawn_workers (same name/worktree).\n",
                item.agent.name,
                reset::confirmation_timeout().as_secs(),
                item.dirs
                    .iter()
                    .map(|dir| dir.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        for refusal in &refusals {
            output.push_str(&format!("❌ {refusal}\n"));
        }

        if !outstanding.is_empty() || !refusals.is_empty() {
            // Never report success for a reset that was not observed.
            return Err(Self::error(ErrorCode::INTERNAL_ERROR, output));
        }

        Ok(Self::success(output))
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

        // Worktree binding (cas-30c6).
        //
        // The branch must be resolved AT the directory this session is bound to
        // — previously it came from wherever the MCP server process happened to
        // be running, so my_context and the PreToolUse commit guard could report
        // different repositories. Both now classify the same canonical binding
        // through `factory_isolation`, and a worker bound to a sibling's
        // worktree or to the shared checkout is told so explicitly instead of
        // being shown a branch line that looks fine.
        let bound_path = std::env::var("CAS_CLONE_PATH")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::current_dir().ok());
        let branch = bound_path
            .as_deref()
            .and_then(crate::factory_isolation::branch_at);
        output.push_str(&crate::factory_isolation::render_worker_binding(
            &agent.name,
            agent.role == AgentRole::Worker,
            bound_path
                .as_deref()
                .map(|path| path.display().to_string())
                .as_deref(),
            branch.as_deref(),
        ));

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

        Ok(Self::success(output))
    }

    /// Push a sync incident to the worker whose directory it is AND to the
    /// supervisor (cas-0a6f / GH #103).
    ///
    /// A stranded stash or an unfinished rebase is invisible from inside the
    /// worker's next turn — it just sees a working tree that lost its changes.
    /// Best-effort by design: a queue failure must not mask the sync report,
    /// so it is reported back as a line instead of an error. Returns the
    /// delivery outcomes for the report.
    fn notify_sync_incident(
        &self,
        worker_name: &str,
        path: &std::path::Path,
        sync_ref: &str,
        failure: &SyncFailure,
    ) -> Vec<String> {
        use crate::store::{NotificationPriority, open_prompt_queue_store};

        let mut body = format!(
            "SYNC INCIDENT in your worktree ({}) while syncing to '{sync_ref}': {}",
            path.display(),
            failure.message
        );
        if let Some(stash) = failure.stranded_stash.as_deref() {
            body.push_str(&format!(
                "\n\nYour uncommitted work was stashed and could NOT be restored automatically. \
                 It is not lost. In that worktree run:\n\n    {}\n\nDo this before making \
                 further edits, or the apply will conflict. Once it is applied cleanly, find the \
                 entry in `git stash list` and drop it by its stash@{{N}} index (`drop` and `pop` \
                 do not accept the SHA above).",
                stash_recovery_command(stash)
            ));
        }
        if failure.mid_rebase {
            body.push_str(
                "\n\nThe worktree was left MID-REBASE. Resolve the conflict and \
                 `git rebase --continue`, or `git rebase --abort` to return to the prior state, \
                 before doing anything else in it.",
            );
        }

        let summary = if failure.stranded_stash.is_some() {
            format!("sync stranded {worker_name} WIP in a stash")
        } else {
            format!("sync left {worker_name} mid-rebase")
        };

        let queue = match open_prompt_queue_store(&self.inner.cas_root) {
            Ok(queue) => queue,
            Err(error) => {
                tracing::error!(
                    target: "cas::coordination",
                    stage = "sync_incident_queue_open_failed",
                    worker = %worker_name,
                    "{error}"
                );
                return vec![format!(
                    "{worker_name}: COULD NOT NOTIFY (queue unavailable: {error}) — relay the \
                     recovery instruction above by hand"
                )];
            }
        };

        let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
        let supervisor = std::env::var("CAS_SUPERVISOR_NAME")
            .ok()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                use cas_types::{AgentRole, AgentStatus};
                crate::store::open_agent_store(&self.inner.cas_root)
                    .ok()
                    .and_then(|store| store.list(None).ok())
                    .and_then(|agents| {
                        agents
                            .into_iter()
                            .find(|a| {
                                a.role == AgentRole::Supervisor
                                    && matches!(a.status, AgentStatus::Active | AgentStatus::Idle)
                            })
                            .map(|a| a.name)
                    })
            });

        let mut outcomes = Vec::new();
        let mut targets = vec![worker_name.to_string()];
        match supervisor {
            Some(name) if name != worker_name => targets.push(name),
            Some(_) => {}
            None => outcomes.push(format!(
                "{worker_name}: supervisor identity unresolved — incident delivered to the worker \
                 only"
            )),
        }

        for target in targets {
            match queue.enqueue_full(
                "cas-sync",
                &target,
                &body,
                factory_session.as_deref(),
                Some(summary.as_str()),
                Some(NotificationPriority::High),
            ) {
                Ok(id) => outcomes.push(format!("{target}: notified (message {id})")),
                Err(error) => {
                    tracing::error!(
                        target: "cas::coordination",
                        stage = "sync_incident_enqueue_failed",
                        worker = %worker_name,
                        notify_target = %target,
                        "{error}"
                    );
                    outcomes.push(format!(
                        "{target}: COULD NOT NOTIFY ({error}) — relay the recovery instruction by \
                         hand"
                    ));
                }
            }
        }
        outcomes
    }

    pub(super) async fn factory_sync_all_workers(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::store::open_agent_store;
        use cas_types::{AgentRole, AgentStatus};

        // cas-bfa5: resolve and validate the complete target before inspecting
        // or mutating any worker clone. In particular, an invalid explicit id
        // must not fall through to a focused/ready epic and a stale session
        // focus must not select an epic from another project.
        let sync_ref = resolve_sync_all_workers_target(&self.inner.cas_root, &req)
            .map_err(|e| Self::error(ErrorCode::INVALID_PARAMS, e))?;

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

        let mut synced = Vec::new();
        let mut skipped = Vec::new();
        let mut failed = Vec::new();
        let mut notified = Vec::new();

        // cas-0a6f (GH #103): sync is a bulk, supervisor-initiated rewrite of
        // other agents' working directories. Live WIP and in-flight tasks are
        // never collateral without explicit consent.
        let force = req.force.unwrap_or(false);
        let bindings = worker_task_bindings(&self.inner.cas_root);

        // cas-5884: a fleet sweep must not rebase a worker onto a branch its
        // task does not integrate into. `worker_names=` targeting is the
        // documented override; `force=` deliberately is not.
        let explicitly_targeted = req
            .worker_names
            .as_deref()
            .map(str::trim)
            .is_some_and(|names| !names.is_empty());
        let default_branch = crate::worktree::GitOperations::detect_repo_root(&self.inner.cas_root)
            .ok()
            .map(crate::worktree::GitOperations::new)
            .map(|git| git.detect_default_branch())
            .unwrap_or_else(|| "main".to_string());

        for worker in workers {
            let binding = bindings.get(&worker.name);

            // cas-5884: branch affinity first — it is decided from task state
            // alone and is the most decisive reason not to touch a worktree.
            if let SyncGate::Refuse(reason) = sync_affinity_gate(
                &worker.name,
                binding.map(|b| b.label.as_str()),
                binding.map_or(&BranchAffinity::Unknown, |b| &b.affinity),
                &sync_ref,
                &default_branch,
                explicitly_targeted,
            ) {
                skipped.push(reason);
                continue;
            }

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

            let dirty_files = match dirty_file_count(&path) {
                Ok(count) => count,
                Err(err) => {
                    // Cannot establish cleanliness ⇒ cannot claim consent.
                    skipped.push(format!(
                        "{} (refusing sync — could not read worktree status: {err})",
                        worker.name
                    ));
                    continue;
                }
            };
            let in_progress = binding
                .filter(|b| b.blocks_rebase)
                .map(|b| b.label.as_str());
            if let SyncGate::Refuse(reason) = sync_gate_for_worker(
                &worker.name,
                dirty_files,
                rebase_in_progress(&path),
                in_progress,
                force,
            ) {
                skipped.push(reason);
                continue;
            }

            match sync_worker_clone(&path, &sync_ref) {
                Ok(details) => synced.push(format!("{} ({})", worker.name, details)),
                Err(failure) => {
                    // A stranded stash or an unfinished rebase is invisible to
                    // the worker whose directory it is — push it to them and to
                    // the supervisor rather than only into this report.
                    if failure.stranded_stash.is_some() || failure.mid_rebase {
                        notified.extend(self.notify_sync_incident(
                            &worker.name,
                            &path,
                            &sync_ref,
                            &failure,
                        ));
                    }
                    failed.push(format!("{} ({})", worker.name, failure.report_line()));
                }
            }
        }

        let mut out = format!(
            "Worker Sync Report\n==================\n\nSync target: {sync_ref}\nTrunk: \
             {default_branch}\nMode: {}\nBranch affinity: {}\n",
            if force {
                "force=true (dirty worktrees stashed; mid-task workers rebased)"
            } else {
                "safe (dirty or mid-task worktrees are skipped — pass force=true to include them)"
            },
            if explicitly_targeted {
                "bypassed (workers named explicitly in worker_names=)"
            } else {
                "enforced (workers whose task integrates into a different branch are skipped; \
                 name them in worker_names= to override — force= does not)"
            }
        );
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
        if !notified.is_empty() {
            out.push_str("\nIncident notifications:\n");
            for item in notified {
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
        let live_workers = live_factory_workers(agent_store.as_ref());
        let (
            orphan_process_groups,
            live_owned_process_groups,
            stale_process_group_records,
            unverifiable_process_groups,
        ) = orphan_process_groups(&self.inner.cas_root, stale_after, &live_workers);

        let prompt_queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open prompt queue: {e}"),
            )
        })?;
        let pending_prompts = prompt_queue.pending_count().unwrap_or(0);
        let live_viktor_watches = cas_store::SqliteViktorWatchStore::open(&self.inner.cas_root)
            .and_then(|store| store.list_live())
            .unwrap_or_default();

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

        let config = crate::config::Config::load(&self.inner.cas_root).unwrap_or_default();
        let trace_archive =
            cas_store::trace_archive_stats(&self.inner.cas_root).unwrap_or_default();
        let closed_task_ids = crate::store::open_task_store(&self.inner.cas_root)
            .ok()
            .and_then(|store| store.list(Some(cas_types::TaskStatus::Closed)).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|task| task.id)
            .collect();
        let artifact_report = factory_artifact_inventory(
            crate::config::resolved_factory_artifacts_root(
                config.factory().artifacts_root.as_deref(),
            ),
            &closed_task_ids,
        );
        let target_cache_report = {
            let policy = crate::factory_target_cache::TargetCachePolicy::from(config.factory());
            let live_roots = live_target_cache_worktrees(agent_store.as_ref());
            let known_roots = known_target_cache_worktrees(
                &self.inner.cas_root,
                &config,
                agent_store.as_ref(),
                worktree_store.as_ref(),
            );
            crate::factory_target_cache::inspect(
                &self.inner.cas_root,
                policy,
                &known_roots,
                &live_roots,
                true,
            )
        };
        let host_registry_pids = crate::store::known_repos::host_registry_open_pids();
        // cas-b7dd (GH #88): processes still alive inside a worktree with no
        // live owner, plus registered servers whose session is gone.
        let orphan_processes = scan_orphan_processes(&self.inner.cas_root, &live_workers);
        // GH #529: a pnpm virtual-store link can survive after its worktree
        // does not. Surface that false-green condition in the regular factory
        // report instead of relying on a later JS test runner to notice it.
        let primary_checkout = self.inner.cas_root.parent().unwrap_or(&self.inner.cas_root);
        let dangling_node_modules =
            crate::worktree::scan_dangling_node_modules_symlinks(primary_checkout);

        let mut out = String::from("Factory GC Report\n=================\n");
        out.push_str(&format!(
            "\nStale agent threshold: {}s\nStale agents: {}\nPending prompts: {}\nLive Viktor watches: {}\nActive worktrees: {}\nOrphan worktrees: {}\nOrphan worker process groups: {}\nLive-owned process groups skipped: {}\nUnverifiable process-group records preserved: {}\nStale process-group records: {}\nHost-registry open processes: {}\nOrphan processes in worktrees: {}\nStale server registrations: {}\nReapable orphans: {}\n",
            stale_after,
            stale_agents.len(),
            pending_prompts,
            live_viktor_watches.len(),
            active_worktrees.len(),
            orphan_worktrees.len(),
            orphan_process_groups.len(),
            live_owned_process_groups.len(),
            unverifiable_process_groups.len(),
            stale_process_group_records,
            host_registry_pids.len(),
            orphan_processes.processes.len(),
            orphan_processes.servers.len(),
            orphan_processes.reapable_count(),
        ));
        out.push_str(&format!(
            "\nTrace archive: {} files, {} bytes\n",
            trace_archive.files, trace_archive.bytes
        ));
        if !dangling_node_modules.is_empty() {
            out.push_str(
                "\nDangling primary-checkout node_modules symlinks (JS install is broken):\n",
            );
            for link in &dangling_node_modules {
                out.push_str(&format!(
                    "  - {} -> {}\n",
                    link.link.display(),
                    link.target.display(),
                ));
            }
            out.push_str(
                "  Remediation: run the repository's locked package-manager install from the primary checkout before relying on JS/TS tests.\n",
            );
        }
        out.push_str(&orphan_processes.render());
        out.push_str(&artifact_report.render());
        // GH #704: leaked disposable roots under $TMPDIR filled a 32 GB tmpfs
        // and broke every live session's shell output. Name them here, with
        // age and size, before the filesystem fills. Read-only by contract.
        out.push_str(
            &crate::temp_hygiene::scan_stale_temp_roots(
                &std::env::temp_dir(),
                std::time::Duration::from_secs(
                    req.older_than_secs
                        .and_then(|secs| u64::try_from(secs).ok())
                        .unwrap_or(crate::temp_hygiene::DEFAULT_TEMP_ROOT_STALE_SECS),
                ),
                std::time::SystemTime::now(),
            )
            .render(),
        );
        // Same incident, other half: a Cassy root that itself sits on RAM.
        // Reported, never enforced here — gc_report is read-only.
        if let Some(message) = crate::temp_hygiene::inspect_isolated_root(
            &self.inner.cas_root,
            crate::temp_hygiene::root_holds_bulk_dirs(&self.inner.cas_root),
            &crate::temp_hygiene::HostMountProbe,
        )
        .message()
        {
            out.push_str(&format!("Active Cassy root placement: {message}\n"));
        }

        if !live_viktor_watches.is_empty() {
            out.push_str("\nLive Viktor watches:\n");
            for watch in &live_viktor_watches {
                out.push_str(&format!(
                    "  - #{} thread={} run={} requester={} task={} polls={} expires={}\n",
                    watch.id,
                    watch.thread_id,
                    watch.run_id,
                    watch.requesting_agent_name,
                    watch.task_id.as_deref().unwrap_or("-"),
                    watch.poll_count,
                    watch.expires_at.to_rfc3339(),
                ));
            }
        }

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
        if !orphan_process_groups.is_empty() {
            out.push_str("\nOrphan worker process groups:\n");
            for record in &orphan_process_groups {
                out.push_str(&format!(
                    "  - {} (session {}, PGID {}, age {}s)\n",
                    record.worker_name,
                    record.factory_session,
                    record.pgid,
                    crate::ui::factory::process_groups::age(record).as_secs(),
                ));
            }
            out.push_str(
                "\nReclaim with gc_cleanup force=true after reviewing these fingerprinted groups.\n",
            );
        }
        if !live_owned_process_groups.is_empty() {
            out.push_str("\nLive-owned process groups skipped:\n");
            for record in &live_owned_process_groups {
                out.push_str(&format!(
                    "  - {} (session {}, PGID {}; registered owner is supervision-live)\n",
                    record.worker_name, record.factory_session, record.pgid,
                ));
            }
        }
        if !unverifiable_process_groups.is_empty() {
            out.push_str("\nUnverifiable process-group records preserved:\n");
            for record in &unverifiable_process_groups {
                out.push_str(&format!(
                    "  - {} (session {}, PGID {}; cleanup refused without a validated start-time fingerprint)\n",
                    record.worker_name, record.factory_session, record.pgid,
                ));
            }
        }
        if !host_registry_pids.is_empty() {
            let db = crate::store::known_repos::host_cas_dir().join("cas.db");
            out.push_str("\nProcesses with the host registry DB/WAL/SHM open (review for orphaned Cassy children):\n");
            for pid in &host_registry_pids {
                let command = process_command_line(*pid);
                out.push_str(&format!("  - PID {pid}: {command}\n"));
            }
            out.push_str(&format!(
                "Inspect exact ownership with `fuser -v {db}` and `ps -o pid,ppid,stat,etime,wchan:20,cmd -p <PID>` before terminating an orphan.\n",
                db = db.display()
            ));
        }

        match target_cache_report {
            Ok(report) => {
                out.push_str(&format!(
                    "\nCargo target cache pressure:\n  filesystem used: {}% (high {}%, low {}%)\n  pressure: {}\n  exact cache bytes: {}\n  selected reclaimable bytes: {}\n",
                    report.filesystem.used_percent,
                    report.filesystem.high_watermark_percent,
                    report.filesystem.low_watermark_percent,
                    report.filesystem.pressure,
                    report.candidate_bytes,
                    report.selected_bytes,
                ));
                for cache in &report.caches {
                    out.push_str(&format!(
                        "  - {} bytes={} state={:?} reason={}\n",
                        cache.path.display(),
                        cache.bytes,
                        cache.disposition,
                        cache.reason,
                    ));
                }
                out.push_str(&format!(
                    "TARGET_CACHE_STATUS_JSON={}\n",
                    report.machine_json()
                ));
            }
            Err(error) => out.push_str(&format!(
                "\nCargo target cache pressure: unavailable (fail-closed; no cache cleanup): {error}\n"
            )),
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
            collect_epic_branch_statuses, render_epic_status_report_with_stack,
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

        // Use the exact explicit repository/branch authority the epic close
        // path uses. An epic branch can be a useful coordination branch while
        // the project intentionally lands every child directly on the task's
        // declared target (typically `main`). Reporting against the former
        // while close evaluates the latter creates a false hard-block and
        // trains operators to fast-forward cosmetic epic branches merely to
        // silence this diagnostic (cas-50fe).
        let declared_repo_context = epic
            .deliverables
            .work_target
            .as_ref()
            .map(|target| {
                crate::mcp::tools::core::task::repo_context::resolve_repo_context(
                    &self.inner.cas_root,
                    target,
                )
            })
            .transpose()
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;
        let close_project_root = declared_repo_context
            .as_ref()
            .map(|context| context.repo_root.clone())
            .unwrap_or_else(|| {
                self.inner
                    .cas_root
                    .parent()
                    .unwrap_or(&self.inner.cas_root)
                    .to_path_buf()
            });
        let parent_branch = declared_repo_context
            .as_ref()
            .map(|context| context.target_branch.as_str())
            .or(epic.branch.as_deref())
            .unwrap_or("master");

        let subtasks = task_store.get_subtasks(epic_id).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to walk subtasks of {epic_id}: {e}"),
            )
        })?;

        let mut statuses =
            collect_epic_branch_statuses(&subtasks, parent_branch, &close_project_root);

        // cas-aae6 (GH #110): an epic stacked on other unlanded epic branches
        // cannot land alone. Show that here, where the supervisor decides
        // merge order, rather than only in the creation message that scrolled
        // away hours ago.
        let stacked_on = {
            use crate::worktree::GitOperations;
            // "Landed" is only meaningful against the trunk this epic is
            // actually destined for. Prefer the target branch the epic itself
            // declared (which is what epic creation branched from and what the
            // close gate merges into); only fall back to config/repo defaults
            // when the epic declared nothing. Re-deriving trunk from current
            // config would misclassify an ancestor as landed on a repo
            // configured with a different base (e.g. staging vs main).
            let trunk = epic
                .deliverables
                .work_target
                .as_ref()
                .and_then(|target| {
                    crate::mcp::tools::core::task::repo_context::resolve_repo_context(
                        &self.inner.cas_root,
                        target,
                    )
                    .ok()
                })
                .map(|context| context.target_branch)
                .or_else(|| crate::config::Config::configured_epic_base_branch(&close_project_root))
                .unwrap_or_else(|| {
                    GitOperations::new(close_project_root.to_path_buf()).detect_default_branch()
                });
            GitOperations::new(close_project_root.clone())
                .unlanded_epic_ancestry(parent_branch, &trunk)
        };
        if let Ok(agent_store) = crate::store::open_agent_store(&self.inner.cas_root) {
            if let Ok(agents) = agent_store.list(None) {
                for status in &mut statuses {
                    status.dead_or_stale_assignee =
                        status.assignee.as_ref().is_some_and(|assignee| {
                            agents.iter().any(|agent| {
                                (agent.name == *assignee || agent.id == *assignee)
                                    && matches!(
                                        agent.status,
                                        cas_types::AgentStatus::Stale
                                            | cas_types::AgentStatus::Shutdown
                                    )
                            })
                        });
                }
            }
        }
        let report =
            render_epic_status_report_with_stack(epic_id, parent_branch, &statuses, &stacked_on);

        Ok(Self::success(report))
    }

    pub(super) async fn factory_focus_epic(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        use crate::mcp::tools::types::validate_delivery_mode;
        use crate::store::open_task_store;
        use crate::ui::factory::{
            metadata_path, persist_session_metadata_delivery_mode_at,
            persist_session_metadata_pinned_epic_id_at,
        };
        use cas_types::{TaskStatus, TaskType};

        let factory_session = current_factory_session().ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_REQUEST,
                "focus_epic requires an active factory session (CAS_FACTORY_SESSION is not set)",
            )
        })?;
        let requested_delivery_mode = validate_delivery_mode(req.delivery_mode.as_deref())
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;

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
            self.record_focus_epic_event(&factory_session, None, None);
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

        let mut epic = task_store.get(epic_id).map_err(|e| {
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

        let delivery_mode = requested_delivery_mode.unwrap_or(epic.delivery_mode);
        if requested_delivery_mode.is_some() {
            epic.delivery_mode = delivery_mode;
            task_store.update(&epic).map_err(|error| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to persist epic delivery mode: {error}"),
                )
            })?;
        }

        persist_session_metadata_pinned_epic_id_at(&metadata_path, Some(epic_id)).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to persist pinned epic focus: {e}"),
            )
        })?;
        persist_session_metadata_delivery_mode_at(&metadata_path, delivery_mode).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to persist factory delivery mode: {e}"),
            )
        })?;
        self.record_focus_epic_event(&factory_session, Some(epic_id), Some(delivery_mode));

        Ok(Self::success(format!(
            "Pinned epic focus to {epic_id} for factory session {factory_session} (delivery_mode={delivery_mode})"
        )))
    }

    fn record_focus_epic_event(
        &self,
        factory_session: &str,
        epic_id: Option<&str>,
        delivery_mode: Option<cas_types::DeliveryMode>,
    ) {
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
            "delivery_mode": delivery_mode.map(|mode| mode.to_string()),
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

        let prompt_expiry_age = req.older_than_secs;
        let stale_after = prompt_expiry_age.unwrap_or(120);
        let agent_store = open_agent_store(&self.inner.cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open agent store: {e}"),
            )
        })?;
        let live_workers = live_factory_workers(agent_store.as_ref());
        let live_target_cache_roots = live_target_cache_worktrees(agent_store.as_ref());
        let (orphan_process_groups, live_owned_process_groups, _, unverifiable_process_groups) =
            orphan_process_groups(&self.inner.cas_root, stale_after, &live_workers);
        let mut orphan_process_groups_reaped = 0usize;
        let mut live_owned_process_groups_skipped = live_owned_process_groups.len();
        let mut stale_process_group_records_removed = 0usize;
        let mut process_group_errors = Vec::new();

        // Dead/recycled records are safe to discard. Live groups are reaped
        // only through gc_cleanup's existing explicit force gate.
        for record in
            crate::ui::factory::process_groups::list(&self.inner.cas_root).unwrap_or_default()
        {
            if matches!(
                crate::ui::factory::process_groups::status(&record),
                crate::ui::factory::process_groups::ProcessGroupStatus::Gone
                    | crate::ui::factory::process_groups::ProcessGroupStatus::FingerprintMismatch
            ) && crate::ui::factory::process_groups::untrack(&self.inner.cas_root, record.pgid)
                .is_ok()
            {
                stale_process_group_records_removed += 1;
            }
        }
        if req.force.unwrap_or(false) {
            for record in &orphan_process_groups {
                // Re-read canonical liveness immediately before the destructive
                // action. A worker may have registered or recovered after the
                // report/classification snapshot.
                if process_group_has_live_owner(record, &live_factory_workers(agent_store.as_ref()))
                {
                    live_owned_process_groups_skipped += 1;
                    continue;
                }
                match crate::ui::factory::process_groups::reap(&self.inner.cas_root, record).await {
                    Ok(crate::ui::factory::process_groups::ReapOutcome::Reaped) => {
                        orphan_process_groups_reaped += 1;
                    }
                    Ok(_) => stale_process_group_records_removed += 1,
                    Err(error) => process_group_errors.push(format!(
                        "{} (PGID {}): {error}",
                        record.worker_name, record.pgid
                    )),
                }
            }
        }
        let stale_agents = agent_store.list_stale(stale_after).unwrap_or_default();
        let mut stale_marked = 0usize;
        for agent in stale_agents {
            // Don't let workers prune supervisors/directors
            if agent.role == AgentRole::Supervisor || agent.role == AgentRole::Director {
                continue;
            }
            if crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness(&agent)
                .is_live()
            {
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
                if crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness(&agent)
                    .is_live()
                {
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
        let mut expired_prompts = 0usize;
        if req.force.unwrap_or(false) {
            let prompt_queue = open_prompt_queue_store(&self.inner.cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open prompt queue: {e}"),
                )
            })?;
            if let Some(older_than_secs) = prompt_expiry_age {
                // Targeted poison-queue remediation: preserve forensic rows
                // and terminally abandon only pending entries older than the
                // explicit cutoff. Omitting the cutoff retains the legacy
                // force-clear behavior.
                expired_prompts = prompt_queue
                    .abandon_pending_older_than(older_than_secs)
                    .unwrap_or(0);
            } else {
                cleared_prompts = prompt_queue.clear().unwrap_or(0);
            }
        }

        const SKILL_MARKER_MAX_AGE: std::time::Duration =
            std::time::Duration::from_secs(30 * 24 * 60 * 60);
        let stale_skill_markers_removed =
            cleanup_stale_skill_markers(&self.inner.cas_root, SKILL_MARKER_MAX_AGE);

        // Target-cache reclamation has a stricter destructive gate than the
        // legacy GC actions: both force=true AND dry_run=false must be
        // explicit. Existing `gc_cleanup force=true` calls remain preview-only
        // for Cargo artifacts instead of unexpectedly deleting warm caches.
        let target_cache_mutation_authorized =
            req.force.unwrap_or(false) && req.dry_run == Some(false);

        // cas-b7dd (GH #88): orphan processes use that same double gate, and
        // for a stronger reason — this path sends SIGKILL to processes Cassy did
        // not start. A killed dev server cannot be un-killed, so `force=true`
        // alone stays a preview here exactly as it does for warm caches.
        // The scan is re-run now rather than reusing the report's snapshot, and
        // each kill revalidates its own fingerprint again immediately before
        // signalling (see `orphan_gc::cleanup`).
        let orphan_processes = scan_orphan_processes(&self.inner.cas_root, &live_workers);
        let orphan_process_summary = crate::ui::factory::orphan_gc::cleanup(
            &self.inner.cas_root,
            &orphan_processes,
            target_cache_mutation_authorized,
        );
        let config = crate::config::Config::load(&self.inner.cas_root).unwrap_or_default();
        let closed_task_ids = crate::store::open_task_store(&self.inner.cas_root)
            .ok()
            .and_then(|store| store.list(Some(cas_types::TaskStatus::Closed)).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|task| task.id)
            .collect();
        let artifact_root = crate::config::resolved_factory_artifacts_root(
            config.factory().artifacts_root.as_deref(),
        );
        // Durable receipts are deleted only after their task is closed, and
        // only through the same explicit destructive gate as cache reclamation.
        // Unknown directories are inventory-only: an operator must review them.
        let artifact_cleanup = factory_artifact_cleanup(
            &artifact_root,
            &closed_task_ids,
            target_cache_mutation_authorized,
        );
        let target_cache_result = {
            let policy = crate::factory_target_cache::TargetCachePolicy::from(config.factory());
            let known_roots = known_target_cache_worktrees(
                &self.inner.cas_root,
                &config,
                agent_store.as_ref(),
                worktree_store.as_ref(),
            );
            crate::factory_target_cache::inspect(
                &self.inner.cas_root,
                policy,
                &known_roots,
                &live_target_cache_roots,
                !target_cache_mutation_authorized,
            )
            .and_then(|mut report| {
                if target_cache_mutation_authorized {
                    crate::factory_target_cache::cleanup_selected(
                        &self.inner.cas_root,
                        &mut report,
                        policy,
                        &live_target_cache_roots,
                    )?;
                }
                Ok(report)
            })
        };

        let mut output = format!(
            "Factory GC cleanup complete.\n\nStale agents marked: {stale_marked}\nDead agent records purged: {dead_agent_records_purged}\nOrphan worktrees marked removed: {orphan_marked_removed}\nOrphan worker process groups reaped: {orphan_process_groups_reaped}\nLive-owned process groups skipped: {live_owned_process_groups_skipped}\nUnverifiable process-group records preserved: {}\nStale process-group records removed: {stale_process_group_records_removed}\nPrompt queue entries expired: {expired_prompts}\nPrompt queue entries cleared: {cleared_prompts}\nStale skill markers removed: {stale_skill_markers_removed}\nOrphan processes killed: {}\nStale server registrations cleared: {}\nOrphan candidates spared or refused: {}",
            unverifiable_process_groups.len(),
            orphan_process_summary.killed.len(),
            orphan_process_summary.records_cleared.len(),
            orphan_process_summary.skipped,
        );
        output.push_str(&artifact_cleanup.render());
        if !orphan_process_summary.killed.is_empty() {
            output.push_str(&format!(
                "\nKilled pids: {}",
                orphan_process_summary
                    .killed
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for error in &orphan_process_summary.errors {
            output.push_str(&format!("\nOrphan cleanup error: {error}"));
        }
        if !target_cache_mutation_authorized && orphan_process_summary.would_kill > 0 {
            output.push_str(&format!(
                "\nOrphan processes previewed, not killed: {} (rerun with force=true dry_run=false)",
                orphan_process_summary.would_kill
            ));
        }
        // Always show WHY a candidate was left alone — a silently filtered
        // orphan is indistinguishable from one Cassy never saw, and that is the
        // difference between "the port is free" and a wasted morning.
        if !orphan_processes.is_empty() {
            output.push_str(&orphan_processes.render());
        }
        if !req.force.unwrap_or(false) && !orphan_process_groups.is_empty() {
            output.push_str(&format!(
                "\nLive orphan process groups preserved: {} (rerun with force=true to reap)",
                orphan_process_groups.len()
            ));
        }
        if !process_group_errors.is_empty() {
            output.push_str("\n\nProcess-group cleanup errors:");
            for error in process_group_errors {
                output.push_str(&format!("\n  - {error}"));
            }
        }
        match target_cache_result {
            Ok(report) => {
                output.push_str(&format!(
                    "\n\nCargo target caches: mode={} candidates={} bytes={} selected_bytes={} reclaimed_bytes={}",
                    if report.dry_run { "dry-run" } else { "cleanup" },
                    report.caches.len(),
                    report.candidate_bytes,
                    report.selected_bytes,
                    report.reclaimed_bytes,
                ));
                for cache in &report.caches {
                    output.push_str(&format!(
                        "\n  - {} bytes={} state={:?} reason={}",
                        cache.path.display(),
                        cache.bytes,
                        cache.disposition,
                        cache.reason,
                    ));
                }
                output.push_str(&format!(
                    "\nTARGET_CACHE_STATUS_JSON={}",
                    report.machine_json()
                ));
            }
            Err(error) => output.push_str(&format!(
                "\n\nCargo target caches: unavailable (fail-closed; none deleted): {error}"
            )),
        }

        Ok(Self::success(output))
    }
}

fn process_command_line(pid: u32) -> String {
    let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return "<command unavailable>".to_string();
    };
    let command = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part))
        .collect::<Vec<_>>()
        .join(" ");
    if command.is_empty() {
        "<command unavailable>".to_string()
    } else {
        command
    }
}

/// Inventory and reclaim the durable per-task artifact root from the factory
/// workspace contract (GH #196). Directories are named for task IDs, so task
/// lifecycle is the only authority that makes a receipt eligible for removal.
#[derive(Default)]
struct FactoryArtifactInventory {
    root: std::path::PathBuf,
    closed: Vec<std::path::PathBuf>,
    stray: Vec<std::path::PathBuf>,
    error: Option<String>,
    removed: usize,
    dry_run: bool,
}

impl FactoryArtifactInventory {
    fn render(&self) -> String {
        let mut out = format!(
            "\nDurable task artifacts: root={} closed-task candidates={} stray inventory={} mode={}\n",
            self.root.display(),
            self.closed.len(),
            self.stray.len(),
            if self.dry_run {
                "review-only"
            } else {
                "cleanup"
            },
        );
        for path in &self.closed {
            out.push_str(&format!("  - closed-task artifact: {}\n", path.display()));
        }
        for path in &self.stray {
            out.push_str(&format!(
                "  - review-only stray artifact: {}\n",
                path.display()
            ));
        }
        if let Some(error) = &self.error {
            out.push_str(&format!("  - artifact inventory unavailable: {error}\n"));
        } else if self.dry_run && !self.closed.is_empty() {
            out.push_str("  Reclaim closed-task artifacts with gc_cleanup force=true dry_run=false. Strays are never auto-deleted.\n");
        } else if self.removed > 0 {
            out.push_str(&format!(
                "  Closed-task artifact directories removed: {}\n",
                self.removed
            ));
        }
        out
    }
}

fn factory_artifact_inventory(
    root: std::path::PathBuf,
    closed_task_ids: &std::collections::BTreeSet<String>,
) -> FactoryArtifactInventory {
    let mut inventory = FactoryArtifactInventory {
        root,
        dry_run: true,
        ..Default::default()
    };
    let entries = match std::fs::read_dir(&inventory.root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return inventory,
        Err(error) => {
            inventory.error = Some(error.to_string());
            return inventory;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if closed_task_ids.contains(&name) {
            inventory.closed.push(path);
        } else {
            inventory.stray.push(path);
        }
    }
    inventory.closed.sort();
    inventory.stray.sort();
    inventory
}

fn factory_artifact_cleanup(
    root: &std::path::Path,
    closed_task_ids: &std::collections::BTreeSet<String>,
    authorized: bool,
) -> FactoryArtifactInventory {
    let mut inventory = factory_artifact_inventory(root.to_path_buf(), closed_task_ids);
    inventory.dry_run = !authorized;
    if authorized {
        for path in &inventory.closed {
            if std::fs::remove_dir_all(path).is_ok() {
                inventory.removed += 1;
            }
        }
    }
    inventory
}

type LiveFactoryWorkers = std::collections::HashSet<(String, Option<String>)>;

fn live_factory_workers(agent_store: &dyn cas_store::AgentStore) -> LiveFactoryWorkers {
    live_factory_workers_from_agents(agent_store.list(None).unwrap_or_default())
}

fn live_target_cache_worktrees(agent_store: &dyn cas_store::AgentStore) -> Vec<std::path::PathBuf> {
    agent_store
        .list(None)
        .unwrap_or_default()
        .into_iter()
        .filter(|agent| {
            crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness(agent)
                .is_live()
        })
        .filter_map(|agent| {
            agent
                .metadata
                .get("clone_path")
                .map(std::path::PathBuf::from)
        })
        .collect()
}

fn known_target_cache_worktrees(
    cas_root: &std::path::Path,
    config: &crate::config::Config,
    agent_store: &dyn cas_store::AgentStore,
    worktree_store: &dyn cas_store::WorktreeStore,
) -> Vec<std::path::PathBuf> {
    let mut roots = std::collections::HashSet::new();
    roots.extend(
        agent_store
            .list(None)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|agent| {
                agent
                    .metadata
                    .get("clone_path")
                    .map(std::path::PathBuf::from)
            }),
    );
    roots.extend(
        worktree_store
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|worktree| worktree.path),
    );
    if let Some(repo_root) = cas_root.parent() {
        let configured_base = config.worktrees().resolve_base_path(repo_root);
        if let Ok(entries) = std::fs::read_dir(configured_base) {
            roots.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    roots.into_iter().collect()
}

fn live_factory_workers_from_agents(
    agents: impl IntoIterator<Item = cas_types::Agent>,
) -> LiveFactoryWorkers {
    use cas_types::AgentRole;

    agents
        .into_iter()
        .filter(|agent| agent.role == AgentRole::Worker)
        .filter(crate::mcp::tools::service::agent_liveness::is_live_factory_worker)
        .map(|agent| (agent.name, agent.factory_session))
        .collect()
}

fn process_group_has_live_owner(
    record: &crate::ui::factory::process_groups::TrackedProcessGroup,
    live_workers: &LiveFactoryWorkers,
) -> bool {
    live_workers.contains(&(
        record.worker_name.clone(),
        Some(record.factory_session.clone()),
    )) || live_workers.contains(&(record.worker_name.clone(), None))
}

/// Factory sessions currently running, by name.
fn live_factory_sessions() -> std::collections::HashSet<String> {
    crate::ui::factory::SessionManager::new()
        .list_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.is_running)
        .map(|session| session.name)
        .collect()
}

/// Scan for orphan processes and stale server registrations (cas-b7dd, GH #88).
///
/// Process groups belonging to live workers are passed in as protected, so a
/// running worker's own descendants are reported as owned rather than as
/// orphans — that distinction is the difference between a GC and an outage.
fn scan_orphan_processes(
    cas_root: &std::path::Path,
    live_workers: &LiveFactoryWorkers,
) -> crate::ui::factory::orphan_gc::OrphanReport {
    let protected_pgids: std::collections::HashSet<u32> =
        crate::ui::factory::process_groups::list(cas_root)
            .unwrap_or_default()
            .into_iter()
            .filter(|record| {
                crate::ui::factory::process_groups::is_live(record)
                    || process_group_has_live_owner(record, live_workers)
            })
            .map(|record| record.pgid)
            .collect();
    crate::ui::factory::orphan_gc::scan(
        cas_root,
        &live_factory_sessions(),
        &protected_pgids,
        &deregistered_worker_worktrees(cas_root, live_workers),
    )
}

fn deregistered_worker_worktrees(
    cas_root: &std::path::Path,
    live_workers: &LiveFactoryWorkers,
) -> std::collections::HashSet<std::path::PathBuf> {
    let repo_root = cas_root.parent().unwrap_or(cas_root);
    let config = crate::config::Config::load(cas_root).unwrap_or_default();
    let default_root = config.worktrees().resolve_base_path(repo_root);
    crate::ui::factory::SessionManager::new()
        .list_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.is_running)
        .flat_map(|session| {
            let session_name = session.name;
            let worktree_root = default_root.clone();
            session
                .metadata
                .workers
                .into_iter()
                .filter_map(move |worker| {
                    let live = live_workers
                        .contains(&(worker.name.clone(), Some(session_name.clone())))
                        || live_workers.contains(&(worker.name.clone(), None));
                    (!live).then(|| {
                        worker
                            .worktree_path
                            .map(std::path::PathBuf::from)
                            .unwrap_or_else(|| worktree_root.join(worker.name))
                    })
                })
        })
        .filter(|path| path.is_dir())
        .collect()
}

fn orphan_process_groups(
    cas_root: &std::path::Path,
    stale_after_secs: i64,
    live_workers: &LiveFactoryWorkers,
) -> (
    Vec<crate::ui::factory::process_groups::TrackedProcessGroup>,
    Vec<crate::ui::factory::process_groups::TrackedProcessGroup>,
    usize,
    Vec<crate::ui::factory::process_groups::TrackedProcessGroup>,
) {
    let live_sessions: std::collections::HashSet<String> =
        crate::ui::factory::SessionManager::new()
            .list_sessions()
            .unwrap_or_default()
            .into_iter()
            .filter(|session| session.is_running)
            .map(|session| session.name)
            .collect();
    let records = crate::ui::factory::process_groups::list(cas_root).unwrap_or_default();
    let stale_records = records
        .iter()
        .filter(|record| {
            matches!(
                crate::ui::factory::process_groups::status(record),
                crate::ui::factory::process_groups::ProcessGroupStatus::Gone
                    | crate::ui::factory::process_groups::ProcessGroupStatus::FingerprintMismatch
            )
        })
        .count();
    let unverifiable_records: Vec<_> = records
        .iter()
        .filter(|record| {
            crate::ui::factory::process_groups::status(record)
                == crate::ui::factory::process_groups::ProcessGroupStatus::Unverifiable
        })
        .cloned()
        .collect();
    let minimum_age = std::time::Duration::from_secs(stale_after_secs.max(0) as u64);
    let candidates: Vec<_> = records
        .into_iter()
        .filter(crate::ui::factory::process_groups::is_live)
        .filter(|record| crate::ui::factory::process_groups::age(record) >= minimum_age)
        .filter(|record| {
            !live_sessions.contains(&record.factory_session)
                || !process_group_has_live_owner(record, live_workers)
        })
        .collect();
    let (mut live_owned, mut orphans): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|record| process_group_has_live_owner(record, live_workers));
    orphans.sort_by_key(|record| record.pgid);
    live_owned.sort_by_key(|record| record.pgid);
    (orphans, live_owned, stale_records, unverifiable_records)
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

/// Query-time worktree evidence that floors `worker_activity` underneath the
/// event and transcript feeds. A dirty snapshot is live evidence even when a
/// harness did not emit a corresponding `WorkerFileEdited` event.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerActivityWorktreeSnapshot {
    worker_name: String,
    dirty_file_count: usize,
    diffstat: String,
    last_commit: String,
}

/// Whether `worker_activity` has no evidence to render at all. Kept pure so
/// the zero-event worktree floor cannot accidentally be omitted from the
/// early-empty branch during a future response refactor.
fn worker_activity_has_no_rows(
    worker_event_count: usize,
    transcript_activity_count: usize,
    worktree_activity_count: usize,
    terminal_event_count: usize,
    dead_worker_count: usize,
) -> bool {
    worker_event_count == 0
        && transcript_activity_count == 0
        && worktree_activity_count == 0
        && terminal_event_count == 0
        && dead_worker_count == 0
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

/// Gather the live dirty-worktree floor for `worker_activity`.
///
/// This uses the same clone resolution and git-status collector as
/// `worker_status`. It intentionally returns no row for a clean or unavailable
/// worktree; absence is not manufactured into activity.
fn collect_worker_activity_worktree_snapshot(
    cas_root: &std::path::Path,
    agent: &cas_types::Agent,
) -> Option<WorkerActivityWorktreeSnapshot> {
    let WorkerClonePathResolve::Ready(path) = resolve_worker_clone_path(cas_root, agent) else {
        return None;
    };
    let git_status = collect_worker_git_status(&path);
    if !git_status.dirty {
        return None;
    }

    let dirty_file_count = worker_scope_paths(&path).ok()?.len();
    if dirty_file_count == 0 {
        return None;
    }

    let diffstat = run_git(&path, &["diff", "--stat", "HEAD"])
        .ok()
        .filter(|stat| !stat.is_empty())
        .unwrap_or_else(|| "no tracked diffstat (untracked files may be present)".to_string());
    let last_commit = run_git(&path, &["log", "-1", "--format=%h %s"])
        .ok()
        .filter(|commit| !commit.is_empty())
        .unwrap_or_else(|| head_sha_for_display(&git_status.head_sha).to_string());

    Some(WorkerActivityWorktreeSnapshot {
        worker_name: agent.name.clone(),
        dirty_file_count,
        diffstat,
        last_commit,
    })
}

fn format_worker_activity_worktree_snapshot(snapshot: &WorkerActivityWorktreeSnapshot) -> String {
    let diffstat = snapshot.diffstat.replace('\n', "\n    ");
    format!(
        "• {} - live worktree: {} dirty file{} (query-time snapshot; no event required)\n    diffstat: {}\n    last commit: {}\n",
        snapshot.worker_name,
        snapshot.dirty_file_count,
        if snapshot.dirty_file_count == 1 {
            ""
        } else {
            "s"
        },
        diffstat,
        snapshot.last_commit,
    )
}

/// Resolve a registered worker's concrete harness artifact using the same
/// clone-path fallback and harness-aware resolver as `worker_status`.
///
/// Factory agent rows created before clone-path metadata was persisted still
/// resolve through `{cas_root}/worktrees/{worker_name}`; message_status must
/// not silently lose rollout evidence for those legacy/live rows.
pub(crate) fn worker_transcript_path_for_agent(
    cas_root: &std::path::Path,
    agent: &cas_types::Agent,
) -> Option<std::path::PathBuf> {
    let clone_path = match resolve_worker_clone_path(cas_root, agent) {
        WorkerClonePathResolve::Ready(path) => path,
        WorkerClonePathResolve::NotOnDisk { candidate, .. } => candidate,
    };
    let cli = worker_cli_from_agent(agent);
    let session_id = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
    worker_status_transcript_path_for_account(
        clone_path.to_str(),
        session_id,
        cli,
        agent.metadata.get("worker_account_dir").map(String::as_str),
    )
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

/// Paths that belong to the worker's own scope signal.
///
/// Ordinarily this is exactly the path list from `git status --porcelain`.
/// During an unresolved merge, though, Git stages every cleanly merged
/// incoming path in the index. Those paths belong to the branch being merged
/// *into* the worker, not to the worker's contribution; treating that index
/// residue as worker drift caused the cas-7a21 false merge block. In that
/// state, use the worker branch's committed range from the common merge base
/// to `HEAD` instead. The index is deliberately not consulted.
///
/// This is only a status/drift classifier. Operational safety checks that
/// decide whether to stash, rebase, or remove a worktree intentionally keep
/// reading the raw index, because an unfinished merge must never be treated as
/// safe for those destructive actions.
pub(crate) fn worker_scope_paths(
    path: &std::path::Path,
) -> std::result::Result<Vec<String>, String> {
    let merge_head_present = run_git(path, &["rev-parse", "--verify", "-q", "MERGE_HEAD"]).is_ok();
    if merge_head_present {
        let merge_base = run_git(path, &["merge-base", "HEAD", "MERGE_HEAD"])?;
        let range = format!("{merge_base}..HEAD");
        let diff = run_git(path, &["diff", "--name-only", &range])?;
        return Ok(diff
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect());
    }

    // `run_git` intentionally trims its text result for scalar Git replies.
    // Porcelain's leading space is itself a status column, so retain raw stdout
    // here rather than shifting an unstaged path one byte left.
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .map_err(|error| format!("git status --porcelain failed to start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git status --porcelain failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let status = String::from_utf8_lossy(&output.stdout);
    Ok(status
        .lines()
        .filter_map(|line| {
            // `git status --porcelain` has two state columns, one separator,
            // then the path. For a rename, retain the destination path.
            line.get(3..)
                .map(|path| path.rsplit(" -> ").next().unwrap_or(path).trim().to_owned())
        })
        .filter(|path| !path.is_empty())
        .collect())
}

/// The integration branch a worker's task will actually be merged into
/// (cas-5884).
///
/// Sync rebases a worker's worktree onto a ref. If that ref is not the branch
/// the worker's task integrates into, the rebase grafts foreign commits onto
/// the factory branch and rewrites the worker's own already-merged commits —
/// which then read as "N commit(s) from this task not on <target>" at close
/// time, a false count that forces the `commit_receipt` escape hatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchAffinity {
    /// The task integrates into this named branch (parent epic branch, or an
    /// explicit `work_target.target_branch`).
    Branch(String),
    /// Standalone task: it integrates into the repository's trunk.
    Trunk,
    /// The worker holds no task, so sync has no affinity evidence and does not
    /// invent one.
    Unknown,
}

/// A worker's current task binding as far as sync is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerTaskBinding {
    /// Operator-facing task label (id, plus status for parked states).
    pub label: String,
    /// True for statuses whose commits must not be rewritten under the worker
    /// (`InProgress`, `AwaitingMerge`).
    pub blocks_rebase: bool,
    /// Branch this task's work integrates into.
    pub affinity: BranchAffinity,
}

/// Normalize a ref for affinity comparison: `refs/heads/x`, `origin/x` and `x`
/// all name the same integration branch from a worker worktree's point of view.
pub(crate) fn normalize_branch_ref(reference: &str) -> String {
    let reference = reference.trim();
    let reference = reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference);
    reference
        .strip_prefix("origin/")
        .unwrap_or(reference)
        .into()
}

/// Resolve which branch a task integrates into.
///
/// Order (cas-580e): an epic's recorded integration parent wins, then its
/// coordination branch; a task's own target is next, else it is standalone
/// and belongs on trunk.
fn task_branch_affinity(
    store: &dyn crate::store::TaskStore,
    task: &cas_types::Task,
) -> BranchAffinity {
    fn non_empty(value: Option<&String>) -> Option<String> {
        value
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    fn epic_parent_or_branch(epic: &cas_types::Task) -> Option<String> {
        epic.deliverables
            .work_target
            .as_ref()
            .map(|target| target.target_branch.trim())
            .filter(|branch| !branch.is_empty())
            .map(str::to_string)
            .or_else(|| non_empty(epic.branch.as_ref()))
    }

    if task.task_type == cas_types::TaskType::Epic
        && let Some(branch) = epic_parent_or_branch(task)
    {
        return BranchAffinity::Branch(branch);
    }
    if let Ok(Some(epic)) = store.get_parent_epic(&task.id)
        && let Some(branch) = epic_parent_or_branch(&epic)
    {
        return BranchAffinity::Branch(branch);
    }
    if let Some(target) = task
        .deliverables
        .work_target
        .as_ref()
        .map(|wt| wt.target_branch.trim())
        .filter(|b| !b.is_empty())
    {
        return BranchAffinity::Branch(target.to_string());
    }
    BranchAffinity::Trunk
}

/// True when an `AwaitingMerge` task's parked factory branch is already
/// reachable from the task's real integration target.
///
/// `AwaitingMerge` normally means the supervisor still has to land the branch,
/// but a worker can go idle after that landing and before its required re-close.
/// Reporting the normal merge instruction in that state sends the supervisor
/// back to an action that is already complete. This is best-effort status
/// evidence only: unknown Git state retains the conservative normal wording.
fn awaiting_merge_delivery_is_already_integrated(
    cas_root: &std::path::Path,
    task: &cas_types::Task,
    clone_path: Option<&str>,
) -> bool {
    if task.status != cas_types::TaskStatus::AwaitingMerge {
        return false;
    }
    let Some(clone_path) = clone_path else {
        return false;
    };
    let path = std::path::Path::new(clone_path);
    let Ok(task_store) = crate::store::open_task_store_local(cas_root) else {
        return false;
    };
    let target = match task_branch_affinity(task_store.as_ref(), task) {
        BranchAffinity::Branch(branch) => branch,
        // The normal trunk's exact name is repository-specific. Resolve the
        // same local default signal Git uses for worker status, then retain the
        // established `main` fallback for repositories without an origin HEAD.
        BranchAffinity::Trunk => run_git(
            path,
            &["symbolic-ref", "refs/remotes/origin/HEAD", "--short"],
        )
        .unwrap_or_else(|_| "main".to_string()),
        BranchAffinity::Unknown => return false,
    };
    let source = task
        .deliverables
        .parked_branch
        .as_deref()
        .filter(|branch| !branch.trim().is_empty())
        .unwrap_or("HEAD");

    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    let mut candidates = vec![target.to_string()];
    if !target.starts_with("origin/") && !target.starts_with("refs/") {
        candidates.push(format!("origin/{target}"));
    }
    candidates.into_iter().any(|candidate| {
        run_git(path, &["merge-base", "--is-ancestor", source, &candidate]).is_ok()
    })
}

/// Map worker display name -> the task binding sync must respect.
///
/// `InProgress` is the obvious rebase-blocker. `AwaitingMerge` and
/// `AwaitingMerge` are included because their commits are already named by a
/// delivery receipt: rebasing rewrites exactly the SHAs the supervisor is
/// about to verify and merge. `Open` rows are collected too — they carry no
/// rebase block, but they do carry branch affinity for a parked worker
/// (cas-5884).
fn worker_task_bindings(
    cas_root: &std::path::Path,
) -> std::collections::HashMap<String, WorkerTaskBinding> {
    use cas_types::TaskStatus;

    let mut map = std::collections::HashMap::new();
    let Ok(store) = crate::store::open_task_store_local(cas_root) else {
        return map;
    };
    for status in [
        TaskStatus::InProgress,
        TaskStatus::AwaitingMerge,
        TaskStatus::Open,
    ] {
        let Ok(tasks) = store.list(Some(status)) else {
            continue;
        };
        for task in tasks {
            let Some(assignee) = task.assignee.clone() else {
                continue;
            };
            let label = if status == TaskStatus::InProgress {
                task.id.clone()
            } else {
                format!("{} [{}]", task.id, status)
            };
            let affinity = task_branch_affinity(store.as_ref(), &task);
            map.entry(assignee).or_insert(WorkerTaskBinding {
                label,
                blocks_rebase: status != TaskStatus::Open,
                affinity,
            });
        }
    }
    map
}

/// Why `sync_all_workers` must not touch a given worktree (cas-0a6f / GH #103).
///
/// Sync used to rebase every worker worktree unconditionally: uncommitted WIP
/// was stashed without consent, a failed stash pop stranded it silently, and a
/// conflicting rebase left the worktree mid-rebase in a state the worker never
/// initiated. The decision is a pure function so every branch is testable
/// without a git fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncGate {
    Proceed,
    /// Skip this worktree; the string is the operator-facing reason.
    Refuse(String),
}

/// Decide whether a worker worktree may be rebased.
///
/// `force` covers exactly the two consent-shaped cases — dirty tree and a
/// worker mid-task. It deliberately does NOT cover an in-flight rebase: that
/// state was not created by sync, a second rebase on top of it destroys the
/// resolution in progress, and no automated recovery is safe.
pub(crate) fn sync_gate_for_worker(
    worker_name: &str,
    dirty_files: usize,
    mid_rebase: bool,
    in_progress_task: Option<&str>,
    force: bool,
) -> SyncGate {
    if mid_rebase {
        return SyncGate::Refuse(format!(
            "{worker_name} (ALREADY MID-REBASE — sync did not start this and will not rebase on \
             top of it; finish or `git rebase --abort` in the worktree, then re-run sync. \
             force= does not override this)"
        ));
    }
    if let Some(task_id) = in_progress_task
        && !force
    {
        return SyncGate::Refuse(format!(
            "{worker_name} (mid-task on {task_id} — rebasing under a working agent rewrites the \
             commits it is building on; wait for the task to land, or pass force=true)"
        ));
    }
    if dirty_files > 0 && !force {
        return SyncGate::Refuse(format!(
            "{worker_name} ({dirty_files} uncommitted change(s) — refusing to stash and rebase \
             live WIP; commit it, or pass force=true to stash/rebase/restore)"
        ));
    }
    SyncGate::Proceed
}

/// Decide whether a worker worktree may be rebased onto `sync_ref` given the
/// branch its task actually integrates into (cas-5884).
///
/// A fleet-wide `sync_all_workers branch=epic/…` used to rebase EVERY worker,
/// including ones on standalone trunk-targeted tasks. That grafted the epic's
/// unmerged commits onto their factory branch and rewrote their own merged
/// commit, so the later close guard counted commits "not on main" that were in
/// fact merged — the operator had to reach for `commit_receipt` to close
/// correct work.
///
/// Override semantics (design choice, cas-5884): an explicit `worker_names=`
/// targeting IS the override — naming a worker is a deliberate statement about
/// that worktree. `force=` is NOT: it exists to consent to stashing WIP and
/// rebasing under a live agent, neither of which says anything about the
/// branch being correct, and a fleet sweep with `force=true` is exactly the
/// call that caused this bug.
pub(crate) fn sync_affinity_gate(
    worker_name: &str,
    task_label: Option<&str>,
    affinity: &BranchAffinity,
    sync_ref: &str,
    default_branch: &str,
    explicitly_targeted: bool,
) -> SyncGate {
    if explicitly_targeted {
        return SyncGate::Proceed;
    }
    let sync = normalize_branch_ref(sync_ref);
    let task = task_label.unwrap_or("its task");
    match affinity {
        BranchAffinity::Unknown => SyncGate::Proceed,
        BranchAffinity::Branch(branch) => {
            if normalize_branch_ref(branch) == sync {
                SyncGate::Proceed
            } else {
                SyncGate::Refuse(format!(
                    "{worker_name} (branch affinity mismatch — {task} integrates into `{branch}`, \
                     not the requested sync target `{sync_ref}`; rebasing it would graft foreign \
                     commits onto its factory branch and corrupt close accounting. Name it in \
                     worker_names= to sync it anyway)"
                ))
            }
        }
        BranchAffinity::Trunk => {
            if normalize_branch_ref(default_branch) == sync {
                SyncGate::Proceed
            } else {
                SyncGate::Refuse(format!(
                    "{worker_name} (branch affinity mismatch — {task} is standalone and \
                     integrates into trunk `{default_branch}`, not the requested sync target \
                     `{sync_ref}`; rebasing it would graft foreign commits onto its factory \
                     branch and corrupt close accounting. Name it in worker_names= to sync it \
                     anyway)"
                ))
            }
        }
    }
}

/// True when the worktree is sitting in an unfinished rebase.
fn rebase_in_progress(path: &std::path::Path) -> bool {
    for probe in ["rebase-merge", "rebase-apply"] {
        if let Ok(dir) = run_git(path, &["rev-parse", "--git-path", probe]) {
            let dir = dir.trim();
            if dir.is_empty() {
                continue;
            }
            let candidate = std::path::Path::new(dir);
            let resolved = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                path.join(candidate)
            };
            if resolved.exists() {
                return true;
            }
        }
    }
    false
}

/// Count of entries reported by `git status --porcelain`.
fn dirty_file_count(path: &std::path::Path) -> std::result::Result<usize, String> {
    let status = run_git(path, &["status", "--porcelain"])?;
    Ok(status.lines().filter(|l| !l.trim().is_empty()).count())
}

/// A sync attempt that failed in a way the operator must act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncFailure {
    pub message: String,
    /// Set when stashed WIP was NOT restored — the exact ref to recover from.
    pub stranded_stash: Option<String>,
    /// Set when the worktree was left in an unfinished rebase.
    pub mid_rebase: bool,
}

impl SyncFailure {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stranded_stash: None,
            mid_rebase: false,
        }
    }

    /// One-line render for the sync report.
    pub(crate) fn report_line(&self) -> String {
        let mut line = self.message.clone();
        if let Some(stash) = self.stranded_stash.as_deref() {
            line.push_str(&format!(
                " — WIP IS NOT LOST: recover with `{}` in the worktree",
                stash_recovery_command(stash)
            ));
        }
        if self.mid_rebase {
            line.push_str(
                " — WORKTREE LEFT MID-REBASE: resolve or `git rebase --abort` before using it",
            );
        }
        line
    }
}

/// The recovery command an operator can actually run for a stranded stash.
///
/// Two git details this must respect, both verified against real git rather
/// than assumed:
/// - `git stash pop`/`drop` reject a bare commit SHA ("is not a stash
///   reference"); only `apply`/`show` accept one. The ref recorded here is a
///   SHA on purpose (a later stash push shifts `stash@{0}` off this entry), so
///   the instruction must be `apply`.
/// - `git stash show -p` prints nothing for untracked-only WIP unless
///   `--include-untracked` is passed — and untracked WIP is exactly what the
///   auto-stash sweeps up. Without the flag the inspection reads as "empty",
///   i.e. "my work is gone".
/// - `apply` itself refuses while a file of the same name exists in the
///   worktree ("already exists, no checkout") — which is usually the very
///   reason the pop failed. Saying only "run apply" would send the operator
///   into the same wall, so the caveat and its way out are part of the text.
pub(crate) fn stash_recovery_command(stash_ref: &str) -> String {
    format!(
        "git stash show -p --include-untracked {stash_ref}   # what is in it\n    \
         git stash apply {stash_ref}                          # restore it\n    \
         # if apply says \"already exists, no checkout\", move that file aside \
         (that collision is why the restore failed) and re-run apply; the stash \
         entry stays until you drop it by its `git stash list` index"
    )
}

/// Resolve the stash just pushed to a durable revision so the recovery
/// instruction survives later pushes shifting `stash@{0}`.
///
/// The `^0` suffix is significant: `git stash` treats a revision made only of
/// decimal digits as a positional stash index. A randomly generated short SHA
/// can be all-decimal, which would make a recovery command address
/// `stash@{N}` instead of this stash. Dereferencing the commit keeps it a
/// shell-safe single token while unambiguously selecting the recorded object.
fn resolve_stash_ref(path: &std::path::Path) -> String {
    run_git(path, &["rev-parse", "refs/stash"])
        .map(|sha| durable_stash_ref(&sha))
        .unwrap_or_else(|_| "stash@{0}".to_string())
}

fn durable_stash_ref(sha: &str) -> String {
    format!("{}^0", &sha[..sha.len().min(12)])
}

fn sync_worker_clone(
    path: &std::path::Path,
    sync_ref: &str,
) -> std::result::Result<String, SyncFailure> {
    let status = run_git(path, &["status", "--porcelain"]).map_err(SyncFailure::plain)?;
    let mut stash_ref: Option<String> = None;

    if !status.trim().is_empty() {
        let stash_msg = format!(
            "cas-factory-auto-sync {}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
        );
        let stash_out = run_git(
            path,
            &["stash", "push", "--include-untracked", "-m", &stash_msg],
        )
        .map_err(SyncFailure::plain)?;
        if !stash_out.contains("No local changes") {
            stash_ref = Some(resolve_stash_ref(path));
        }
    }

    let _ = run_git(path, &["fetch", "origin"]);

    if let Err(rebase_err) = run_git(path, &["rebase", sync_ref]) {
        let abort = run_git(path, &["rebase", "--abort"]);
        let still_mid_rebase = abort.is_err() && rebase_in_progress(path);
        let pop_failed = match stash_ref.as_ref() {
            Some(_) => run_git(path, &["stash", "pop"]).is_err(),
            None => false,
        };
        return Err(SyncFailure {
            message: format!("rebase failed: {rebase_err}"),
            stranded_stash: if pop_failed { stash_ref } else { None },
            mid_rebase: still_mid_rebase,
        });
    }

    if stash_ref.is_some()
        && let Err(pop_err) = run_git(path, &["stash", "pop"])
    {
        return Err(SyncFailure {
            message: format!("sync applied but stash pop failed: {pop_err}"),
            stranded_stash: stash_ref,
            mid_rebase: false,
        });
    }

    Ok(if stash_ref.is_some() {
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
/// hook-capable harnesses (Claude, Grok) always get a Cassy event recorded
/// for their real work. That reasoning was falsified live, on this
/// factory, on the shipped binary: Claude worker `interrupt-fixer` was
/// reported `last activity: 401s ago ⚠ STALLED` at 2026-07-27T21:29:20Z
/// while its transcript's last record was 21:29:18.757Z — two seconds
/// earlier — with tool calls in every single minute since 21:22 (cas-c2c2).
/// The real defect was never "Codex has no hooks"; it's that Cassy's
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

/// Fold the latest parsed harness turn-start watermark into stall freshness.
///
/// Transcript mtime remains useful for tool activity, but a concrete turn
/// record is stronger evidence and is the signal that makes Claude workers
/// participate in the same stall classification as Codex and Grok.
fn last_worker_activity_secs_with_harness_turn(
    events: &[cas_types::Event],
    agent_id: &str,
    cli: cas_mux::SupervisorCli,
    transcript_path: Option<&std::path::Path>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<(i64, &'static str)> {
    let activity =
        last_worker_activity_secs_with_transcript(events, agent_id, cli, transcript_path);
    let turn_start = transcript_path
        .and_then(|path| {
            crate::mcp::tools::service::harness_observation::latest_turn_observations(path, cli)
                .wake
        })
        .map(|wake| ((now - wake.at).num_seconds().max(0), "turn start"));

    match (activity, turn_start) {
        (Some(existing), Some(turn)) if existing.0 <= turn.0 => Some(existing),
        (_, Some(turn)) => Some(turn),
        (existing, None) => existing,
    }
}

/// Latest explicit file-edit event for one worker.
fn latest_worker_file_write_at(
    events: &[cas_types::Event],
    agent_id: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    events
        .iter()
        .filter(|event| {
            event.event_type == cas_types::EventType::WorkerFileEdited
                && (event.session_id.as_deref() == Some(agent_id) || event.entity_id == agent_id)
        })
        .map(|event| event.created_at)
        .max()
}

fn format_worker_progress_timestamps(
    last_outbound: Option<chrono::DateTime<chrono::Utc>>,
    last_file_write: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let render = |label: &str, at: Option<chrono::DateTime<chrono::Utc>>| match at {
        Some(at) => format!(
            "\n    {label}: {} ({}s ago)",
            at.to_rfc3339(),
            (now - at).num_seconds().max(0)
        ),
        None => format!("\n    {label}: unobserved"),
    };
    format!(
        "{}{}",
        render("last outbound message", last_outbound),
        render("last file write", last_file_write)
    )
}

/// Render the latest real harness turn observation for `worker_status`.
///
/// This is intentionally separate from `last activity`: transcript mtime is
/// useful freshness evidence, but it does not name the state transition a
/// supervisor needs when diagnosing a swallowed delivery. Only parsed harness
/// records reach this line; an unresolved artifact remains explicitly
/// unobserved.
fn format_harness_turn_observation(
    cli: cas_mux::SupervisorCli,
    artifact_path: Option<&std::path::Path>,
) -> String {
    format_harness_turn_observation_at(cli, artifact_path, chrono::Utc::now())
}

fn format_harness_turn_observation_at(
    cli: cas_mux::SupervisorCli,
    artifact_path: Option<&std::path::Path>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let Some(path) = artifact_path else {
        return format!(
            "\n    harness turn: unobserved ({} artifact unresolved)",
            cli.backend().name()
        );
    };
    let observations =
        crate::mcp::tools::service::harness_observation::latest_turn_observations(path, cli);
    let Some(wake) = observations.wake else {
        let completion = observations
            .completion
            .map_or_else(String::new, |completion| {
                format!(
                    "; completion observed at {} from {}",
                    completion.at.to_rfc3339(),
                    completion.evidence
                )
            });
        return format!(
            "\n    harness turn: unobserved (resolved {} artifact has no authoritative turn-start record{})",
            cli.backend().name(),
            completion
        );
    };
    let age = (now - wake.at).num_seconds().max(0);
    let reaction = observations.reaction.map_or_else(
        || "reaction unobserved".to_string(),
        |reaction| format!("reaction observed at {}", reaction.at.to_rfc3339()),
    );
    let completion = observations.completion.map_or_else(
        || "completion unobserved".to_string(),
        |completion| format!("completion observed at {}", completion.at.to_rfc3339()),
    );
    format!(
        "\n    harness turn: started {age}s ago at {} ({reaction}; {completion}; artifact-backed: {})",
        wake.at.to_rfc3339(),
        wake.evidence
    )
}

/// cas-78bf: elapsed time for the sustained assigned-but-unstarted state.
///
/// Assignment and the most recent harness-aware activity are both valid
/// baselines; whichever is newer starts (or resets) the quiet window. An
/// in-flight tool call is direct busy evidence and suppresses escalation.
fn assigned_unstarted_elapsed_secs(
    assigned_at: chrono::DateTime<chrono::Utc>,
    last_activity_secs_ago: Option<i64>,
    threshold_secs: i64,
    in_flight_tool_call: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<i64> {
    if in_flight_tool_call {
        return None;
    }
    let assigned_elapsed = (now - assigned_at).num_seconds();
    if assigned_elapsed < 0 {
        return None;
    }
    let quiet_elapsed = last_activity_secs_ago
        .map(|activity_elapsed| assigned_elapsed.min(activity_elapsed))
        .unwrap_or(assigned_elapsed);
    (quiet_elapsed >= threshold_secs).then_some(assigned_elapsed)
}

fn format_assigned_unstarted_status(
    task_id: &str,
    elapsed_secs: i64,
    threshold_secs: i64,
) -> String {
    format!(
        "\n    ⚠ ASSIGNED BUT UNSTARTED: {task_id} was assigned {elapsed_secs}s ago and remains unstarted with no recent activity (threshold: {threshold_secs}s; possible missed wake — inspect the queued task message)"
    )
}

/// cas-e728 (GH #105): does this harness publish an authoritative turn-start
/// artifact Cassy can read?
///
/// All supported harnesses now publish a readable turn-start artifact:
/// Codex and Grok use rollout/signals records, while Claude uses top-level
/// textual `user` records in its transcript (tool-result user records are
/// excluded by the parser). This keeps stall classification artifact-backed
/// instead of inferring wake state from inbox persistence.
fn harness_publishes_turn_start(cli: cas_mux::SupervisorCli) -> bool {
    matches!(
        cli,
        cas_mux::SupervisorCli::Claude
            | cas_mux::SupervisorCli::Codex
            | cas_mux::SupervisorCli::Grok
    )
}

/// How many unseen inbox rows `worker_status` reads per worker in order to
/// classify them. Status is a human-facing summary, not a mailbox dump; past
/// this depth the exact composition of the backlog changes no verdict (a
/// worker with 200 unread rows is in trouble either way), and the authoritative
/// unread COUNT still comes from the store, so nothing is silently lost.
const WORKER_INBOX_PEEK_LIMIT: usize = 200;

/// cas-f08d (GH #147): a live, not-yet-due reminder the worker is waiting on.
///
/// This is the receipt for the wait pattern every worker is told to follow —
/// act once, arm a reminder, end the turn — and it is the only evidence Cassy
/// has that a silent Claude worker went quiet ON PURPOSE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingReminderWait {
    /// Reminder ID, as the supervisor sees it in `remind_list`.
    id: i64,
    /// Seconds until it fires (always > 0; due/overdue reminders are excluded).
    due_in_secs: i64,
    /// When the reminder was armed. A reminder armed AFTER an unread reminder
    /// delivery is proof the worker woke, acted on that delivery, and re-armed:
    /// the row is stale bookkeeping, not unheard mail.
    armed_at: chrono::DateTime<chrono::Utc>,
}

/// A single unseen prompt-queue row, reduced to what classification needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnseenDelivery {
    /// Whether this row is a fired-reminder delivery rather than a message
    /// somebody sent the worker.
    is_reminder_delivery: bool,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl UnseenDelivery {
    fn from_queued(prompt: &cas_store::QueuedPrompt) -> Self {
        Self {
            is_reminder_delivery: is_reminder_delivery(&prompt.prompt),
            created_at: prompt.created_at,
        }
    }
}

/// A worker's inbox as `worker_status` reasons about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct WorkerInbox {
    /// Unread rows that still mean something — reminder deliveries the worker
    /// has demonstrably consumed are excluded (cas-f08d).
    unread: usize,
    /// Age of the oldest such row.
    oldest_unread_secs: Option<i64>,
    /// Set only when the worker is holding a live reminder and has nothing
    /// genuinely unread: the sanctioned wait, not a stall.
    reminder_wait: Option<PendingReminderWait>,
}

/// Whether a queued prompt is the factory daemon delivering a fired reminder.
///
/// The wire format is owned by `cas_store`, which also writes it in
/// `fire_reminder` — producer and classifier share one definition on purpose,
/// so a future wording change fails to compile instead of silently reviving
/// this false positive.
fn is_reminder_delivery(prompt: &str) -> bool {
    cas_store::parse_reminder_delivery_id(prompt).is_some()
}

/// cas-f08d (GH #147): decide what a worker's unseen rows actually mean.
///
/// The rule that keeps the wedge alarm honest: presence of a pending reminder
/// NEVER suppresses anything on its own. A reminder delivery is discounted only
/// when the worker armed a NEW reminder after that delivery arrived — that
/// ordering is the proof of wake. Anything else (a work message, a reminder
/// delivery with no later re-arm) stays unread and keeps its alarm.
///
/// `total_unread` and `store_oldest` are the store's authoritative figures;
/// `rows` is a bounded peek of the same set, so a backlog deeper than
/// [`WORKER_INBOX_PEEK_LIMIT`] can only ever under-discount — never invent
/// silence that is not there.
fn classify_worker_inbox(
    total_unread: usize,
    store_oldest: Option<i64>,
    rows: &[UnseenDelivery],
    pending: Option<PendingReminderWait>,
    now: chrono::DateTime<chrono::Utc>,
) -> WorkerInbox {
    let consumed = |row: &UnseenDelivery| {
        row.is_reminder_delivery && pending.is_some_and(|wait| wait.armed_at > row.created_at)
    };
    let discounted = rows.iter().filter(|row| consumed(row)).count();
    let unread = total_unread.saturating_sub(discounted);
    let oldest_unread_secs = if unread == 0 {
        None
    } else {
        rows.iter()
            .find(|row| !consumed(row))
            .map(|row| (now - row.created_at).num_seconds().max(0))
            // Every peeked row was discounted yet the store still counts
            // unread rows beyond the peek window: keep the store's age rather
            // than reporting none.
            .or(store_oldest)
    };
    WorkerInbox {
        unread,
        oldest_unread_secs,
        reminder_wait: if unread == 0 { pending } else { None },
    }
}

/// Live, not-yet-due time-based reminders per target agent ID.
///
/// Only TIME-based reminders qualify. An event-based reminder may never fire at
/// all, so holding one proves nothing about whether the worker is awake — using
/// it to suppress the wedge alarm would mask a genuinely dead worker forever.
/// Where a worker holds several, the soonest-due one is reported: that is the
/// one that decides how long the quiet is expected to last.
fn pending_reminder_waits(
    cas_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
) -> std::collections::HashMap<String, PendingReminderWait> {
    use crate::store::open_reminder_store;

    let Ok(store) = open_reminder_store(cas_root) else {
        return std::collections::HashMap::new();
    };
    let Ok(pending) = store.list_all_pending() else {
        return std::collections::HashMap::new();
    };
    let mut waits: std::collections::HashMap<String, PendingReminderWait> =
        std::collections::HashMap::new();
    for reminder in pending {
        if reminder.trigger_type != cas_store::ReminderTriggerType::Time {
            continue;
        }
        let Some(trigger_at) = reminder.trigger_at else {
            continue;
        };
        let due_in_secs = (trigger_at - now).num_seconds();
        if due_in_secs <= 0 {
            continue;
        }
        let wait = PendingReminderWait {
            id: reminder.id,
            due_in_secs,
            armed_at: reminder.created_at,
        };
        waits
            .entry(reminder.target_id)
            .and_modify(|existing| {
                if wait.due_in_secs < existing.due_in_secs {
                    *existing = wait;
                }
            })
            .or_insert(wait);
    }
    waits
}

/// cas-f08d (GH #147): the state a healthy worker following the mandated wait
/// pattern is actually in.
///
/// Worker guidance forbids polling: check once, arm a reminder, end the turn.
/// That discipline is indistinguishable from a wedged pane by silence alone, so
/// before this line existed it was reported as `⚠ NOT WAKING` and answered with
/// a turn-breaking interrupt — punishing exactly the behaviour the factory asks
/// for. The banner must therefore say who will wake the worker and when, and
/// say plainly that interrupting is the wrong move.
fn format_reminder_wait_status(
    last_activity: Option<(i64, &'static str)>,
    wait: PendingReminderWait,
) -> String {
    let quiet = match last_activity {
        Some((secs, phase)) => format!("quiet since {secs}s ago (last: {phase})"),
        None => "quiet with no activity in the last 10m".to_string(),
    };
    let due = format_due_in(wait.due_in_secs);
    format!(
        "\n    waiting on reminder #{} due in {due} — {quiet}, nothing unread. This is the sanctioned wait pattern (act once, arm a reminder, end the turn); the reminder will wake it, so do NOT interrupt or respawn",
        wait.id
    )
}

/// Human-readable "due in" for a reminder countdown.
fn format_due_in(secs: i64) -> String {
    let minutes = secs / 60;
    let seconds = secs % 60;
    if minutes > 0 && seconds > 0 {
        format!("{minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

/// cas-e728 (GH #105): the honest replacement for `⚠ STALLED` on a
/// turn-unobservable worker that is still heartbeating.
///
/// States only what is known. Cassy cannot see Claude turn boundaries at all
/// (the same row says `harness turn: unobserved` two lines down), so this must
/// not assert that no turn is running — a worker twenty minutes into a
/// `cargo build` would read as free, and the supervisor's natural response is
/// to reset a live worker and discard its in-flight work.
fn format_between_turns_status(
    last_activity: Option<(i64, &'static str)>,
    unread_inbox: usize,
) -> String {
    let mail = match unread_inbox {
        0 => "inbox empty — nothing is waiting on it".to_string(),
        1 => "1 unread message waiting".to_string(),
        n => format!("{n} unread messages waiting"),
    };
    match last_activity {
        Some((secs, phase)) => format!(
            "\n    between turns since {secs}s ago (last: {phase}); {mail}. Turn-based worker: Cassy cannot see Claude turn boundaries, so quiet is not evidence either way"
        ),
        None => format!(
            "\n    between turns: no activity in last 10m; {mail}. Turn-based worker: Cassy cannot see Claude turn boundaries, so quiet is not evidence either way"
        ),
    }
}

/// cas-e728 (GH #105): the real stall signal for a harness whose turns Cassy
/// cannot observe.
///
/// Silence alone proves nothing about a Claude worker — but silence *after it
/// was handed work* does. A message left unconsumed past the stall threshold
/// while the worker produced no activity means the wake-up did not take: the
/// harness is wedged (this repo's own CLAUDE.md documents a Claude UI crash
/// that leaves the process alive and heartbeating with a dead pane), the
/// delivery was lost, or the worker is stuck mid-turn. All three need a human,
/// and none are visible from the heartbeat — which the daemon stamps purely
/// from process liveness, independent of turn execution.
///
/// This is what keeps the alarm honest after `⚠ STALLED` was narrowed: the
/// flag moves from "it is quiet" (always true between turns) to "it was given
/// work and did not react" (only true when something is actually wrong).
fn format_not_waking_status(
    last_activity: Option<(i64, &'static str)>,
    unread_inbox: usize,
    oldest_unread_secs: i64,
) -> String {
    let quiet = match last_activity {
        Some((secs, phase)) => format!("last activity {secs}s ago ({phase})"),
        None => "no activity in the last 10m".to_string(),
    };
    let plural = if unread_inbox == 1 { "" } else { "s" };
    format!(
        "\n    ⚠ NOT WAKING: {unread_inbox} message{plural} unread for {oldest_unread_secs}s and {quiet}. The worker was handed work and has not reacted — check its pane for a wedged harness, then re-send or respawn"
    )
}

/// Whether old unread mail is evidence that a worker failed to react.
///
/// An unread row is historical evidence. It becomes a NOT WAKING signal only
/// when the worker has also been inactive for the same sustained interval.
/// In particular, a recent activity / turn-start observation proves the worker
/// is alive *after* old inbox residue was created, so that residue must not
/// accuse the worker of failing to wake. Keep this predicate separate from the
/// broader `is_worker_stalled` classifier: cas-058e can extend the shared
/// liveness inputs without having to reconstruct this conjunction from an
/// output-formatting branch.
fn has_unreacted_stale_inbox(
    unread_inbox: usize,
    oldest_unread_secs: Option<i64>,
    last_activity: Option<(i64, &'static str)>,
    inactivity_threshold_secs: i64,
) -> bool {
    unread_inbox > 0
        && oldest_unread_secs.is_some_and(|age| age >= inactivity_threshold_secs)
        && last_activity.is_none_or(|(age, _)| age >= inactivity_threshold_secs)
}

/// Render the highest-priority worker-status alert.
///
/// A confirmed InProgress stall is more urgent than a second assigned Open
/// task that has not started, so it must win when both states coexist.
///
/// cas-e728 (GH #105): `⚠ STALLED` is only honest when silence is evidence.
/// It is kept for a worker whose HEARTBEAT has lapsed (the genuine
/// no-heartbeat stall — that worker really has stopped, whatever its harness)
/// and for harnesses that publish a turn-start artifact. A heartbeating
/// worker on a turn-unobservable harness gets the between-turns line instead.
fn format_priority_worker_status_alert(
    stalled: bool,
    last_activity: Option<(i64, &'static str)>,
    stall_threshold_secs: i64,
    assigned_unstarted: Option<(&str, i64, i64)>,
    turn_start_observable: bool,
    heartbeat_elapsed_secs: i64,
    inbox: WorkerInbox,
) -> Option<String> {
    let WorkerInbox {
        unread: unread_inbox,
        oldest_unread_secs,
        reminder_wait,
    } = inbox;
    if stalled {
        let heartbeat_lapsed = heartbeat_elapsed_secs >= WORKER_STALE_SECS;
        if !turn_start_observable && !heartbeat_lapsed {
            // Handed work and did not react: a real, actionable stall the
            // heartbeat cannot see. Old unread mail alone is not enough: the
            // worker must also have been inactive for the entire threshold.
            if has_unreacted_stale_inbox(
                unread_inbox,
                oldest_unread_secs,
                last_activity,
                stall_threshold_secs,
            ) {
                let oldest = oldest_unread_secs.expect("predicate requires unread age");
                return Some(format_not_waking_status(
                    last_activity,
                    unread_inbox,
                    oldest,
                ));
            }
            // A second assigned-but-unstarted task is a separate, still-valid
            // alarm — it is about an untouched assignment, not about silence —
            // so narrowing the stall flag must not swallow it.
            if let Some((task_id, elapsed, threshold)) = assigned_unstarted {
                return Some(format_assigned_unstarted_status(
                    task_id, elapsed, threshold,
                ));
            }
            // cas-f08d (GH #147): quiet WITH a live reminder and nothing
            // unread is the mandated wait, not an absence of news — name the
            // reminder that will end it instead of hedging.
            if let Some(wait) = reminder_wait {
                return Some(format_reminder_wait_status(last_activity, wait));
            }
            return Some(format_between_turns_status(last_activity, unread_inbox));
        }
        return Some(match last_activity {
            Some((secs, phase)) => format!(
                "\n    last activity: {secs}s ago ({phase}) ⚠ STALLED (no activity ≥{stall_threshold_secs}s while task in progress)"
            ),
            None => "\n    ⚠ STALLED: no activity in last 10m while task in progress".to_string(),
        });
    }

    assigned_unstarted.map(|(task_id, elapsed, threshold)| {
        format_assigned_unstarted_status(task_id, elapsed, threshold)
    })
}

/// cas-9829: whether a `worker_status` row should render the `⚠ STALLED`
/// marker instead of the soft "may be investigating or idle" hedge.
///
/// True when the worker has an in-progress task (a lease and/or a real
/// task assignment — see the `has_in_progress_task` computation in
/// `factory_worker_status`, cas-d165 Finding 2) AND there is no in-flight
/// tool call or live background process (cas-d165/cas-058e) AND either:
/// - its last observable activity is at/past `stall_threshold_secs`, or
/// - no activity was observed at all within the query window (`None`).
///
/// A worker with no in-progress task is never "stalled" in this sense —
/// idle-with-no-task is a distinct, already-signaled state (`WorkerIdle`).
///
/// `has_active_work` is the shared evidence cas-7e85 / cas-058e / `cas factory
/// is-wedged` consume —
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
    has_active_work: bool,
) -> bool {
    if !has_in_progress_task || has_active_work {
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
    let escaped = escaped_project_slug(clone_path);
    format!("~/.claude/projects/{escaped}/{session_id}.jsonl")
}

/// Claude Code's per-cwd directory name: the absolute path with `/` and `.`
/// collapsed to `-`.
fn escaped_project_slug(clone_path: &str) -> String {
    clone_path
        .chars()
        .map(|c| match c {
            '/' | '.' => '-',
            other => other,
        })
        .collect()
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
        // cas-7296 owns CAS↔OpenCode transcript mapping; invent no real path.
        cas_mux::SupervisorCli::OpenCode => TranscriptResolution::Synthesized(String::new()),
    }
}

fn resolve_claude_transcript(
    projects_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
) -> TranscriptResolution {
    let roots: Vec<std::path::PathBuf> = projects_dir
        .map(std::path::Path::to_path_buf)
        .into_iter()
        .collect();
    resolve_claude_transcript_in_roots(&roots, clone_path, session_id)
}

/// cas-9e81: the same Claude resolution, but over EVERY projects root a
/// session could have been written under.
///
/// A two-subscription factory runs some panes under `CLAUDE_CONFIG_DIR`
/// (e.g. `~/.claude-alt`) and others under the default `~/.claude`, and
/// Claude Code writes each session's transcript beneath the config dir its
/// own process was launched with. A single hardcoded `~/.claude/projects`
/// root therefore resolves to `None` for every pane on the other account —
/// and `None` is not a neutral outcome downstream: the daemon's wake gate
/// reads an unresolvable transcript as "tool call in flight" and silently
/// refuses to wake that recipient forever.
///
/// Globbing several roots is safe and cheap: the match key is the session
/// UUID, which is unique across accounts, so extra roots can only ever add
/// the one true file (or nothing). Multiple hits still fall through to the
/// existing `Ambiguous` handling.
fn resolve_claude_transcript_in_roots(
    projects_dirs: &[std::path::PathBuf],
    clone_path: Option<&str>,
    session_id: &str,
) -> TranscriptResolution {
    let synthesized = clone_path.map(|p| synthesized_transcript_path(p, session_id));
    if projects_dirs.is_empty() {
        return TranscriptResolution::Synthesized(
            synthesized.unwrap_or_else(|| synthesized_unknown_clone_path(session_id)),
        );
    }
    let mut matches: Vec<std::path::PathBuf> = Vec::new();
    let mut truncated = false;
    for projects in projects_dirs {
        let (found, hit_cap) = glob_transcript_candidates(projects, session_id);
        truncated |= hit_cap;
        for path in found {
            if matches.len() >= MAX_TRANSCRIPT_CANDIDATES {
                truncated = true;
                break;
            }
            if !matches.contains(&path) {
                matches.push(path);
            }
        }
    }
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
#[cfg(test)]
pub(crate) fn default_claude_projects_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Every `projects` root a Claude session on this host could have been
/// written under (cas-9e81): the default `~/.claude` plus the active
/// `CLAUDE_CONFIG_DIR`, deduplicated — the same set `cas hook` already keeps
/// hooked, so the two cannot drift apart on a two-account install.
pub(crate) fn default_claude_projects_dirs() -> Vec<std::path::PathBuf> {
    claude_projects_dirs_from(
        dirs::home_dir().as_deref(),
        std::env::var("CLAUDE_CONFIG_DIR").ok().as_deref(),
    )
}

/// Injectable half of [`default_claude_projects_dirs`].
pub(crate) fn claude_projects_dirs_from(
    home: Option<&std::path::Path>,
    env_config_dir: Option<&str>,
) -> Vec<std::path::PathBuf> {
    crate::cli::hook::config_gen::known_claude_config_dirs_from(home, env_config_dir)
        .into_iter()
        .map(|dir| dir.join("projects"))
        .collect()
}

/// Harness-appropriate transcript search roots. Claude may legitimately have
/// more than one (see [`default_claude_projects_dirs`]); Codex and Grok have a
/// single root each.
fn default_transcript_roots(cli: cas_mux::SupervisorCli) -> Vec<std::path::PathBuf> {
    match cli {
        cas_mux::SupervisorCli::Grok => default_grok_sessions_dir().into_iter().collect(),
        cas_mux::SupervisorCli::Codex => default_codex_sessions_dir().into_iter().collect(),
        cas_mux::SupervisorCli::Claude => default_claude_projects_dirs(),
        // cas-7296 owns OpenCode transcript roots; shared SQLite is not a transcript.
        cas_mux::SupervisorCli::OpenCode => Vec::new(),
    }
}

/// Return the rollout root for the account a Codex worker was actually
/// launched under.  `worker_account_dir` is durable spawn metadata, whereas
/// this process's `CODEX_HOME` belongs to whichever supervisor/daemon happens
/// to be answering an operator query.  Those are often different accounts.
fn codex_sessions_dir_for_worker_account(account_dir: Option<&str>) -> Option<std::path::PathBuf> {
    let account_dir = account_dir?.trim();
    if account_dir.is_empty() {
        return None;
    }
    let expanded = account_dir.strip_prefix('~').map_or_else(
        || std::path::PathBuf::from(account_dir),
        |suffix| {
            dirs::home_dir()
                .map(|home| home.join(suffix.trim_start_matches('/')))
                .unwrap_or_else(|| std::path::PathBuf::from(account_dir))
        },
    );
    Some(expanded.join("sessions"))
}

/// Harness transcript roots for one registered worker.  Codex is special:
/// each worker may have been spawned under a named `CODEX_HOME`, and querying
/// from a supervisor must not silently substitute the supervisor's account.
fn transcript_roots_for_worker(
    cli: cas_mux::SupervisorCli,
    account_dir: Option<&str>,
) -> Vec<std::path::PathBuf> {
    if cli == cas_mux::SupervisorCli::Codex {
        return codex_sessions_dir_for_worker_account(account_dir)
            .or_else(default_codex_sessions_dir)
            .into_iter()
            .collect();
    }
    default_transcript_roots(cli)
}

/// [`resolve_transcript`] over a set of roots rather than a single one.
///
/// Only Claude actually searches more than one (cas-9e81); Codex and Grok keep
/// their existing single-root resolution byte-for-byte so this change cannot
/// alter their behavior.
pub(crate) fn resolve_transcript_in_roots(
    roots: &[std::path::PathBuf],
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> TranscriptResolution {
    match cli {
        cas_mux::SupervisorCli::Claude => {
            resolve_claude_transcript_in_roots(roots, clone_path, session_id)
        }
        _ => resolve_transcript(
            roots.first().map(std::path::PathBuf::as_path),
            clone_path,
            session_id,
            cli,
        ),
    }
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
            synthesized: synthesized
                .unwrap_or_else(|| synthesized_unknown_grok_clone_path(session_id)),
            truncated,
        },
    }
}

// ---------------------------------------------------------------------------
// cas-c655: Codex rollout resolution.
//
// Codex does NOT use Claude's `~/.claude/projects/<escaped-cwd>/<session>.jsonl`
// layout. Factory workers get a Cassy session id of the form
// `codex-<name>-<uuid>` (see `PtyConfig::codex`), but on-disk rollouts live at:
//   ~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<rollout-uuid>.jsonl
// with `session_meta.payload.cwd` equal to the worker's clone_path and a
// different rollout UUID than the Cassy session id. Matching is therefore by
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

/// Whether a resolved Codex rollout records the terminal account-limit failure
/// that leaves its MCP child heartbeating while no harness turn can proceed.
///
/// This intentionally requires the exact user-facing terminal error and the
/// machine-readable exhausted-credit state in the same bounded tail. A generic
/// error, an old rate-limit record, or an unresolved rollout is not enough to
/// declare a live worker unavailable.
pub(crate) fn worker_reports_usage_limit(
    cas_root: &std::path::Path,
    agent: &cas_types::Agent,
) -> bool {
    matches!(
        worker_usage_limit_evidence(cas_root, agent),
        UsageLimitEvidence::Limited { .. }
    )
}

/// The rollout scanner has three states.  In particular, an unreadable or
/// unresolved rollout is not a recovery: callers retain a previously-open
/// episode until they have affirmative later-turn evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UsageLimitEvidence {
    Limited { first_evidence: String },
    Recovered,
    Unavailable,
}

pub(crate) fn worker_usage_limit_evidence(
    cas_root: &std::path::Path,
    agent: &cas_types::Agent,
) -> UsageLimitEvidence {
    let cli = worker_cli_from_agent(agent);
    let rollout = worker_transcript_path_for_agent(cas_root, agent);
    codex_rollout_usage_limit_evidence(rollout.as_deref(), cli)
}

/// Account health read from the worker's own transcript.
///
/// The sibling of [`worker_usage_limit_evidence`] for the failure that hides
/// even better than an exhausted account: a harness that cannot authenticate
/// ends its first turn in about a second and then heartbeats forever, so the
/// worker reads as live, assigned and merely slow (cas-8a55).
pub(crate) fn worker_auth_failure_evidence(
    cas_root: &std::path::Path,
    agent: &cas_types::Agent,
) -> crate::factory_auth_health::AuthFailureEvidence {
    use crate::factory_auth_health::AuthFailureEvidence;
    let cli = worker_cli_from_agent(agent);
    let Some(transcript) = worker_transcript_path_for_agent(cas_root, agent) else {
        return AuthFailureEvidence::Unavailable;
    };
    let Some(tail) = read_transcript_tail(&transcript) else {
        return AuthFailureEvidence::Unavailable;
    };
    match cli {
        cas_mux::SupervisorCli::Codex => {
            crate::factory_auth_health::codex_rollout_auth_failure(&tail)
        }
        cas_mux::SupervisorCli::Claude => {
            crate::factory_auth_health::claude_transcript_auth_failure(&tail)
        }
        // Harnesses without a transcript reader must not be reported as
        // failing on evidence nobody collected.
        _ => AuthFailureEvidence::Unavailable,
    }
}

/// The account directory a worker was actually spawned against, which is what
/// a remedy has to name on a host running several of them.
pub(crate) fn worker_account_dir(agent: &cas_types::Agent) -> Option<String> {
    agent
        .metadata
        .get("worker_account_dir")
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty())
}

/// Bounded tail read shared by the transcript scanners. A transcript can grow
/// without limit; the terminal records that matter are always at its end.
fn read_transcript_tail(path: &std::path::Path) -> Option<String> {
    const TAIL_BYTES: u64 = 256 * 1024;
    let metadata = std::fs::metadata(path).ok()?;
    let start = metadata.len().saturating_sub(TAIL_BYTES);
    let mut file = std::fs::File::open(path).ok()?;
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = String::new();
    file.read_to_string(&mut tail).ok()?;
    Some(tail)
}

fn codex_rollout_reports_usage_limit(
    rollout: Option<&std::path::Path>,
    cli: cas_mux::SupervisorCli,
) -> bool {
    matches!(
        codex_rollout_usage_limit_evidence(rollout, cli),
        UsageLimitEvidence::Limited { .. }
    )
}

fn codex_rollout_usage_limit_evidence(
    rollout: Option<&std::path::Path>,
    cli: cas_mux::SupervisorCli,
) -> UsageLimitEvidence {
    if cli != cas_mux::SupervisorCli::Codex {
        return UsageLimitEvidence::Recovered;
    }
    let Some(rollout) = rollout else {
        return UsageLimitEvidence::Unavailable;
    };
    let Ok(metadata) = std::fs::metadata(rollout) else {
        return UsageLimitEvidence::Unavailable;
    };
    const TAIL_BYTES: u64 = 256 * 1024;
    let start = metadata.len().saturating_sub(TAIL_BYTES);
    let Ok(mut file) = std::fs::File::open(rollout) else {
        return UsageLimitEvidence::Unavailable;
    };
    use std::io::{Read, Seek, SeekFrom};
    if file.seek(SeekFrom::Start(start)).is_err() {
        return UsageLimitEvidence::Unavailable;
    }
    let mut tail = String::new();
    if file.read_to_string(&mut tail).is_err() {
        return UsageLimitEvidence::Unavailable;
    }
    // Rollouts are append-only JSONL.  A limit line that is followed by a
    // successful terminal turn is historical rollout text, not a live outage.
    // Keep the record timestamp (or its byte offset as a legacy fallback) as
    // the durable episode identity used by daemon restarts and retry keys.
    let mut latest_terminal = None;
    for (line_index, line) in tail.lines().enumerate() {
        let limited = line.contains("You've hit your usage limit")
            && (line.contains("\"has_credits\":false") || line.contains("\"has_credits\": false"));
        let completed = line.contains("\"type\":\"task_complete\"")
            || line.contains("\"type\": \"task_complete\"");
        if limited {
            let timestamp = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|value| {
                    value
                        .get("timestamp")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| format!("tail-{}", start + line_index as u64));
            latest_terminal = Some(UsageLimitEvidence::Limited {
                first_evidence: timestamp,
            });
        } else if completed {
            latest_terminal = Some(UsageLimitEvidence::Recovered);
        }
    }
    latest_terminal.unwrap_or(UsageLimitEvidence::Recovered)
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
            Some(source) if source.eq_ignore_ascii_case("cli") => CodexRolloutKind::InteractiveCli,
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

/// Scan budget for `**/rollout-*.jsonl` under the sessions root. Codex hosts
/// accumulate hundreds of historical rollouts; matching by cwd only needs
/// recent ones, so we mtime-sort and cap the walk (cas-c655 / cas-900b cap
/// spirit).
const MAX_CODEX_ROLLOUT_SCAN: usize = 200;

const WORKER_TRANSCRIPT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);
const MAX_WORKER_TRANSCRIPT_CACHE_ENTRIES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WorkerTranscriptCacheKey {
    cli: &'static str,
    // cas-9e81: the full search-root set, not a single dir — Claude now
    // resolves across every known config dir's `projects` root.
    base_dirs: Vec<std::path::PathBuf>,
    clone_path: Option<String>,
    session_id: String,
}

#[derive(Debug, Clone)]
struct WorkerTranscriptCacheEntry {
    resolved_at: std::time::Instant,
    resolution: TranscriptResolution,
}

fn worker_transcript_cache() -> &'static std::sync::Mutex<
    std::collections::HashMap<WorkerTranscriptCacheKey, WorkerTranscriptCacheEntry>,
> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<WorkerTranscriptCacheKey, WorkerTranscriptCacheEntry>,
        >,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

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
    resolve_worker_transcript_path_for_account(clone_path, session_id, cli, None)
}

/// Account-aware form of [`resolve_worker_transcript_path`].  Callers that
/// have the agent row (is-wedged, debug, and worker_status) must use this so a
/// named Codex account resolves its own rollout tree.
pub(crate) fn resolve_worker_transcript_path_for_account(
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
    account_dir: Option<&str>,
) -> Option<std::path::PathBuf> {
    // cas-9e81: search every root this harness could have written under, not
    // just the default one. On a two-account factory the single-root lookup
    // returned None for every pane on the non-default `CLAUDE_CONFIG_DIR`, and
    // the daemon's wake gate treats an unresolvable transcript as evidence of
    // an in-flight tool call — a permanent, silent refusal to wake.
    transcript_path_from_resolution(resolve_transcript_in_roots(
        &transcript_roots_for_worker(cli, account_dir),
        clone_path,
        session_id,
        cli,
    ))
}

/// Resolve the activity/context path for `worker_status`.
///
/// Codex is the cas-fa69 fix: use the existing cli-aware resolver so a real
/// rollout is reachable. Grok is the cas-a9ea follow-up: its
/// directory-per-session layout must use the same harness-aware resolver as
/// `cas factory is-wedged`, otherwise this function stats a synthesized Claude
/// path and returns `None` while the wedged classifier finds `updates.jsonl`.
/// Claude alone retains the historical single-stat fast path.
fn worker_status_transcript_path(
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> Option<std::path::PathBuf> {
    worker_status_transcript_path_for_account(clone_path, session_id, cli, None)
}

fn worker_status_transcript_path_for_account(
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
    account_dir: Option<&str>,
) -> Option<std::path::PathBuf> {
    match cli {
        cas_mux::SupervisorCli::Codex | cas_mux::SupervisorCli::Grok => {
            let cached = worker_status_cached_transcript_resolution_for_account(
                clone_path,
                session_id,
                cli,
                account_dir,
            );
            worker_status_path_from_resolution(cached.resolution, cli)
        }
        cas_mux::SupervisorCli::Claude => transcript_path_fast(clone_path, session_id),
        // cas-7296 owns OpenCode worker-status session mapping.
        cas_mux::SupervisorCli::OpenCode => None,
    }
}

/// Only harnesses with a scan-based transcript resolver may use the shared
/// worker-status cache. In particular, a legacy Claude row must never be
/// treated as a Codex/Grok rollout merely because a same-name stale row used
/// a different harness.
fn worker_status_uses_scanned_transcript(cli: cas_mux::SupervisorCli) -> bool {
    matches!(
        cli,
        cas_mux::SupervisorCli::Codex | cas_mux::SupervisorCli::Grok
    )
}

#[derive(Debug, Clone)]
struct WorkerStatusTranscriptResolution {
    resolution: TranscriptResolution,
    base_dir_resolved: bool,
}

fn hard_dead_worker_transcript_block(
    cached: Option<&WorkerStatusTranscriptResolution>,
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> String {
    cached.map_or_else(
        || format_transcript_block(clone_path, session_id, cli),
        |cached| render_transcript_block(&cached.resolution, session_id, cached.base_dir_resolved),
    )
}

fn worker_status_cached_transcript_resolution_for_account(
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
    account_dir: Option<&str>,
) -> WorkerStatusTranscriptResolution {
    // cas-9e81: same multi-root search as `resolve_worker_transcript_path`, so
    // `worker_status` and the daemon's wake gate cannot disagree about whether
    // a worker has a readable transcript.
    let roots = transcript_roots_for_worker(cli, account_dir);
    WorkerStatusTranscriptResolution {
        resolution: worker_status_cached_transcript_resolution_in_roots(
            &roots, clone_path, session_id, cli,
        ),
        base_dir_resolved: !roots.is_empty(),
    }
}

fn worker_status_path_from_resolution(
    resolution: TranscriptResolution,
    cli: cas_mux::SupervisorCli,
) -> Option<std::path::PathBuf> {
    match cli {
        // Codex worker_status accepts only a resolved real rollout.
        // Synthesized paths are not evidence (cas-de95), and any residual
        // Ambiguous result remains unresolved rather than inventing an age.
        cas_mux::SupervisorCli::Codex => worker_status_codex_path_from_resolution(resolution),
        // Keep Grok aligned with is-wedged's historical ambiguity selection.
        cas_mux::SupervisorCli::Grok => transcript_path_from_resolution(resolution),
        cas_mux::SupervisorCli::Claude => transcript_path_from_resolution(resolution),
        // cas-7296 owns OpenCode transcript evidence.
        cas_mux::SupervisorCli::OpenCode => None,
    }
}

#[cfg(test)]
fn worker_status_codex_transcript_path_in(
    sessions_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    worker_status_codex_path_from_resolution(worker_status_cached_transcript_resolution_in(
        sessions_dir,
        clone_path,
        session_id,
        cas_mux::SupervisorCli::Codex,
    ))
}

/// Bounded-TTL transcript resolution shared by worker_status and
/// worker_activity for scan-based harnesses. Cache the rich resolution rather
/// than only its concrete path so hard-dead status can surface Synthesized and
/// Ambiguous salvage information without repeating the directory walk.
#[cfg(test)]
fn worker_status_cached_transcript_resolution_in(
    base_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> TranscriptResolution {
    let roots: Vec<std::path::PathBuf> = base_dir
        .map(std::path::Path::to_path_buf)
        .into_iter()
        .collect();
    worker_status_cached_transcript_resolution_in_roots(&roots, clone_path, session_id, cli)
}

/// Multi-root form of [`worker_status_cached_transcript_resolution_in`]
/// (cas-9e81). The cache key covers the whole root set, so a single-root and a
/// multi-root lookup for the same session can never alias.
fn worker_status_cached_transcript_resolution_in_roots(
    roots: &[std::path::PathBuf],
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> TranscriptResolution {
    let key = WorkerTranscriptCacheKey {
        cli: cli.backend().name(),
        base_dirs: roots.to_vec(),
        clone_path: clone_path.map(str::to_owned),
        session_id: session_id.to_owned(),
    };
    if let Ok(cache) = worker_transcript_cache().lock()
        && let Some(entry) = cache.get(&key)
        && entry.resolved_at.elapsed() < WORKER_TRANSCRIPT_CACHE_TTL
    {
        return entry.resolution.clone();
    }

    let resolution = resolve_transcript_in_roots(roots, clone_path, session_id, cli);
    if let Ok(mut cache) = worker_transcript_cache().lock() {
        cache.retain(|_, entry| entry.resolved_at.elapsed() < WORKER_TRANSCRIPT_CACHE_TTL);
        if cache.len() >= MAX_WORKER_TRANSCRIPT_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(
            key,
            WorkerTranscriptCacheEntry {
                resolved_at: std::time::Instant::now(),
                resolution: resolution.clone(),
            },
        );
    }
    resolution
}

/// Injectable half of [`resolve_worker_transcript_path`] for deterministic
/// path-resolution and latency tests.
#[cfg(test)]
fn resolve_worker_transcript_path_in(
    base_dir: Option<&std::path::Path>,
    clone_path: Option<&str>,
    session_id: &str,
    cli: cas_mux::SupervisorCli,
) -> Option<std::path::PathBuf> {
    transcript_path_from_resolution(resolve_transcript(base_dir, clone_path, session_id, cli))
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
    let roots = default_transcript_roots(cli);
    let resolution = resolve_transcript_in_roots(&roots, clone_path, session_id, cli);
    render_transcript_block(&resolution, session_id, !roots.is_empty())
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

/// A transcript's latest prompt occupancy and, where the harness reports it,
/// its model's actual context window. Never infer the latter from a model name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContextUsage {
    input_tokens: u64,
    model_context_window: Option<u64>,
}

/// Classify live prompt occupancy against the harness-reported context window.
fn context_band(input_tokens: u64, model_context_window: u64) -> &'static str {
    let pct = input_tokens.saturating_mul(100) / model_context_window;
    match pct {
        0..=49 => "ok",
        50..=79 => "approaching",
        _ => "near-limit",
    }
}

/// Render context state without guessing a model window. A raw input-token
/// count alone does not establish how close a worker is to compaction.
fn format_context_usage(usage: ContextUsage) -> String {
    let ktok = usage.input_tokens / 1_000;
    match usage.model_context_window {
        Some(window) if window > 0 => {
            let band = context_band(usage.input_tokens, window);
            let headroom = window
                .saturating_sub(usage.input_tokens)
                .saturating_mul(100)
                / window;
            format!(
                "\n    context: {band} (~{ktok}k / {}k tk; ~{headroom}% headroom)",
                window / 1_000,
            )
        }
        _ => format!("\n    context input: ~{ktok}k tk (model window unavailable)"),
    }
}

/// Render the short-lived state left by Claude's PreCompact hook.  This is
/// deliberately metadata-backed: a normal worker heartbeat continues during
/// compaction, so timestamp age is the only portable indication that the
/// worker is still heads-down rather than merely having an old checkpoint.
///
/// No equivalent automatic state is claimed for Codex/Grok because those
/// harnesses do not publish a PreCompact event to Cassy.
pub(crate) fn format_context_checkpoint_status(
    metadata: &std::collections::HashMap<String, String>,
) -> String {
    if metadata.get("context_checkpoint_state").map(String::as_str) != Some("compacting") {
        return String::new();
    }
    let fresh = metadata
        .get("context_checkpoint_at")
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|at| (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds() < 300)
        .unwrap_or(false);
    if !fresh {
        return String::new();
    }
    let task = metadata
        .get("context_checkpoint_task_id")
        .map(String::as_str)
        .unwrap_or("unknown task");
    let branch = metadata
        .get("context_checkpoint_branch")
        .map(String::as_str)
        .unwrap_or("unknown branch");
    format!("\n    state: compacting / heads-down — will resume {task} on {branch}")
}

/// Historical live-worker path lookup used by Claude reporting.
///
/// Reconstructs the Claude-layout path from `clone_path` + `session_id` and
/// checks it with one `stat(2)`. Grok cannot use this path because its
/// transcript is `~/.grok/sessions/<encoded-cwd>/<session>/updates.jsonl`.
fn transcript_path_fast(clone_path: Option<&str>, session_id: &str) -> Option<std::path::PathBuf> {
    // cas-9e81: `synthesized_transcript_path` hardcodes `~/.claude/projects`,
    // so this stat missed every session written under a non-default
    // `CLAUDE_CONFIG_DIR`. Stat the same slug under each known projects root.
    let home = dirs::home_dir();
    let clone = clone_path?;
    let slug = escaped_project_slug(clone);
    let file = format!("{session_id}.jsonl");
    default_claude_projects_dirs()
        .into_iter()
        .map(|projects| projects.join(&slug).join(&file))
        .find(|path| path.exists())
        .or_else(|| transcript_path_fast_in(home.as_deref()?, clone_path, session_id))
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

/// Harness-aware live-context reader. Claude has no reported window, so its
/// observed input is deliberately not classified as remaining capacity. Codex
/// reports both the latest prompt occupancy and its actual context window in
/// each rollout `token_count` event. Grok has no equivalent parser yet.
fn read_context_usage_from_tail_for_cli(
    path: &std::path::Path,
    cli: cas_mux::SupervisorCli,
) -> Option<ContextUsage> {
    if cli != cas_mux::SupervisorCli::Codex {
        return read_context_usage_from_tail(path).map(|input_tokens| ContextUsage {
            input_tokens,
            model_context_window: None,
        });
    }

    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    const TAIL_BYTES: u64 = 8192;
    file.seek(SeekFrom::Start(file_len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    // A bounded seek can land on one of the at most three continuation
    // bytes of a UTF-8 code point. Drop only that incomplete leading scalar;
    // invalid UTF-8 anywhere else still fails closed as a malformed rollout.
    let buf = (0..=3).find_map(|offset| {
        bytes
            .get(offset..)
            .and_then(|tail| std::str::from_utf8(tail).ok())
    })?;

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
        let input_tokens = value
            .pointer("/payload/info/last_token_usage/input_tokens")
            .and_then(|v| v.as_u64())?;
        return Some(ContextUsage {
            input_tokens,
            model_context_window: value
                .pointer("/payload/info/model_context_window")
                .and_then(|v| v.as_u64()),
        });
    }
    None
}

// =============================================================================
// B1 (cas-844bf): worker_status git introspection
// =============================================================================

/// Git state snapshot for a factory worker.
///
/// All fields are best-effort: a failed git sub-command yields a sentinel
/// value ("?" or "none" or 0) rather than aborting the status render.  The
/// PR lookup is deliberately tri-state so an unavailable lookup cannot be
/// mistaken for a successful query that found no PR.
/// See [`collect_worker_git_status`] for field semantics.
///
/// `pub(crate)` so the Stop hook (cas-5c0a) can reuse this struct without
/// creating a divergent duplicate.
#[derive(Debug)]
pub(crate) struct WorkerGitStatus {
    /// Current branch name (or "HEAD" if detached, "?" on error)
    pub branch: String,
    /// Full 40-char HEAD SHA (or "?" on error).
    ///
    /// cas-ea51: this was `git rev-parse --short HEAD`, whose width is git's
    /// *dynamic* abbreviation length — it grows with object count, so the live
    /// DB holds a mix (594 rows at 7 chars, 390 at 8 as measured for the
    /// cas-7ad6 spec). A consumer slicing `sha[0..8]` silently missed the
    /// 7-char rows. Storing the full SHA makes every new row an exact-match
    /// join key. Renderers truncate for display via `head_sha_for_display`;
    /// storage and the event metadata stay full-width.
    pub head_sha: String,
    /// Commits ahead of `base_branch` (0 when the count can't be determined)
    pub ahead: usize,
    /// Commits behind `base_branch` (0 when the count can't be determined)
    pub behind: usize,
    /// Branch used as the ahead/behind baseline (e.g. "origin/main")
    pub base_branch: String,
    /// `true` if the worker scope has changes: porcelain index state normally,
    /// or the committed contribution range while a merge is in progress.
    pub dirty: bool,
    /// `"origin/<branch>"` when the branch has been pushed, otherwise `"none"`
    pub pushed_ref: String,
    /// Result of the open pull-request lookup.
    pub pr_url: WorkerPrUrl,
    /// `true` when this path is the SHARED primary checkout (the main
    /// working tree) rather than a linked `git worktree`.
    ///
    /// cas-5bef (GH #120): a non-isolated worker branching in place re-points
    /// the HEAD that the supervisor's landing sequence runs against, so the
    /// distinction has to survive into the rendered status. Defaults to
    /// `false` (treated as a linked worktree) whenever git can't answer —
    /// a false alarm on every non-git dir would train the reader to ignore it.
    pub is_shared_checkout: bool,
}

/// Outcome of the best-effort open pull-request lookup used by
/// [`WorkerGitStatus`].
///
/// `None` is reserved for a successful `gh` query with empty output.  Every
/// other inability to answer is `Unknown` with a fixed, redacted reason; the
/// reason must never include command output, paths, or authentication data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkerPrUrl {
    /// `gh` returned a non-empty URL.
    Url(String),
    /// `gh` succeeded and reported no open PR.
    None,
    /// The lookup could not establish whether an open PR exists.
    Unknown(&'static str),
}

impl std::fmt::Display for WorkerPrUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(url) => f.write_str(url),
            Self::None => f.write_str("none"),
            Self::Unknown(reason) => write!(f, "unknown ({reason})"),
        }
    }
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
    collect_worker_git_status_with_gh(worktree_path, std::path::Path::new("gh"))
}

/// Collect git status while using the supplied `gh` executable.
///
/// The executable parameter keeps the production collector's subprocess
/// behavior intact while allowing tests to cover missing, failed, and empty
/// `gh` responses without changing the process-wide `PATH`.
fn collect_worker_git_status_with_gh(
    worktree_path: &std::path::Path,
    gh_program: &std::path::Path,
) -> WorkerGitStatus {
    // --- current branch -------------------------------------------------------
    let branch = run_git(worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_else(|_| "?".to_string());

    // --- HEAD SHA (full 40 chars, cas-ea51) -----------------------------------
    // Deliberately NOT `--short`: that returns git's dynamic abbreviation,
    // which varies by repo size and made stored SHAs unjoinable without a
    // variable-width prefix match. Display truncation happens at render.
    let head_sha =
        run_git(worktree_path, &["rev-parse", "HEAD"]).unwrap_or_else(|_| "?".to_string());

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

    // --- worker scope changes -------------------------------------------------
    // `MERGE_HEAD` stages clean incoming paths. `worker_scope_paths` switches
    // to merge-base..HEAD there so another lane's incoming files do not render
    // as this worker's dirty/drift signal (cas-d04f / cas-7a21).
    let dirty = worker_scope_paths(worktree_path)
        .map(|paths| !paths.is_empty())
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
    // Keep the pushed-ref short-circuit to avoid an unnecessary ~200ms query,
    // but report why it could not answer instead of claiming "none".
    let pr_url = collect_worker_pr_url(worktree_path, &branch, &pushed_ref, gh_program);

    // --- shared checkout or linked worktree? ----------------------------------
    // cas-5bef (GH #120). In a linked worktree `--git-dir` resolves to
    // `<repo>/.git/worktrees/<name>` while `--git-common-dir` stays at
    // `<repo>/.git`; in the primary checkout the two are the same path. Both
    // must answer for the claim to be made — an unreadable/non-git path stays
    // `false` so the loud warning below can only fire on a positive ID.
    let is_shared_checkout = match (
        run_git(worktree_path, &["rev-parse", "--git-dir"]),
        run_git(worktree_path, &["rev-parse", "--git-common-dir"]),
    ) {
        (Ok(git_dir), Ok(common_dir)) => git_dir == common_dir,
        _ => false,
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
        is_shared_checkout,
    }
}

/// Resolve an open PR for a branch without exposing subprocess details.
fn collect_worker_pr_url(
    worktree_path: &std::path::Path,
    branch: &str,
    pushed_ref: &str,
    gh_program: &std::path::Path,
) -> WorkerPrUrl {
    if branch == "?" {
        return WorkerPrUrl::Unknown("branch unknown");
    }
    if pushed_ref == "none" {
        return WorkerPrUrl::Unknown("branch not on origin locally");
    }

    let output = match std::process::Command::new(gh_program)
        .args([
            "pr", "list", "--head", branch, "--json", "url", "--jq", ".[0].url",
        ])
        .current_dir(worktree_path)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return WorkerPrUrl::Unknown("gh unavailable");
        }
        Err(_) => return WorkerPrUrl::Unknown("gh could not start"),
    };

    if !output.status.success() {
        return WorkerPrUrl::Unknown("gh lookup failed");
    }

    let stdout = match String::from_utf8(output.stdout) {
        Ok(stdout) => stdout,
        Err(_) => return WorkerPrUrl::Unknown("gh returned invalid output"),
    };
    let url = stdout.trim();
    if url.is_empty() {
        WorkerPrUrl::None
    } else {
        WorkerPrUrl::Url(url.to_string())
    }
}

/// Warning text for a SHARED primary checkout whose HEAD is parked on a
/// `factory/*` branch, or `None` when that is not the situation.
///
/// cas-5bef (GH #120): a non-isolated worker refused by the WORKER COMMIT
/// GUARD created `factory/bright-eagle-91` in the shared checkout and left it
/// checked out. The supervisor's landing sequence in that directory then
/// misfired silently — `git merge --ff-only <sha>` said "Already up to date"
/// (it merged into the factory branch), `git push origin main` pushed the
/// stale local trunk, and the release tag pointed at a commit unreachable from
/// `origin/main`. Every step "succeeded"; only the parked HEAD was wrong.
///
/// Pure so the wording is testable without a git fixture.
pub(crate) fn shared_checkout_parked_warning(
    is_shared_checkout: bool,
    branch: &str,
    base_branch: &str,
) -> Option<String> {
    let branch = branch.trim();
    if !is_shared_checkout || !branch.starts_with("factory/") {
        return None;
    }
    // "origin/main" → "main": the remedy is a local switch, not a remote ref.
    let trunk = base_branch
        .trim()
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("main");

    Some(format!(
        "\n    ⚠️ SHARED CHECKOUT PARKED ON '{branch}': this is the shared primary checkout, \
         not an isolated worktree, and its HEAD is '{branch}' instead of '{trunk}'. \
         Any `git merge --ff-only`, `git push origin {trunk}` or release tag run in this \
         directory targets '{branch}' and reports success while landing nothing on \
         '{trunk}' (GH #120). Remedy: once that work is pushed, restore trunk here with \
         `git switch {trunk}`, and give the worker `git worktree add` instead of a branch \
         created in place."
    ))
}

/// Truncate a stored full-width SHA to a human-readable prefix for display.
///
/// cas-ea51: `WorkerGitStatus.head_sha` is stored full-width (40 chars) so it
/// is an exact join key, but a 40-char SHA in a status line is noise. This
/// keeps the pre-existing 8-char visual width. Length-safe by construction, so
/// the `"?"` error sentinel passes through unchanged rather than panicking on a
/// slice out of bounds.
pub(crate) fn head_sha_for_display(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
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
    let pr_label = gs.pr_url.to_string(); // URL, "none", or "unknown (...)"

    // cas-ecf7 (GH #118): a worktree that is behind its base was the failure
    // mode that cost an epic three workers built on 25-commit-old history. The
    // count alone reads as background noise in a wall of status lines, so state
    // the consequence on its own line.
    let stale_label = if gs.behind > 0 {
        format!(
            "\n    ⚠️ STALE BASE: {} commit(s) behind {} — this worktree is missing that work; \
             rebase/sync before trusting builds or tests here.",
            gs.behind, gs.base_branch,
        )
    } else {
        String::new()
    };

    // cas-5bef (GH #120): a shared checkout parked on factory/* is rendered
    // with the same loudness as STALE BASE — it is the same class of failure
    // (the supervisor's next git command silently operates on the wrong thing).
    let parked_label =
        shared_checkout_parked_warning(gs.is_shared_checkout, &gs.branch, &gs.base_branch)
            .unwrap_or_default();

    format!(
        "\n    git: {} @ {} {} {}\
         \n    ahead: {} behind: {} (vs {}){}{}\
         \n    PR: {}",
        gs.branch,
        head_sha_for_display(&gs.head_sha),
        dirty_label,
        pushed_label,
        gs.ahead,
        gs.behind,
        gs.base_branch,
        stale_label,
        parked_label,
        pr_label,
    )
}

// =============================================================================

#[cfg(test)]
mod spawn_lifecycle_tests {
    use super::*;
    use cas_store::{SpawnLifecycle, SpawnLifecycleState};

    fn row(
        id: i64,
        worker: Option<&str>,
        state: SpawnLifecycleState,
        age_secs: i64,
        detail: Option<&str>,
    ) -> SpawnLifecycle {
        SpawnLifecycle {
            id,
            worker_name: worker.map(str::to_string),
            state,
            detail: detail.map(str::to_string),
            requested_names: Vec::new(),
            task_id: None,
            created_at: chrono::Utc::now() - chrono::Duration::seconds(age_secs),
            state_at: None,
        }
    }

    fn render(rows: &[SpawnLifecycle]) -> String {
        format_spawn_lifecycle_section(rows, chrono::Utc::now())
    }

    fn spec_for(cli: cas_mux::SupervisorCli, config_dir: Option<&str>) -> cas_mux::WorkerSpec {
        cas_mux::WorkerSpec {
            name: None,
            cli,
            model: None,
            effort: None,
            config_dir: config_dir.map(str::to_string),
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        }
    }

    fn evidence(availability: cas_factory::CapabilityAvailability, reason: &str) -> cas_factory::CapabilityEvidence {
        let mut evidence = cas_factory::CapabilityEvidence::new(availability, 0);
        evidence.reason = Some(reason.to_string());
        evidence
    }

    #[test]
    fn a_logged_out_account_refuses_the_spawn_by_name_before_any_worktree_exists() {
        let specs = vec![spec_for(cas_mux::SupervisorCli::Codex, Some("~/.codex-alt"))];
        let error = preflight_account_auth_with(&specs, |_, _| {
            evidence(
                cas_factory::CapabilityAvailability::Unavailable,
                "Codex login status reports no authenticated account",
            )
        })
        .expect_err("a logged-out account must refuse the spawn");
        assert!(error.contains("~/.codex-alt"), "{error}");
        assert!(error.contains("codex login"), "{error}");
        assert!(error.contains("No worktree was created"), "{error}");
        assert!(
            error.contains("no authenticated account"),
            "the probe's own reason must survive: {error}"
        );
    }

    #[test]
    fn a_refusal_names_the_default_account_when_the_caller_named_none() {
        let specs = vec![spec_for(cas_mux::SupervisorCli::Codex, None)];
        let error = preflight_account_auth_with(&specs, |_, _| {
            evidence(
                cas_factory::CapabilityAvailability::Unavailable,
                "logged out",
            )
        })
        .expect_err("refusal expected");
        assert!(error.contains("~/.codex"), "{error}");
    }

    #[test]
    fn an_unreadable_probe_never_blocks_a_spawn() {
        // No CLI on PATH, a probe that timed out, or a harness with no account
        // plumbing is absence of evidence. A preflight that failed closed on
        // its own unreliability would ground the factory over a slow binary.
        let specs = vec![spec_for(cas_mux::SupervisorCli::Codex, Some("~/.codex"))];
        assert!(
            preflight_account_auth_with(&specs, |_, _| {
                evidence(
                    cas_factory::CapabilityAvailability::Unknown,
                    "Codex login status probe timed out",
                )
            })
            .is_ok()
        );
        assert!(
            preflight_account_auth_with(&specs, |_, _| {
                evidence(cas_factory::CapabilityAvailability::Available, "logged in")
            })
            .is_ok()
        );
    }

    #[test]
    fn each_distinct_account_is_probed_once_and_harnesses_without_accounts_are_skipped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let specs = vec![
            spec_for(cas_mux::SupervisorCli::Codex, Some("~/.codex")),
            spec_for(cas_mux::SupervisorCli::Codex, Some("~/.codex")),
            spec_for(cas_mux::SupervisorCli::Codex, Some("~/.codex-alt")),
            spec_for(cas_mux::SupervisorCli::Claude, Some("~/.claude")),
            spec_for(cas_mux::SupervisorCli::Grok, None),
        ];
        preflight_account_auth_with(&specs, |cli, _| {
            assert_ne!(
                cli,
                cas_mux::SupervisorCli::Grok,
                "a harness with no account plumbing must not be probed"
            );
            calls.fetch_add(1, Ordering::SeqCst);
            evidence(cas_factory::CapabilityAvailability::Available, "logged in")
        })
        .expect("healthy accounts pass");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "four Codex/Claude specs across three distinct accounts must cost three probes"
        );
    }

    #[test]
    fn launched_pty_can_be_shutdown_by_name_or_spawn_request_id_before_registration() {
        let launched = vec![row(
            491,
            Some("kind-dragon-90"),
            SpawnLifecycleState::Launched,
            2,
            None,
        )];

        let by_name =
            select_launched_shutdown_targets(&launched, Some("kind-dragon-90"), &[], None);
        let by_request_id = select_launched_shutdown_targets(&launched, Some("491"), &[], None);

        assert_eq!(by_name[0].worker_name.as_deref(), Some("kind-dragon-90"));
        assert_eq!(by_request_id[0].id, 491);
    }

    fn lost_relay(task_summary: &str) -> cas_store::UndeliveredLifecycleRelay {
        cas_store::UndeliveredLifecycleRelay {
            prompt_id: 7783,
            source: "lifecycle-wake:3386".to_string(),
            target: "supervisor".to_string(),
            summary: Some(task_summary.to_string()),
            prompt: "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-fe23\">"
                .to_string(),
            stage: "abandoned".to_string(),
            reason: Some(cas_store::PendingReason::UndeliveredLifecycleRelay),
            detail: Some("task closed before delivery".to_string()),
            factory_session: Some("cas-src-fast-pelican-83".to_string()),
            created_at: chrono::Utc::now(),
            processed_at: Some(chrono::Utc::now()),
        }
    }

    /// cas-7787 (GH #160), acceptance criterion 3: a lost relay must be LOUD
    /// in `worker_status`, naming the task, and must never let the supervisor
    /// read the absence of a message as "nothing needs me".
    #[test]
    fn an_undelivered_relay_is_named_in_worker_status() {
        let out = format_undelivered_relay_section(&[lost_relay(
            "task_awaiting_merge: cas-fe23 (2026-08-07T18:51:51Z)",
        )]);
        assert!(
            out.contains("UNDELIVERED SUPERVISOR RELAY"),
            "the banner must state the failure outright: {out}"
        );
        assert!(out.contains("cas-fe23"), "must name the task: {out}");
        assert!(
            out.contains("message_ack notification_id=3386"),
            "the exact acknowledge action must be visible rather than making a reconciled relay replay forever: {out}"
        );
        assert!(
            out.contains("still be waiting on you"),
            "must say the lane may still need the supervisor: {out}"
        );
        assert!(
            out.contains("not evidence the work was handled"),
            "must refuse to let silence read as success: {out}"
        );
    }

    /// The banner must be absent when there is nothing lost — a heading that
    /// renders on every healthy poll is one people stop reading, and this one
    /// has to be believed the single time it fires.
    #[test]
    fn no_undelivered_relays_renders_nothing() {
        assert!(format_undelivered_relay_section(&[]).is_empty());
    }

    #[test]
    fn relay_banner_states_the_true_total_beyond_its_display_cap() {
        let rows = (0..12)
            .map(|index| {
                let mut row = lost_relay("task_blocked: cas-open (2026-08-09T15:57:00Z)");
                row.prompt_id += index;
                row
            })
            .collect::<Vec<_>>();
        let rendered = format_undelivered_relay_section(&rows);
        assert!(
            rendered.contains("12 total; displaying 10"),
            "the display cap must never conceal backlog depth: {rendered}"
        );
    }

    #[test]
    fn task_free_worker_death_is_not_an_undelivered_lane_warning() {
        let mut informational = lost_relay("worker died: steady-fox-24");
        informational.source = "lifecycle-wake:worker-died:2466".to_string();
        informational.prompt = crate::prompt_revalidation::format_worker_died_relay(
            "old-supervisor-row",
            "steady-fox-24",
            "worker_died:old-supervisor-row:1",
            "heartbeat stale",
            &[],
            &[],
            2466,
        );

        assert!(
            format_undelivered_relay_section(&[informational]).is_empty(),
            "a death envelope with no held or recovered tasks cannot represent a waiting lane"
        );
    }

    /// No spawn history → no section. `worker_status` must not grow a stub
    /// heading on every poll in a session that never spawned.
    #[test]
    fn empty_history_renders_nothing() {
        assert!(render(&[]).is_empty());
    }

    /// The core GH #60 requirement: a request that launched but never
    /// registered is reported as FAILED with its reason — not as silence.
    #[test]
    fn failed_spawn_is_named_with_its_reason() {
        let out = render(&[row(
            417,
            Some("quiet-lynx-3"),
            SpawnLifecycleState::Failed,
            200,
            Some("did not register with Cassy within 120 seconds"),
        )]);
        assert!(out.contains("request 417"), "{out}");
        assert!(out.contains("quiet-lynx-3"), "{out}");
        assert!(out.contains("FAILED"), "{out}");
        assert!(out.contains("did not register"), "{out}");
        assert!(
            out.contains("⚠"),
            "a failure must carry the warning line: {out}"
        );
    }

    /// A request still sitting in a non-terminal state past the threshold is
    /// UNCONFIRMED. This is the 2026-07-27 incident shape: success-shaped
    /// receipt, nothing consuming the queue, both daemon logs zero bytes.
    #[test]
    fn stale_queued_request_is_flagged_unconfirmed() {
        let out = render(&[row(43, None, SpawnLifecycleState::Queued, 600, None)]);
        assert!(out.contains("UNCONFIRMED"), "{out}");
        assert!(out.contains("treat it as not dispatched"), "{out}");
    }

    /// A spawn still provisioning inside the normal window is NOT flagged —
    /// the signal has to stay quiet during ordinary git worktree setup or
    /// supervisors will learn to ignore it.
    #[test]
    fn fresh_in_flight_request_is_not_flagged() {
        let out = render(&[row(
            44,
            Some("brave-otter-9"),
            SpawnLifecycleState::Provisioning,
            5,
            None,
        )]);
        assert!(out.contains("provisioning"), "{out}");
        assert!(!out.contains("UNCONFIRMED"), "{out}");
        assert!(!out.contains("⚠"), "{out}");
    }

    /// A registered spawn is never flagged, however old it is.
    #[test]
    fn registered_spawn_is_never_flagged() {
        let out = render(&[row(
            45,
            Some("steady-crane-1"),
            SpawnLifecycleState::Registered,
            1500,
            None,
        )]);
        assert!(out.contains("registered"), "{out}");
        assert!(!out.contains("UNCONFIRMED"), "{out}");
        assert!(!out.contains("⚠"), "{out}");
    }

    /// Each request renders its OWN worker. The live defect this fixes:
    /// four spawn-verified receipts attributed requests 414-417 to the wrong
    /// workers, and two batch receipts both claimed request 417.
    #[test]
    fn every_request_renders_its_own_worker_and_id() {
        let out = render(&[
            row(
                414,
                Some("worker-a"),
                SpawnLifecycleState::Registered,
                60,
                None,
            ),
            row(
                415,
                Some("worker-b"),
                SpawnLifecycleState::Registered,
                50,
                None,
            ),
            row(
                416,
                Some("worker-c"),
                SpawnLifecycleState::Registered,
                40,
                None,
            ),
            row(
                417,
                Some("worker-d"),
                SpawnLifecycleState::Registered,
                30,
                None,
            ),
        ]);
        for (id, worker) in [
            (414, "worker-a"),
            (415, "worker-b"),
            (416, "worker-c"),
            (417, "worker-d"),
        ] {
            let line = out
                .lines()
                .find(|l| l.contains(&format!("request {id}")))
                .unwrap_or_else(|| panic!("request {id} missing from:\n{out}"));
            assert!(
                line.contains(worker),
                "request {id} must name {worker}, got: {line}"
            );
        }
        // And no line may name a worker belonging to a different request.
        let line_414 = out.lines().find(|l| l.contains("request 414")).unwrap();
        assert!(
            !line_414.contains("worker-d"),
            "cross-attribution: {line_414}"
        );
    }

    /// Ancient history is dropped so the section stays scannable.
    #[test]
    fn requests_outside_the_window_are_dropped() {
        let out = render(&[row(
            1,
            Some("ancient-worker"),
            SpawnLifecycleState::Registered,
            SPAWN_HISTORY_WINDOW_SECS + 60,
            None,
        )]);
        assert!(out.is_empty(), "{out}");
    }

    /// A pre-assigned task travels with the request so the supervisor can see
    /// which dispatch died without opening the task store.
    #[test]
    fn preassigned_task_is_shown() {
        let mut r = row(
            418,
            Some("worker-e"),
            SpawnLifecycleState::Failed,
            200,
            None,
        );
        r.task_id = Some("cas-1234".to_string());
        let out = render(&[r]);
        assert!(out.contains("[task cas-1234]"), "{out}");
    }

    // ===== GH #67: the roster names the assignment =====

    /// An in-progress assignment is named on the worker's own row, so a
    /// supervisor never has to open the task store or the worktree to learn
    /// what a worker is doing.
    #[test]
    fn in_progress_task_is_named_on_the_row() {
        let out = format_assigned_task_info(
            Some(("cas-8b84", "Worker lifecycle observability")),
            None,
            None,
            false,
        );
        assert!(out.contains("cas-8b84"), "{out}");
        assert!(out.contains("in progress"), "{out}");
        assert!(out.contains("Worker lifecycle observability"), "{out}");
    }

    /// Assigned-but-not-started is a DIFFERENT state from in-progress: it is
    /// either the dispatch grace window or a worker that never picked the task
    /// up, and the supervisor must be able to tell those from an idle row.
    #[test]
    fn assigned_but_unstarted_is_distinguished_from_in_progress() {
        let out = format_assigned_task_info(
            None,
            Some(("cas-4242", "Fix the thing", false)),
            None,
            false,
        );
        assert!(out.contains("cas-4242"), "{out}");
        assert!(out.contains("assigned, not started"), "{out}");
        assert!(!out.contains("in progress"), "{out}");
    }

    #[test]
    fn rejected_review_reopen_is_waiting_on_supervisor() {
        let out =
            format_assigned_task_info(None, Some(("cas-56f8", "Needs rework", true)), None, false);
        assert!(out.contains("WAITING ON YOU"), "{out}");
        assert!(
            out.contains("resume the existing worker or replace it"),
            "{out}"
        );
    }

    /// In-progress wins when a worker somehow holds both — the started task is
    /// the one it is actually working.
    #[test]
    fn in_progress_takes_precedence_over_open_assignment() {
        let out = format_assigned_task_info(
            Some(("cas-1111", "Started work")),
            Some(("cas-2222", "Also assigned", false)),
            None,
            false,
        );
        assert!(out.contains("cas-1111"), "{out}");
        assert!(!out.contains("cas-2222"), "{out}");
    }

    /// A worker with nothing assigned says so explicitly. An absent line reads
    /// as missing data; "none assigned" is an answer.
    #[test]
    fn unassigned_worker_says_none_assigned() {
        let out = format_assigned_task_info(None, None, None, false);
        assert!(out.contains("none assigned"), "{out}");
    }

    /// Long titles are capped so one verbose task cannot wreck the roster.
    #[test]
    fn long_titles_are_truncated() {
        let long = "x".repeat(200);
        let out = format_assigned_task_info(Some(("cas-9999", &long)), None, None, false);
        assert!(out.contains('…'), "{out}");
        assert!(
            out.len() < 140,
            "row must stay scannable, got {} chars",
            out.len()
        );
    }

    /// GH #257: spawning is another planning write moment. Its ordinary queue
    /// receipt must carry a matching project memory, not require a separate
    /// pull/search after the worker request was already made.
    #[tokio::test]
    async fn spawn_response_surfaces_related_recall_for_active_epic_cas_0efb() {
        use cas_types::{Entry, Task, TaskType};

        let temp = tempfile::tempdir().expect("temp project");
        let core = CasCore::with_daemon(temp.path().to_path_buf(), None, None);
        let task_store = core.open_task_store().expect("open task store");
        task_store.init().expect("init task store");

        let mut epic = Task::new(
            "cas-spawn-recall".to_string(),
            "Deploy timeline work".to_string(),
        );
        epic.task_type = TaskType::Epic;
        epic.description = "Coordinate the timeline deployment milestone.".to_string();
        task_store.add(&epic).expect("add epic");
        core.open_search_index()
            .expect("open search index")
            .index_task(&epic)
            .expect("index epic");

        let mut memory = Entry::new(
            "m-spawn-recall".to_string(),
            "Timeline deployment already has a verified rollout plan.".to_string(),
        );
        memory.title = Some("Reuse timeline rollout plan".to_string());
        core.open_store()
            .expect("open memory store")
            .add(&memory)
            .expect("add memory");
        core.open_search_index()
            .expect("open search index")
            .index_entry(&memory)
            .expect("index memory");

        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        let request: FactoryRequest = serde_json::from_value(serde_json::json!({
            "action": "spawn_workers",
            "count": 1,
            "cli": "claude"
        }))
        .expect("valid spawn request");

        let response = service
            .factory_spawn_workers(request)
            .await
            .expect("queue worker spawn");
        let text = response
            .content
            .into_iter()
            .filter_map(|content| match content.raw {
                rmcp::model::RawContent::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Related prior context:"), "{text}");
        assert!(text.contains("m-spawn-recall"), "{text}");
        assert!(text.contains("Reuse timeline rollout plan"), "{text}");
        assert!(text.contains("NON-ISOLATED SHARED-CHECKOUT RISK"), "{text}");
        assert!(text.contains("foreign factory branch"), "{text}");
        assert!(text.contains("graft"), "{text}");
        assert!(text.contains("SKILL.md"), "{text}");
        assert!(text.contains("Prefer isolate=true"), "{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // cas-2c05: shutdown_workers target resolution.
    //
    // Reproduced by the cas-src supervisor against an idle worker with no
    // assigned task: shutdown by name AND by session id both returned
    // "Worker target(s) not found", and the same error printed that worker in
    // its known list. Three different notions of identity were in play — the
    // id selector matched name-or-id, the names selector matched name only,
    // and the message was built from a third filtered set.
    // -----------------------------------------------------------------

    fn worker_named(name: &str, id: &str) -> cas_types::Agent {
        let mut agent = cas_types::Agent::new(id.to_string(), name.to_string());
        agent.role = AgentRole::Worker;
        // The reporter's worker had just closed its last task: idle, no task
        // assigned. If identity resolution filtered on a non-idle state this
        // is the case that would fail.
        agent.status = cas_types::AgentStatus::Idle;
        agent
    }

    #[test]
    fn a_worker_answers_to_its_name_its_agent_id_and_its_session_id() {
        let mut worker = worker_named("quick-dolphin-15", "f9dc5562-6881-4050-a464-72b7ee16cfde");
        worker.cc_session_id = Some("cc-session-abc".to_string());

        assert!(worker_answers_to(&worker, "quick-dolphin-15"));
        assert!(worker_answers_to(
            &worker,
            "f9dc5562-6881-4050-a464-72b7ee16cfde"
        ));
        assert!(worker_answers_to(&worker, "cc-session-abc"));
        // Names are matched case-insensitively; whitespace from a split list
        // must not decide identity either.
        assert!(worker_answers_to(&worker, "QUICK-Dolphin-15"));
        assert!(worker_answers_to(&worker, "  quick-dolphin-15  "));
    }

    #[test]
    fn a_different_worker_is_still_rejected() {
        // The fix must not turn resolution into "matches anything".
        let worker = worker_named("quick-dolphin-15", "f9dc5562-6881-4050-a464-72b7ee16cfde");
        assert!(!worker_answers_to(&worker, "fast-lynx-22"));
        assert!(!worker_answers_to(&worker, "f9dc5562"));
        assert!(!worker_answers_to(&worker, ""));
        assert!(!worker_answers_to(&worker, "quick-dolphin-150"));
    }

    #[test]
    fn a_json_array_target_list_resolves_to_bare_identifiers() {
        // The parameter is a comma-separated string, but its plural name
        // invites a JSON array and that is exactly how it was called when this
        // was reported. Both shapes must land on the same identifiers.
        let names = parse_worker_name_filter(Some(&"[\"quick-dolphin-15\"]".to_string()));
        assert_eq!(
            names,
            std::collections::HashSet::from(["quick-dolphin-15".to_string()])
        );

        let multi = parse_worker_name_filter(Some(
            &"[\"quick-dolphin-15\", \"fast-lynx-22\"]".to_string(),
        ));
        assert_eq!(
            multi,
            std::collections::HashSet::from([
                "quick-dolphin-15".to_string(),
                "fast-lynx-22".to_string()
            ])
        );

        // The documented form keeps working unchanged.
        assert_eq!(
            parse_worker_name_filter(Some(&"quick-dolphin-15, fast-lynx-22".to_string())),
            std::collections::HashSet::from([
                "quick-dolphin-15".to_string(),
                "fast-lynx-22".to_string()
            ])
        );
        assert!(parse_worker_name_filter(None).is_empty());
        assert!(parse_worker_name_filter(Some(&"[]".to_string())).is_empty());
    }

    #[test]
    fn an_idle_worker_with_no_task_resolves_by_name_and_by_id() {
        // The reporter's exact state. Resolution must not depend on the worker
        // having an assigned task or being mid-work.
        let workers = vec![
            worker_named("quick-dolphin-15", "f9dc5562-6881-4050-a464-72b7ee16cfde"),
            worker_named("fast-lynx-22", "aaaaaaaa-0000-0000-0000-000000000000"),
        ];
        for target in [
            "quick-dolphin-15",
            "f9dc5562-6881-4050-a464-72b7ee16cfde",
            "[\"quick-dolphin-15\"]",
        ] {
            let parsed = parse_worker_name_filter(Some(&target.to_string()));
            let resolved: Vec<&cas_types::Agent> = parsed
                .iter()
                .filter_map(|name| workers.iter().find(|worker| worker_answers_to(worker, name)))
                .collect();
            assert_eq!(
                resolved.len(),
                1,
                "target {target} must resolve to exactly one worker"
            );
            assert_eq!(resolved[0].name, "quick-dolphin-15");
        }
    }

    #[test]
    fn an_unknown_target_resolves_to_nothing() {
        let workers = vec![worker_named(
            "quick-dolphin-15",
            "f9dc5562-6881-4050-a464-72b7ee16cfde",
        )];
        let parsed = parse_worker_name_filter(Some(&"no-such-worker-9".to_string()));
        assert!(
            parsed
                .iter()
                .all(|name| !workers.iter().any(|worker| worker_answers_to(worker, name))),
            "an unknown target must still fail to resolve"
        );
    }

    fn response_text(response: CallToolResult) -> String {
        response
            .content
            .into_iter()
            .filter_map(|content| match content.raw {
                rmcp::model::RawContent::Text(text) => Some(text.text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn restarted_supervisor_sees_rehomed_worker_on_status_and_activity_surfaces() {
        use cas_store::{AgentStore, EventStore, SqliteAgentStore, SqliteEventStore};
        use cas_types::{Agent, AgentRole, Event, EventEntityType, EventType};

        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CAS_FACTORY_SESSION", "factory-session-s2"),
            ("CAS_AGENT_ROLE", "supervisor"),
            ("CAS_AGENT_NAME", "stable-supervisor"),
        ]);
        let project = tempfile::tempdir().expect("temp project");
        let cas_root = crate::store::init_cas_dir(project.path()).expect("initialize CAS root");
        let agent_store = SqliteAgentStore::open(&cas_root).expect("open agent store");

        let mut supervisor_s1 = Agent::new(
            "supervisor-session-s1".to_string(),
            "stable-supervisor".to_string(),
        );
        supervisor_s1.role = AgentRole::Supervisor;
        supervisor_s1.factory_session = Some("factory-session-s1".to_string());
        agent_store.register(&supervisor_s1).expect("register S1 supervisor");

        let mut worker = Agent::new(
            "worker-session-w1".to_string(),
            "surviving-worker".to_string(),
        );
        worker.role = AgentRole::Worker;
        worker.factory_session = Some("factory-session-s1".to_string());
        agent_store.register(&worker).expect("register S1 worker");

        let mut supervisor_s2 = Agent::new(
            "supervisor-session-s2".to_string(),
            "stable-supervisor".to_string(),
        );
        supervisor_s2.role = AgentRole::Supervisor;
        supervisor_s2.factory_session = Some("factory-session-s2".to_string());
        agent_store.register(&supervisor_s2).expect("register S2 supervisor");

        let mut activity = Event::new(
            EventType::WorkerFileEdited,
            EventEntityType::Agent,
            worker.id.clone(),
            "edited src/lib.rs",
        );
        activity.session_id = Some(worker.id.clone());
        SqliteEventStore::open(&cas_root)
            .expect("open event store")
            .record(&activity)
            .expect("record worker activity");

        let core = CasCore::with_daemon(cas_root, None, None);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);

        let status_request: FactoryRequest = serde_json::from_value(serde_json::json!({
            "action": "worker_status"
        }))
        .expect("valid status request");
        let status = response_text(
            service
                .factory_worker_status(status_request)
                .await
                .expect("worker status"),
        );
        assert!(status.contains("surviving-worker"), "{status}");
        assert!(
            status.contains("re-homed from prior factory session factory-session-s1"),
            "{status}"
        );

        let activity_request: FactoryRequest = serde_json::from_value(serde_json::json!({
            "action": "worker_activity",
            "worker_names": "surviving-worker"
        }))
        .expect("valid activity request");
        let activity = response_text(
            service
                .factory_worker_activity(activity_request)
                .await
                .expect("worker activity"),
        );
        assert!(activity.contains("surviving-worker"), "{activity}");
        assert!(!activity.contains("No recent worker activity"), "{activity}");
    }

    fn valid_claude_config(dir: &std::path::Path) {
        std::fs::write(dir.join("settings.json"), "{}").unwrap();
        std::fs::write(dir.join(".credentials.json"), "credential").unwrap();
        std::fs::create_dir(dir.join("agents")).unwrap();
        std::fs::create_dir(dir.join("skills")).unwrap();
    }

    #[test]
    fn no_active_epic_guidance_points_to_cas_supervisor_planning_references() {
        let guidance = no_active_epic_guidance("no task_id was supplied.");
        assert!(
            guidance.contains("cas-supervisor skill's planning references"),
            "planning guidance must point at the shipped supervisor references: {guidance}"
        );
        assert!(
            !guidance.contains("epic-spec") && !guidance.contains("epic-breakdown"),
            "planning guidance must not advertise retired commands: {guidance}"
        );
    }

    #[test]
    fn config_dir_preflight_names_each_missing_requirement() {
        let temp = tempfile::tempdir().unwrap();
        let cases = [
            ("settings.json", "settings.json"),
            (".credentials.json", ".credentials.json"),
            ("agents", "agents"),
            ("skills", "skills"),
        ];
        for (missing, expected) in cases {
            let profile = temp.path().join(missing.replace('.', "_"));
            std::fs::create_dir(&profile).unwrap();
            valid_claude_config(&profile);
            let target = profile.join(missing);
            if target.is_dir() {
                std::fs::remove_dir(target).unwrap();
            } else {
                std::fs::remove_file(target).unwrap();
            }
            let error = preflight_claude_config_dir(profile.to_str().unwrap()).unwrap_err();
            assert!(error.contains(expected), "expected {expected} in {error}");
        }
    }

    /// GH #491: Codex's `CODEX_HOME` has `auth.json`, not Claude's
    /// `settings.json` / `.credentials.json` profile files. Keep this at the
    /// preflight boundary so a valid alternate Codex account cannot regress
    /// into the old Claude-shaped validation failure.
    #[test]
    fn codex_config_dir_preflight_accepts_codex_home_without_claude_profile_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("auth.json"), "{}").unwrap();
        assert!(!temp.path().join("settings.json").exists());
        assert!(!temp.path().join(".credentials.json").exists());
        preflight_codex_config_dir(temp.path().to_str().unwrap()).unwrap();
    }

    #[test]
    fn codex_usage_limit_rollout_overrides_a_healthy_heartbeat_claim() {
        let rollout = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            rollout.path(),
            r#"{"type":"event_msg","payload":{"type":"task_complete","error":"You've hit your usage limit","rate_limits":{"credits":{"has_credits":false}}}}"#,
        )
        .unwrap();
        assert!(codex_rollout_reports_usage_limit(
            Some(rollout.path()),
            cas_mux::SupervisorCli::Codex
        ));
        assert!(!codex_rollout_reports_usage_limit(
            Some(rollout.path()),
            cas_mux::SupervisorCli::Claude
        ));
    }

    #[test]
    fn codex_usage_limit_requires_the_latest_terminal_rollout_outcome() {
        let rollout = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            rollout.path(),
            concat!(
                r#"{"timestamp":"2026-08-20T12:00:00Z","type":"event_msg","payload":{"type":"task_complete","error":"You've hit your usage limit","rate_limits":{"credits":{"has_credits":false}}}}"#,
                "\n",
                r#"{"timestamp":"2026-08-20T12:01:00Z","type":"event_msg","payload":{"type":"task_complete"}}"#,
            ),
        )
        .unwrap();
        assert_eq!(
            codex_rollout_usage_limit_evidence(Some(rollout.path()), cas_mux::SupervisorCli::Codex),
            UsageLimitEvidence::Recovered,
            "a later successful terminal turn clears historical rollout text"
        );

        std::fs::write(
            rollout.path(),
            r#"{"timestamp":"2026-08-20T12:02:00Z","type":"event_msg","payload":{"type":"task_complete","error":"You've hit your usage limit","rate_limits":{"credits":{"has_credits":false}}}}"#,
        )
        .unwrap();
        assert_eq!(
            codex_rollout_usage_limit_evidence(Some(rollout.path()), cas_mux::SupervisorCli::Codex),
            UsageLimitEvidence::Limited {
                first_evidence: "2026-08-20T12:02:00Z".to_string(),
            }
        );
    }

    /// cas-4a5e: a typo'd/missing codex config_dir must fail with a message
    /// naming the exact `auth.json` path that was checked — not a generic
    /// error and not the Claude-shaped wording.
    #[test]
    fn codex_config_dir_preflight_names_the_checked_auth_json_path() {
        let temp = tempfile::tempdir().unwrap();
        let missing_dir = temp.path().join("no-such-account");
        let error = preflight_codex_config_dir(missing_dir.to_str().unwrap()).unwrap_err();
        assert!(
            error.contains(&missing_dir.join("auth.json").display().to_string()),
            "error must name the checked auth.json path — got: {error}"
        );
        assert!(
            !error.contains("settings.json") && !error.contains(".credentials.json"),
            "codex preflight must not use claude-shaped wording — got: {error}"
        );
    }

    /// A codex config_dir whose `auth.json` is actually a directory (not a
    /// file) must fail the same way as missing — plain `File::open` would
    /// succeed on a directory on Linux, so the preflight must check
    /// `is_file()` explicitly.
    #[test]
    fn codex_config_dir_preflight_fails_when_auth_json_is_a_directory() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("auth.json")).unwrap();
        let error = preflight_codex_config_dir(temp.path().to_str().unwrap()).unwrap_err();
        assert!(error.contains("auth.json"), "{error}");
    }

    /// An explicit codex config_dir does NOT nest a `.codex/` subdirectory —
    /// it IS the CODEX_HOME. A `.codex/auth.json` nested one level down must
    /// not satisfy the preflight (that would be checking the wrong layout).
    #[test]
    fn codex_config_dir_preflight_rejects_nested_dot_codex_layout() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join(".codex");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("auth.json"), "{}").unwrap();
        assert!(preflight_codex_config_dir(temp.path().to_str().unwrap()).is_err());
    }

    #[test]
    fn worker_status_dedupe_prefers_freshest_heartbeat_across_factory_sessions() {
        let mut parent = cas_types::Agent::new("parent-session".into(), "worker-one".into());
        parent.role = cas_types::AgentRole::Worker;
        parent.factory_session = Some("factory-a".into());
        parent.registered_at = chrono::Utc::now() - chrono::Duration::minutes(2);
        parent.last_heartbeat = chrono::Utc::now() - chrono::Duration::minutes(1);
        parent.metadata.insert("worker_cli".into(), "codex".into());

        let mut nested = parent.clone();
        nested.id = "nested-knowledge-session".into();
        nested.cc_session_id = Some("nested-transcript".into());
        nested.factory_session = None;
        nested.registered_at = chrono::Utc::now();
        nested.last_heartbeat = chrono::Utc::now();
        nested.metadata.insert("worker_cli".into(), "claude".into());

        let mut other = cas_types::Agent::new("other-session".into(), "worker-two".into());
        other.role = cas_types::AgentRole::Worker;
        other.factory_session = Some("factory-a".into());

        let (rows, removed) = dedupe_authoritative_agents(vec![nested, other, parent]);
        assert_eq!(removed, 1);
        assert_eq!(rows.len(), 2);
        let worker_one = rows.iter().find(|row| row.name == "worker-one").unwrap();
        assert_eq!(worker_one.id, "nested-knowledge-session");
        assert_eq!(
            worker_cli_from_agent(worker_one),
            cas_mux::SupervisorCli::Claude,
            "the fresh Claude registration must not inherit the stale Codex row's resolver"
        );
    }

    #[test]
    fn worker_status_names_the_prior_session_for_a_rehomed_worker() {
        let mut worker = cas_types::Agent::new("worker-session".into(), "wild-cobra-45".into());
        worker.metadata.insert(
            "factory_session_rehomed_from".into(),
            "gabber-studio-brave-merlin-89".into(),
        );

        let rendered = factory_rehome_label(&worker);
        assert!(rendered.contains("re-homed"), "{rendered}");
        assert!(
            rendered.contains("gabber-studio-brave-merlin-89"),
            "{rendered}"
        );
    }

    #[test]
    fn worker_status_transcript_scan_is_guarded_by_registered_harness() {
        assert!(!worker_status_uses_scanned_transcript(
            cas_mux::SupervisorCli::Claude
        ));
        assert!(worker_status_uses_scanned_transcript(
            cas_mux::SupervisorCli::Codex
        ));
        assert!(worker_status_uses_scanned_transcript(
            cas_mux::SupervisorCli::Grok
        ));
    }

    #[cfg(target_os = "linux")]
    struct SyntheticProcessGroup {
        child: std::process::Child,
        pgid: u32,
    }

    #[cfg(target_os = "linux")]
    impl Drop for SyntheticProcessGroup {
        fn drop(&mut self) {
            // SAFETY: the test child starts a dedicated session/process group.
            unsafe { libc::killpg(self.pgid as libc::pid_t, libc::SIGKILL) };
            let _ = self.child.wait();
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn orphan_gc_skips_stale_heartbeat_worker_with_live_registered_process() {
        use std::os::unix::process::CommandExt;

        let temp = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 300 & wait"]);
        // SAFETY: isolate this synthetic lane from cargo's process group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        let pgid = child.id();
        let mut group = SyntheticProcessGroup { child, pgid };
        let session = format!("dead-test-session-{}", uuid::Uuid::new_v4());
        let record = crate::ui::factory::process_groups::track(
            temp.path(),
            "synthetic-live-worker",
            &session,
            pgid,
        )
        .unwrap();

        let mut agent = cas_types::Agent::new(
            "synthetic-live-agent".into(),
            "synthetic-live-worker".into(),
        );
        agent.role = cas_types::AgentRole::Worker;
        agent.status = cas_types::AgentStatus::Stale;
        agent.last_heartbeat = chrono::Utc::now() - chrono::Duration::minutes(10);
        agent.pid = Some(pgid);
        crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, pgid);
        // Legacy/malformed registrations may lack this field; liveness must
        // still protect the matching named lane.
        agent.factory_session = None;

        let live_workers = live_factory_workers_from_agents([agent]);
        let (orphans, skipped_live, _, unverifiable) =
            orphan_process_groups(temp.path(), 0, &live_workers);
        assert!(
            orphans.is_empty(),
            "live owner must never enter the reap set"
        );
        assert_eq!(skipped_live, vec![record.clone()]);
        assert!(unverifiable.is_empty());
        assert!(
            crate::mcp::daemon::pid_alive(pgid),
            "GC classification must not signal a stale-heartbeat live owner"
        );

        assert_eq!(
            crate::ui::factory::process_groups::reap(temp.path(), &record)
                .await
                .unwrap(),
            crate::ui::factory::process_groups::ReapOutcome::Reaped
        );
        let _ = group.child.wait();
    }

    /// cas-b7dd / GH #88 at the GC surface: a live worker's own process group
    /// must never be reported as an orphan process, and a planted orphan in
    /// the same worktree tree must be.
    ///
    /// This is the pairing that matters. A GC that finds orphans but also
    /// "finds" live workers is not a GC, it is an outage generator, so both
    /// halves are asserted against the same scan.
    #[cfg(target_os = "linux")]
    #[test]
    fn orphan_process_scan_separates_live_worker_groups_from_real_orphans() {
        use std::os::unix::process::CommandExt;

        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().to_path_buf();
        let worktree = cas_root.join("worktrees/worker-a");
        std::fs::create_dir_all(&worktree).unwrap();

        // A live worker: its own session/process group, tracked, still running.
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 120 & wait"]);
        command.current_dir(&worktree);
        // SAFETY: isolate this synthetic lane from cargo's process group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        let live_pgid = child.id();
        // RAII: `SyntheticProcessGroup::drop` killpg's the group and only then
        // waits. Do NOT call `child.wait()` directly while it is still alive —
        // that blocks for the full sleep and turns a fast test into a
        // two-minute one.
        let _group = SyntheticProcessGroup {
            child,
            pgid: live_pgid,
        };
        crate::ui::factory::process_groups::track(&cas_root, "worker-a", "live-session", live_pgid)
            .unwrap();

        // A genuine orphan in the same worktree: launcher exits, child adopted.
        let planted = std::process::Command::new("sh")
            .arg("-c")
            .arg("sleep 120 >/dev/null 2>&1 </dev/null & echo $!")
            .current_dir(&worktree)
            .output()
            .unwrap();
        let orphan_pid: u32 = String::from_utf8_lossy(&planted.stdout)
            .trim()
            .parse()
            .unwrap();
        // Let the launcher exit so the child is adopted.
        for _ in 0..200 {
            if !crate::mcp::daemon::pid_alive(orphan_pid) {
                break;
            }
            let adopted = crate::ui::factory::orphan_gc::parent_state(
                std::fs::read_to_string(format!("/proc/{orphan_pid}/stat"))
                    .ok()
                    .and_then(|stat| {
                        stat.rsplit_once(')')?
                            .1
                            .split_whitespace()
                            .nth(1)?
                            .parse::<u32>()
                            .ok()
                    })
                    .unwrap_or(1),
            );
            if adopted != crate::ui::factory::orphan_gc::ParentState::Alive {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }

        let live_workers = live_factory_workers_from_agents([]);
        let report = scan_orphan_processes(&cas_root, &live_workers);

        let orphan = report
            .processes
            .iter()
            .find(|p| p.pid == orphan_pid)
            .expect("planted orphan must be reported");
        assert!(
            orphan.disposition.is_reapable(),
            "planted orphan should be reapable, was {:?}",
            orphan.disposition
        );

        // The live worker's group leader sits in the same worktree; it must be
        // spared (reported as owned, never reapable).
        let live_entry = report.processes.iter().find(|p| p.pid == live_pgid);
        assert!(
            live_entry.is_none_or(|p| !p.disposition.is_reapable()),
            "a tracked live worker process must never be reapable: {live_entry:?}"
        );

        // Preview must kill nothing at all.
        let preview = crate::ui::factory::orphan_gc::cleanup(&cas_root, &report, false);
        assert!(preview.killed.is_empty());
        assert!(crate::mcp::daemon::pid_alive(orphan_pid));

        let done = crate::ui::factory::orphan_gc::cleanup(&cas_root, &report, true);
        assert!(
            done.killed.contains(&orphan_pid),
            "authorized cleanup must reap the orphan; errors {:?}",
            done.errors
        );
        assert!(
            !done.killed.contains(&live_pgid),
            "and must never reap the live worker"
        );
        assert!(crate::mcp::daemon::pid_alive(live_pgid));

        // SAFETY: test cleanup for a pid this test planted.
        unsafe { libc::kill(orphan_pid as libc::pid_t, libc::SIGKILL) };
    }

    use crate::test_support::TestEnvGuard;
    use cas_types::AgentRole;

    /// Session id used across the glob tests. Stable UUID shape, unique
    /// across fake projects so the glob doesn't collide with anything else
    /// the test happens to create.
    const TEST_SESSION: &str = "cas-900b-test-0000-0000-000000000000";

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
        let _home = TestEnvGuard::temp_home();
        let json = build_spawn_spec_json(Some("claude"), None, None).unwrap();
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(spec.model.as_deref(), Some("opus"));
        assert_eq!(spec.effort, Some(cas_mux::Effort::High));
    }

    #[test]
    fn spawn_specs_keep_per_worker_harness_and_account_overrides() {
        let specs = build_spawn_specs_with_project_config(
            2,
            Some("claude"),
            None,
            Some("high"),
            Some("/accounts/batch"),
            Some(
                r#"[
                    {"name":"codex-research","cli":"codex","model":"gpt-5.6","config_dir":"/accounts/codex"},
                    {"name":"claude-review","config_dir":"/accounts/claude"}
                ]"#,
            ),
            None,
        )
        .expect("per-worker specs resolve");

        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name.as_deref(), Some("codex-research"));
        assert_eq!(specs[0].cli, cas_mux::SupervisorCli::Codex);
        assert_eq!(specs[0].config_dir.as_deref(), Some("/accounts/codex"));
        assert_eq!(specs[1].name.as_deref(), Some("claude-review"));
        assert_eq!(specs[1].cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(specs[1].config_dir.as_deref(), Some("/accounts/claude"));
        assert_eq!(specs[1].effort, Some(cas_mux::Effort::High));
    }

    #[test]
    fn spawn_specs_reject_more_worker_entries_than_spawn_slots() {
        let err = build_spawn_specs_with_project_config(
            1,
            None,
            None,
            None,
            None,
            Some(r#"[{"cli":"claude"},{"cli":"codex"}]"#),
            None,
        )
        .expect_err("excess worker entries must not be silently ignored");
        assert!(err.contains("only 1 worker slot"), "{err}");
    }

    #[test]
    fn spawn_spec_explicit_model_and_effort_are_unchanged() {
        let _home = TestEnvGuard::temp_home();
        let json = build_spawn_spec_json(Some("claude"), Some("opus"), Some("high")).unwrap();
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(spec.model.as_deref(), Some("opus"));
        assert_eq!(spec.effort, Some(cas_mux::Effort::High));
    }

    #[test]
    fn lane_spawn_specs_use_registry_recipe_and_preserve_slot_metadata() {
        let snapshot = cas_factory::CapabilitySnapshot::default();
        let specs = build_lane_spawn_specs(
            2,
            "light",
            Some("~/.claude-alt"),
            Some(r#"[{"name":"research"},{"name":"review"}]"#),
            &snapshot,
        )
        .expect("lane should resolve");

        assert_eq!(specs.0.len(), 2);
        assert_eq!(specs.1, "claude_haiku");
        assert_eq!(specs.0[0].name.as_deref(), Some("research"));
        assert_eq!(specs.0[1].name.as_deref(), Some("review"));
        assert_eq!(specs.0[0].config_dir.as_deref(), Some("~/.claude-alt"));
        assert_eq!(
            specs.0[0].model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );
        assert_eq!(specs.0[0].effort, Some(cas_mux::Effort::Low));
        assert!(specs.2.is_empty(), "static primary should not warn");
    }

    #[test]
    fn taste_lane_spawn_specs_and_explicit_fable_route_agree() {
        let _home = TestEnvGuard::temp_home();
        let (specs, recipe, warnings) = build_lane_spawn_specs(
            2,
            "taste",
            Some("~/.claude-alt"),
            Some(r#"[{"name":"taste-a"},{"name":"taste-b"}]"#),
            &cas_factory::CapabilitySnapshot::default(),
        )
        .unwrap();
        assert_eq!(recipe, "claude_fable");
        assert!(warnings.is_empty());
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name.as_deref(), Some("taste-a"));
        assert_eq!(specs[1].name.as_deref(), Some("taste-b"));
        for spec in specs {
            assert_eq!(spec.cli, cas_mux::SupervisorCli::Claude);
            assert_eq!(spec.model.as_deref(), Some("claude-fable-5-1"));
            assert_eq!(spec.effort, Some(cas_mux::Effort::Medium));
            assert_eq!(spec.config_dir.as_deref(), Some("~/.claude-alt"));
        }
        assert_eq!(
            cli_for_model_slug("claude-fable-5-1"),
            Some(cas_mux::SupervisorCli::Claude)
        );
        for cli in [None, Some("claude")] {
            for effort in ["medium", "high"] {
                let json =
                    build_spawn_spec_json(cli, Some("claude-fable-5-1"), Some(effort)).unwrap();
                let spec = decoded_spawn_spec(&json);
                assert_eq!(spec.cli, cas_mux::SupervisorCli::Claude);
                assert_eq!(spec.model.as_deref(), Some("claude-fable-5-1"));
                assert_eq!(
                    spec.effort,
                    Some(effort.parse::<cas_mux::Effort>().unwrap())
                );
            }
        }
        for cli in ["codex", "grok", "opencode"] {
            let error =
                build_spawn_spec_json(Some(cli), Some("claude-fable-5-1"), Some("medium"))
                    .unwrap_err();
            assert!(error.contains("cli=claude"), "{error}");
        }
        let error =
            build_spawn_spec_json(Some("claude"), Some("claude-fable-5-1"), Some("ultra"))
                .unwrap_err();
        assert!(error.contains("effort"), "{error}");
    }

    #[test]
    fn lane_spawn_specs_name_fallback_in_the_spawn_receipt_warning() {
        let registry = cas_factory::embedded_registry().unwrap();
        let now = cas_factory::CapabilitySnapshot::now_ms();
        let mut snapshot = cas_factory::CapabilitySnapshot::default();
        snapshot.record(
            cas_factory::recipe_route_identity(
                &registry.recipes["claude_fable"],
                "default",
            ),
            cas_factory::CapabilityEvidence::new(
                cas_factory::CapabilityAvailability::Unavailable,
                now,
            )
            .with_reason("Claude Fable account unavailable"),
        );
        snapshot.record(
            cas_factory::recipe_route_identity(
                &registry.recipes["claude_opus"],
                "default",
            ),
            cas_factory::CapabilityEvidence::new(
                cas_factory::CapabilityAvailability::Available,
                now,
            ),
        );

        let (specs, recipe, warnings) = build_lane_spawn_specs(
            1,
            "taste",
            None,
            None,
            &snapshot,
        )
        .expect("taste fallback should resolve");
        assert_eq!(recipe, "claude_opus");
        assert_eq!(specs[0].model.as_deref(), Some("claude-opus-5"));
        assert_eq!(specs[0].effort, Some(cas_mux::Effort::High));
        assert_eq!(
            warnings,
            ["fallback: claude_opus (primary claude_fable unavailable: Claude Fable account unavailable)"],
        );
    }

    #[test]
    fn lane_spawn_specs_reject_explicit_worker_recipe_fields() {
        let snapshot = cas_factory::CapabilitySnapshot::default();
        let error = build_lane_spawn_specs(
            1,
            "standard",
            None,
            Some(r#"[{"model":"gpt-5.6-sol"}]"#),
            &snapshot,
        )
        .expect_err("lane must not accept a partial explicit recipe");

        assert!(error.contains("lane=\"standard\""), "{error}");
        assert!(error.contains("model"), "{error}");
        assert!(error.contains("choose lane="), "{error}");
    }

    #[test]
    fn lane_spawn_specs_reject_empty_or_zero_slot_requests() {
        let snapshot = cas_factory::CapabilitySnapshot::default();
        let error = build_lane_spawn_specs(0, "light", None, None, &snapshot)
            .expect_err("lane must name a real worker slot");
        assert!(error.contains("at least one worker slot"), "{error}");
    }

    // -----------------------------------------------------------------------
    // cas-28a4 / GH #71: an invalid cli+model pairing must never reach the
    // spawn queue. The live report: `model=claude-opus-4-5` with no `cli=`
    // resolved to the stock Codex default and spawned two Codex workers
    // carrying a Claude slug, with no error.
    // -----------------------------------------------------------------------

    #[test]
    fn spawn_spec_rejects_claude_model_on_explicit_codex_cli() {
        let _home = TestEnvGuard::temp_home();
        let err = build_spawn_spec_json(Some("codex"), Some("claude-opus-4-5"), None)
            .expect_err("codex + claude slug must be rejected at enqueue");

        assert!(err.contains("claude-opus-4-5"), "{err}");
        assert!(err.contains("codex"), "names the requested cli: {err}");
        assert!(
            err.contains("cli=claude"),
            "error must state the actionable fix: {err}"
        );
    }

    #[test]
    fn spawn_spec_rejects_codex_model_on_explicit_claude_cli() {
        let _home = TestEnvGuard::temp_home();
        let err = build_spawn_spec_json(Some("claude"), Some("gpt-5.6-luna"), None)
            .expect_err("claude + codex slug must be rejected at enqueue");

        assert!(err.contains("gpt-5.6-luna"), "{err}");
        assert!(err.contains("cli=codex"), "{err}");
    }

    #[test]
    fn spawn_spec_rejects_suspended_terra_model() {
        let _home = TestEnvGuard::temp_home();
        let err = build_spawn_spec_json(Some("codex"), Some("gpt-5.6-terra"), None)
            .expect_err("suspended Terra must not reach the spawn queue");

        assert!(err.contains("gpt-5.6-terra"), "{err}");
        assert!(err.contains("suspended"), "{err}");
        assert!(err.contains("operator decision pending"), "{err}");
        assert!(err.contains("gpt-5.6-luna"), "{err}");
        assert!(err.contains("routing rule 'suspended recipe'"), "{err}");
        assert!(err.contains("codex_luna"), "{err}");
    }

    #[test]
    fn spawn_spec_rejects_luna_below_maximum_effort() {
        let _home = TestEnvGuard::temp_home();
        let err = build_spawn_spec_json(Some("codex"), Some("gpt-5.6-luna"), Some("high"))
            .expect_err("Luna must not be spawned below xhigh");

        assert!(err.contains("gpt-5.6-luna"), "{err}");
        assert!(err.contains("effort=xhigh"), "{err}");
        assert!(err.contains("routing rule 'allowed effort'"), "{err}");
        assert!(err.contains("codex_luna"), "{err}");
    }

    /// The live #71 case: no `cli=` at all. The model slug is unambiguous, so
    /// it — not the stock Codex default — decides the harness.
    #[test]
    fn spawn_spec_omitted_cli_follows_an_explicit_claude_model() {
        let _home = TestEnvGuard::temp_home();
        let json = build_spawn_spec_json(None, Some("claude-opus-5"), None).unwrap();
        let spec = decoded_spawn_spec(&json);

        assert_eq!(
            spec.cli,
            cas_mux::SupervisorCli::Claude,
            "an explicit claude model must not be spawned on codex"
        );
        assert_eq!(spec.model.as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn spawn_spec_omitted_cli_follows_an_explicit_grok_model() {
        let _home = TestEnvGuard::temp_home();
        let json = build_spawn_spec_json(None, Some("grok-4.5"), None).unwrap();
        let spec = decoded_spawn_spec(&json);
        assert_eq!(spec.cli, cas_mux::SupervisorCli::Grok);
    }

    #[test]
    fn opencode_spawn_preserves_full_provider_model_selector_and_omits_effort() {
        let _env = TestEnvGuard::with_optional_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, None)]);
        let json = build_spawn_spec_json(Some("opencode"), Some("local/qwen3.8"), None)
            .expect("OpenCode selector should resolve without a hard-coded model default");
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::OpenCode);
        assert_eq!(spec.model.as_deref(), Some("local/qwen3.8"));
        assert_eq!(spec.effort, None);
    }

    #[test]
    fn opencode_model_selector_infers_opencode_when_cli_is_omitted() {
        let _env = TestEnvGuard::with_optional_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, None)]);
        let json = build_spawn_spec_json(None, Some("local/qwen3.8"), None)
            .expect("provider/model selector should identify OpenCode");
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::OpenCode);
        assert_eq!(spec.model.as_deref(), Some("local/qwen3.8"));
        assert_eq!(spec.effort, None);
    }

    #[test]
    fn opencode_defaults_to_the_operator_token_plan_lane() {
        let _env = TestEnvGuard::with_optional_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, None)]);
        let json = build_spawn_spec_json(Some("opencode"), None, None)
            .expect("OpenCode should use the operator's explicit Token Plan default");
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::OpenCode);
        assert_eq!(spec.model.as_deref(), Some("qwencloud/qwen3.8-max"));
        assert_eq!(spec.effort, None);
    }

    #[test]
    fn opencode_rejects_effort_outside_endpoint_accepted_set_without_remapping() {
        let _env = TestEnvGuard::with_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, "low,medium,xhigh")]);
        let err = build_spawn_spec_json(Some("opencode"), Some("local/qwen3.8"), Some("high"))
            .expect_err("unsupported local endpoint effort must fail before spawn");

        assert!(err.contains("local/qwen3.8"), "{err}");
        assert!(err.contains("effort high"), "{err}");
        assert!(
            err.contains("endpoint accepted efforts: [low, medium, xhigh]"),
            "{err}"
        );
        assert!(err.contains("No effort remapping is performed"), "{err}");
    }

    #[test]
    fn opencode_hosted_route_preserves_selector_and_uses_hosted_efforts() {
        let _env = TestEnvGuard::with_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, "minimal,high")]);
        for effort in [None, Some("low"), Some("medium"), Some("xhigh")] {
            let json = build_spawn_spec_json(Some("opencode"), Some("alibaba/qwen3.8-max"), effort)
                .unwrap_or_else(|error| panic!("hosted effort {effort:?} should resolve: {error}"));
            let spec = decoded_spawn_spec(&json);
            assert_eq!(spec.cli, cas_mux::SupervisorCli::OpenCode);
            assert_eq!(spec.model.as_deref(), Some("alibaba/qwen3.8-max"));
            assert_eq!(
                spec.effort.map(|value| value.to_string()),
                effort.map(str::to_string)
            );
        }
    }

    #[test]
    fn opencode_token_plan_route_is_explicit_and_separate_from_payg() {
        let _env = TestEnvGuard::with_optional_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, None)]);
        for selector in ["qwencloud/qwen3.8-max", "hosted-token-plan/qwen3.8-max"] {
            let json = build_spawn_spec_json(Some("opencode"), Some(selector), Some("medium"))
                .unwrap_or_else(|error| panic!("Token Plan selector should resolve: {error}"));
            let spec = decoded_spawn_spec(&json);
            assert_eq!(spec.model.as_deref(), Some(selector));
            assert_eq!(spec.effort, Some(cas_mux::Effort::Medium));
        }
        let json = build_spawn_spec_json(
            Some("opencode"),
            Some("alibaba/qwen3.8-max"),
            Some("medium"),
        )
        .expect("pay-as-you-go selector should remain available");
        assert_eq!(
            decoded_spawn_spec(&json).model.as_deref(),
            Some("alibaba/qwen3.8-max")
        );
    }

    #[test]
    fn opencode_registered_recipe_runs_registry_policy_before_route_preflight() {
        let _env = TestEnvGuard::with_optional_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, None)]);
        let error = build_spawn_spec_json(
            Some("opencode"),
            Some("qwencloud/qwen3.8-max"),
            Some("high"),
        )
        .expect_err("the registered Token Plan recipe must reject high before route preflight");

        assert!(
            error.contains("recipe \"qwencloud_qwen\" rejects effort high"),
            "{error}"
        );
        assert!(
            error.contains("routing rule 'recipe allowed efforts'"),
            "{error}"
        );
        assert!(
            !error.contains("accepted hosted efforts"),
            "route-specific validation ran before registry policy: {error}"
        );
    }

    #[test]
    fn opencode_support_claim_gate_refuses_unreceipted_routes_before_queueing() {
        let _env = TestEnvGuard::with_optional_vars(&[
            (OPENCODE_ACCEPTED_EFFORTS_ENV, None),
            (crate::opencode_preflight::DASHSCOPE_API_KEY_ENV, None),
        ]);
        for selector in ["local/qwen3.8", "alibaba/qwen3.8-max"] {
            let json = build_spawn_spec_json(Some("opencode"), Some(selector), None)
                .expect("selector remains syntactically valid");
            let spec = decoded_spawn_spec(&json);
            let error = preflight_hosted_opencode_specs(&[spec])
                .expect_err("unreceipted route must fail before queue insertion");
            assert!(error.contains("pending-conformance"), "{error}");
            assert!(error.contains("was not queued"), "{error}");
        }
    }

    #[test]
    fn opencode_hosted_route_rejects_openai_effort_remaps() {
        let _env = TestEnvGuard::with_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, "minimal,high")]);
        for effort in ["minimal", "high"] {
            let error =
                build_spawn_spec_json(Some("opencode"), Some("alibaba/qwen3.8-max"), Some(effort))
                    .expect_err("hosted Qwen must reject OpenAI compatibility remaps");
            assert!(error.contains("accepted hosted efforts: [low, medium, xhigh]"));
            assert!(error.contains("No effort remapping"));
        }
    }

    #[test]
    fn opencode_hosted_route_requires_supported_explicit_provider() {
        let _env = TestEnvGuard::with_optional_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, None)]);
        for selector in ["qwen3.8-max", "cloud/qwen3.8-max", "alibaba/other-model"] {
            let error = build_spawn_spec_json(Some("opencode"), Some(selector), None)
                .expect_err("unsupported OpenCode route must fail before spawn");
            assert!(
                error.contains("provider/model")
                    || error.contains("supported route")
                    || error.contains("currently supports model"),
                "{error}"
            );
        }
    }

    #[test]
    fn opencode_accepts_operator_configured_effort_set() {
        let _env = TestEnvGuard::with_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, "minimal,high")]);
        let json = build_spawn_spec_json(Some("opencode"), Some("local/qwen3.8"), Some("high"))
            .expect("local endpoint effort set should be configurable");
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::OpenCode);
        assert_eq!(spec.model.as_deref(), Some("local/qwen3.8"));
        assert_eq!(spec.effort, Some(cas_mux::Effort::High));
    }

    #[test]
    fn opencode_accepts_project_configured_effort_set() {
        let _env = TestEnvGuard::with_optional_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, None)]);
        let tmp = tempfile::tempdir().expect("temp project config");
        let config = tmp.path().join("config.toml");
        std::fs::write(
            &config,
            r#"
[factory.defaults]
opencode_accepted_efforts = ["minimal", "high"]
"#,
        )
        .unwrap();

        let json = build_spawn_spec_json_with_project_config(
            Some("opencode"),
            Some("local/qwen3.8"),
            Some("high"),
            Some(config),
        )
        .expect("project endpoint effort set should be configurable");
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::OpenCode);
        assert_eq!(spec.model.as_deref(), Some("local/qwen3.8"));
        assert_eq!(spec.effort, Some(cas_mux::Effort::High));
    }

    #[test]
    fn opencode_factory_default_model_is_config_driven() {
        let _env = TestEnvGuard::with_optional_vars(&[(OPENCODE_ACCEPTED_EFFORTS_ENV, None)]);
        let tmp = tempfile::tempdir().expect("temp project config");
        let config = tmp.path().join("config.toml");
        std::fs::write(
            &config,
            r#"
[factory.defaults]
cli = "opencode"
model = "local/qwen3.8"
"#,
        )
        .unwrap();

        let json = build_spawn_spec_json_with_project_config(None, None, None, Some(config))
            .expect("OpenCode defaults should come from factory config");
        let spec = decoded_spawn_spec(&json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::OpenCode);
        assert_eq!(spec.model.as_deref(), Some("local/qwen3.8"));
        assert_eq!(spec.effort, None);
    }

    /// Matching pairs and unrecognized slugs must stay untouched — validation
    /// rejects known-bad combinations, it does not police the model catalog.
    #[test]
    fn spawn_spec_accepts_matching_and_unknown_model_slugs() {
        let _home = TestEnvGuard::temp_home();
        for (cli, model) in [
            ("claude", "claude-opus-5"),
            ("claude", "opus"),
            ("codex", "gpt-5.6-luna"),
            ("grok", "grok-4.5"),
            ("codex", "some-unreleased-slug"),
        ] {
            let json = build_spawn_spec_json(Some(cli), Some(model), None)
                .unwrap_or_else(|e| panic!("cli={cli} model={model} must be accepted: {e}"));
            let spec = decoded_spawn_spec(&json);
            assert_eq!(spec.model.as_deref(), Some(model));
        }
    }

    #[test]
    fn model_slug_families_are_classified_conservatively() {
        use cas_mux::SupervisorCli;
        assert_eq!(
            cli_for_model_slug("claude-opus-5"),
            Some(SupervisorCli::Claude)
        );
        assert_eq!(cli_for_model_slug("opus"), Some(SupervisorCli::Claude));
        assert_eq!(cli_for_model_slug("SONNET"), Some(SupervisorCli::Claude));
        assert_eq!(
            cli_for_model_slug("gpt-5.6-sol"),
            Some(SupervisorCli::Codex)
        );
        assert_eq!(
            cli_for_model_slug("gpt-5.6-terra"),
            Some(SupervisorCli::Codex)
        );
        assert_eq!(
            cli_for_model_slug("local/qwen3.8"),
            Some(SupervisorCli::OpenCode)
        );
        assert_eq!(cli_for_model_slug("grok-4.5"), Some(SupervisorCli::Grok));
        assert_eq!(
            cli_for_model_slug("mystery-model-9"),
            None,
            "unknown slugs must not be guessed into a harness"
        );
    }

    #[test]
    fn spawn_spec_omitted_cli_without_config_uses_stock_codex_defaults() {
        let _home = TestEnvGuard::temp_home();
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
    fn spawn_policy_fallback_models_are_allowed_by_shipped_routing_doc() {
        // Isolate HOME so no user or project-level worker override can mask
        // the stock fallback this regression is intended to guard.
        let _home = TestEnvGuard::temp_home();
        let routing_doc =
            include_str!("../../../builtins/skills/cas-supervisor/references/model-selection.md");

        for (cli, spec) in [
            (
                cas_mux::SupervisorCli::Codex,
                decoded_spawn_spec(&build_spawn_spec_json(None, None, None).unwrap()),
            ),
            (
                cas_mux::SupervisorCli::Claude,
                decoded_spawn_spec(&build_spawn_spec_json(Some("claude"), None, None).unwrap()),
            ),
        ] {
            assert_eq!(spec.cli, cli);
            let model = spec.model.as_deref().expect("fallback model");
            let allowed_route = format!("cli={} model={model}", cli.backend().name());
            assert!(
                routing_doc
                    .lines()
                    .any(|line| line.contains(&allowed_route)),
                "runtime fallback {allowed_route} must be an allowed route in model-selection.md"
            );
        }
    }

    #[test]
    fn omitted_fields_warning_names_resolved_policy_default_spec() {
        let _home = TestEnvGuard::temp_home();
        let json = build_spawn_spec_json(None, None, None).unwrap();
        let warning = spawn_spec_warning(false, false, &json);

        assert!(
            warning.contains("policy default codex/gpt-5.6-luna/xhigh"),
            "{warning}"
        );
        assert!(
            warning.contains("pass model=/effort= explicitly to tier the spawn"),
            "{warning}"
        );
    }

    #[test]
    fn omitted_fields_warning_survives_multi_worker_specs() {
        let _home = TestEnvGuard::temp_home();
        let specs = [
            decoded_spawn_spec(&build_spawn_spec_json(None, None, None).unwrap()),
            decoded_spawn_spec(&build_spawn_spec_json(Some("claude"), None, None).unwrap()),
        ];

        let warning = spawn_specs_warning(false, false, &specs);

        assert!(
            warning.contains("policy default codex/gpt-5.6-luna/xhigh"),
            "{warning}"
        );
        assert!(
            warning.contains("policy default claude/opus/high"),
            "{warning}"
        );
    }

    #[test]
    fn lane_recipe_does_not_emit_omitted_model_or_effort_warning() {
        let _home = TestEnvGuard::temp_home();
        let json =
            build_spawn_spec_json(Some("claude"), Some("claude-opus-5"), Some("high")).unwrap();
        let spec = decoded_spawn_spec(&json);

        let warning = spawn_warning_for_request(true, false, false, true, &json, &[spec]);

        assert!(warning.is_empty(), "lane recipes are complete: {warning}");
    }

    #[test]
    fn spawn_spec_omitted_cli_respects_project_factory_default() {
        let _home = TestEnvGuard::temp_home();
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
        let warning = spawn_spec_warning(false, false, &json);

        assert_eq!(spec.cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(spec.model.as_deref(), Some("sonnet"));
        assert_eq!(spec.effort, Some(cas_mux::Effort::High));
        assert!(
            warning.contains("configured fallback claude/sonnet/high"),
            "{warning}"
        );
    }

    #[test]
    fn spawn_spec_project_defaults_can_force_frontier_and_warning_nags() {
        let _home = TestEnvGuard::temp_home();
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

    /// Run the real unavailable-Codex probe in a dedicated process. PATH is
    /// process-global, so changing it inside this parallel lib-test process
    /// can make unrelated subprocess spawns intermittently fail with ENOENT.
    /// Supplying PATH/HOME on this one child command preserves the end-to-end
    /// probe coverage without exposing the temporary environment to peers.
    #[test]
    fn resolved_codex_spec_falls_back_to_claude_when_codex_unavailable() {
        const CHILD_TEST: &str = "mcp::tools::service::factory_ops::tests::\
            resolved_codex_spec_falls_back_to_claude_when_codex_unavailable_in_isolated_child";

        let home = tempfile::TempDir::new().expect("isolated child HOME");
        let empty_path = home.path().join("empty-path");
        std::fs::create_dir(&empty_path).expect("empty PATH directory");

        let output = std::process::Command::new(
            std::env::current_exe().expect("current lib-test executable"),
        )
        .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
        .env("CAS_CODEX_FALLBACK_ISOLATED_CHILD", "1")
        .env("HOME", home.path())
        .env("PATH", &empty_path)
        .output()
        .expect("spawn isolated unavailable-Codex child test");

        assert!(
            output.status.success(),
            "isolated unavailable-Codex child failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(CHILD_TEST) && stdout.contains("test result: ok"),
            "isolated helper did not execute the real probe assertions:\n{stdout}"
        );
    }

    #[test]
    #[ignore = "subprocess helper for the isolated unavailable-Codex probe"]
    fn resolved_codex_spec_falls_back_to_claude_when_codex_unavailable_in_isolated_child() {
        assert_eq!(
            std::env::var("CAS_CODEX_FALLBACK_ISOLATED_CHILD").as_deref(),
            Ok("1"),
            "helper must run only through its isolated parent"
        );
        assert!(
            !cas_factory::probe::codex_binary_present(),
            "controlled child PATH must make the real Codex binary probe fail"
        );

        let home = std::path::PathBuf::from(
            std::env::var_os("HOME").expect("isolated child HOME must be set"),
        );
        for auth_present in [false, true] {
            let auth_path = home.join(".codex/auth.json");
            if auth_present {
                std::fs::create_dir_all(auth_path.parent().unwrap()).unwrap();
                std::fs::write(&auth_path, "{}").unwrap();
            } else if auth_path.exists() {
                std::fs::remove_file(&auth_path).unwrap();
            }

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

            assert_eq!(
                spec.cli,
                cas_mux::SupervisorCli::Claude,
                "controlled missing binary must fall back with auth_present={auth_present}"
            );
            assert_eq!(spec.model.as_deref(), Some(claude_default_model));
            assert_eq!(notices.len(), 1);
            assert!(
                notices[0].starts_with(
                    "worker slot 1: codex unavailable (codex binary not found on PATH)"
                ),
                "real probe must report the controlled missing binary — got: {}",
                notices[0]
            );
            assert!(
                !notices[0].contains("worker worker"),
                "unnamed spawn specs must never repeat the role as the identifier"
            );
        }
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

    #[test]
    fn worker_status_names_artifact_backed_codex_turn_start() {
        let temp = tempfile::tempdir().unwrap();
        let rollout = temp.path().join("rollout.jsonl");
        std::fs::write(
            &rollout,
            concat!(
                "{\"timestamp\":\"2026-07-31T20:01:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"worker-status-turn\"}}\n",
                "{\"timestamp\":\"2026-07-31T20:01:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\"}}\n"
            ),
        )
        .unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-31T20:01:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rendered =
            format_harness_turn_observation_at(cas_mux::SupervisorCli::Codex, Some(&rollout), now);
        assert!(rendered.contains("harness turn: started 3s ago"));
        assert!(rendered.contains("reaction observed"));
        assert!(rendered.contains("artifact-backed"));
        assert!(rendered.contains("task_started"));
    }

    #[test]
    fn worker_status_reports_artifact_backed_claude_turn_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let transcript = temp.path().join("claude.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                "{\"timestamp\":\"2026-07-31T20:01:02Z\",\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"do the work\"}}\n",
                "{\"timestamp\":\"2026-07-31T20:01:03Z\",\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n",
                "{\"timestamp\":\"2026-07-31T20:01:04Z\",\"type\":\"system\",\"subtype\":\"turn_duration\"}\n"
            ),
        )
        .unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-31T20:01:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rendered = format_harness_turn_observation_at(
            cas_mux::SupervisorCli::Claude,
            Some(&transcript),
            now,
        );
        assert!(
            rendered.contains("harness turn: started 3s ago"),
            "{rendered}"
        );
        assert!(rendered.contains("reaction observed"), "{rendered}");
        assert!(rendered.contains("completion observed"), "{rendered}");
        assert!(rendered.contains("top-level textual user"), "{rendered}");
    }

    #[test]
    fn claude_turn_watermark_participates_in_stall_classification() {
        let temp = tempfile::tempdir().unwrap();
        let transcript = temp.path().join("claude.jsonl");
        let now = chrono::Utc::now();
        let turn_started = now - chrono::Duration::seconds(200);
        std::fs::write(
            &transcript,
            format!(
                "{{\"timestamp\":\"{}\",\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"resume\"}}}}\n",
                turn_started.to_rfc3339()
            ),
        )
        .unwrap();
        filetime::set_file_mtime(
            &transcript,
            filetime::FileTime::from_system_time(
                std::time::SystemTime::now() - std::time::Duration::from_secs(900),
            ),
        )
        .unwrap();

        let activity = last_worker_activity_secs_with_harness_turn(
            &[],
            "claude-worker",
            cas_mux::SupervisorCli::Claude,
            Some(&transcript),
            now,
        )
        .expect("parsed turn watermark");
        assert_eq!(activity.1, "turn start");
        assert!(activity.0 >= 199 && activity.0 <= 201, "{activity:?}");
        assert!(
            !is_worker_stalled(true, Some(activity.0), 300, false),
            "the 200s Claude turn start is fresher than the 900s file mtime"
        );
        assert!(harness_publishes_turn_start(cas_mux::SupervisorCli::Claude));
    }

    #[test]
    fn worker_status_progress_timestamps_name_outbound_and_file_write() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-12T20:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rendered = format_worker_progress_timestamps(
            Some(now - chrono::Duration::seconds(12)),
            Some(now - chrono::Duration::seconds(34)),
            now,
        );
        assert!(rendered.contains("last outbound message: 2026-08-12T19:59:48+00:00 (12s ago)"));
        assert!(rendered.contains("last file write: 2026-08-12T19:59:26+00:00 (34s ago)"));
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
    fn test_78bf_assigned_unstarted_elapsed_crosses_threshold_and_reports_elapsed() {
        let now = chrono::Utc::now();
        assert_eq!(
            assigned_unstarted_elapsed_secs(
                now - chrono::Duration::seconds(310),
                None,
                300,
                false,
                now,
            ),
            Some(310)
        );
        assert_eq!(
            assigned_unstarted_elapsed_secs(
                now - chrono::Duration::seconds(299),
                None,
                300,
                false,
                now,
            ),
            None,
            "the existing just-assigned grace window must remain unchanged"
        );
    }

    #[test]
    fn test_78bf_assigned_unstarted_elapsed_resets_for_activity_and_in_flight_work() {
        let now = chrono::Utc::now();
        let assigned_at = now - chrono::Duration::seconds(600);
        assert_eq!(
            assigned_unstarted_elapsed_secs(assigned_at, Some(10), 300, false, now),
            None,
            "recent harness-aware activity must suppress escalation"
        );
        assert_eq!(
            assigned_unstarted_elapsed_secs(assigned_at, Some(600), 300, true, now),
            None,
            "an in-flight tool call is direct evidence that the worker is busy"
        );
    }

    #[test]
    fn test_78bf_worker_status_names_assigned_unstarted_state_and_elapsed() {
        let rendered = format_assigned_unstarted_status("cas-unstarted", 310, 300);
        assert!(rendered.contains("ASSIGNED BUT UNSTARTED"));
        assert!(rendered.contains("cas-unstarted was assigned 310s ago"));
        assert!(rendered.contains("remains unstarted with no recent activity"));
        assert!(rendered.contains("threshold: 300s"));
        assert!(rendered.contains("possible missed wake"));
    }

    #[test]
    fn test_c14e4_worker_status_stall_wins_over_assigned_unstarted_banner() {
        let rendered = format_priority_worker_status_alert(
            true,
            Some((310, "activity")),
            300,
            Some(("cas-unstarted", 600, 300)),
            // Turn-observable harness: the STALLED verdict is unchanged there
            // (cas-e728 only replaces it for turn-unobservable workers).
            true,
            0,
            inbox(0, None),
        )
        .expect("coexisting stalled and assigned-unstarted states must render an alert");

        assert!(rendered.contains("⚠ STALLED"), "{rendered}");
        assert!(!rendered.contains("ASSIGNED BUT UNSTARTED"), "{rendered}");
    }

    // --- cas-e728 (GH #105): STALLED is only honest when silence is evidence.

    /// An inbox with nothing but plain unread mail — no reminder in play.
    fn inbox(unread: usize, oldest_unread_secs: Option<i64>) -> WorkerInbox {
        WorkerInbox {
            unread,
            oldest_unread_secs,
            reminder_wait: None,
        }
    }

    /// A Claude worker publishes no turn-start artifact, so quiet is the
    /// NORMAL state between turns. While it is still heartbeating, the row
    /// must state that instead of accusing it of stalling.
    #[test]
    fn turn_unobservable_heartbeating_worker_reports_between_turns_not_stalled() {
        let rendered = format_priority_worker_status_alert(
            true,
            Some((900, "checkpoint")),
            300,
            None,
            false,
            0,
            inbox(0, None),
        )
        .expect("a stalled-by-threshold worker must still render a line");

        assert!(!rendered.contains("STALLED"), "{rendered}");
        assert!(
            rendered.contains("between turns since 900s ago"),
            "{rendered}"
        );
        assert!(rendered.contains("inbox empty"), "{rendered}");
    }

    /// cas-e728 review follow-up: the heartbeat is stamped by the DAEMON from
    /// process liveness (`mcp/daemon.rs` heartbeats while the harness PID is
    /// alive), not by turn execution — so a Claude worker that wedges with its
    /// process alive keeps heartbeating forever. Narrowing STALLED on the
    /// heartbeat alone would have silenced the only manual-poll alarm for
    /// exactly that case and replaced it with reassurance. Mail that was handed
    /// over and never consumed is the evidence that survives.
    #[test]
    fn unconsumed_mail_past_the_threshold_escalates_on_a_turn_unobservable_worker() {
        let rendered = format_priority_worker_status_alert(
            true,
            Some((900, "checkpoint")),
            300,
            None,
            false,
            0, // heartbeat fresh — the wedged-but-alive shape
            inbox(2, Some(900)),
        )
        .expect("alert");

        assert!(rendered.contains("⚠ NOT WAKING"), "{rendered}");
        assert!(
            rendered.contains("2 messages unread for 900s"),
            "{rendered}"
        );
        assert!(
            rendered.contains("wedged harness"),
            "must say what to check: {rendered}"
        );
        assert!(!rendered.contains("between turns"), "{rendered}");
    }

    /// cas-bcf5 (GH #162): stale inbox residue does not outweigh a worker's
    /// own recent-activity line. This was the live contradiction: NOT WAKING
    /// rendered alongside "last activity 348s ago" because the unread-age
    /// branch was independently sufficient.
    #[test]
    fn recent_activity_suppresses_not_waking_even_with_old_unread_mail() {
        let rendered = format_priority_worker_status_alert(
            true,
            Some((348, "activity")),
            600,
            None,
            false,
            0,
            inbox(3, Some(2_884)),
        )
        .expect("a stalled-by-threshold worker must still render a status line");

        assert!(!rendered.contains("NOT WAKING"), "{rendered}");
        assert!(
            rendered.contains("between turns since 348s ago"),
            "{rendered}"
        );
        assert!(rendered.contains("3 unread messages waiting"), "{rendered}");
    }

    /// cas-bcf5 (GH #162): both halves of the evidence remain actionable when
    /// no activity has been observed at all and unread work is old.
    #[test]
    fn silent_worker_with_old_unread_mail_still_flags_not_waking() {
        let rendered = format_priority_worker_status_alert(
            true,
            None,
            300,
            None,
            false,
            0,
            inbox(1, Some(900)),
        )
        .expect("genuinely silent worker with unread work must alert");

        assert!(rendered.contains("⚠ NOT WAKING"), "{rendered}");
        assert!(rendered.contains("1 message unread for 900s"), "{rendered}");
    }

    /// Mail that arrived a moment ago is not evidence of anything — the worker
    /// may simply not have been given its turn yet.
    #[test]
    fn fresh_mail_does_not_escalate() {
        let rendered = format_priority_worker_status_alert(
            true,
            Some((900, "checkpoint")),
            300,
            None,
            false,
            0,
            inbox(1, Some(5)),
        )
        .expect("alert");

        assert!(!rendered.contains("NOT WAKING"), "{rendered}");
        assert!(rendered.contains("between turns"), "{rendered}");
        assert!(rendered.contains("1 unread message waiting"), "{rendered}");
    }

    /// cas-e728 review follow-up: the between-turns branch must not swallow the
    /// assigned-but-unstarted alarm. That alarm is about an untouched
    /// assignment, not about silence, so narrowing the stall flag has no
    /// bearing on it.
    #[test]
    fn between_turns_does_not_swallow_the_assigned_unstarted_alarm() {
        let rendered = format_priority_worker_status_alert(
            true,
            Some((900, "checkpoint")),
            300,
            Some(("cas-unstarted", 600, 300)),
            false,
            0,
            inbox(0, None),
        )
        .expect("alert");

        assert!(rendered.contains("ASSIGNED BUT UNSTARTED"), "{rendered}");
        assert!(rendered.contains("cas-unstarted"), "{rendered}");
    }

    /// cas-e728 review follow-up: the row must not assert something Cassy cannot
    /// know. The same row says `harness turn: unobserved`; claiming "no turn is
    /// in flight" beside it would send a supervisor to reset a worker that is
    /// twenty minutes into a build.
    #[test]
    fn between_turns_line_does_not_claim_to_know_no_turn_is_running() {
        let rendered = format_between_turns_status(Some((600, "activity")), 0);
        assert!(
            !rendered.contains("no turn is in flight"),
            "must not assert unobservable state: {rendered}"
        );
        assert!(
            rendered.contains("cannot see Claude turn boundaries"),
            "must say what is actually known: {rendered}"
        );
    }

    /// cas-e728 review follow-up: Blocked is neither in progress nor
    /// parked-for-the-supervisor, but it must still be named — otherwise a
    /// blocked worker renders as idle-with-nothing-to-do, the same hole this
    /// change closes for awaiting-merge.
    #[test]
    fn blocked_task_is_named_not_rendered_as_idle() {
        let out = format_assigned_task_info(
            None,
            None,
            Some(("cas-block", "Stuck work", cas_types::TaskStatus::Blocked)),
            false,
        );
        assert!(out.contains("cas-block"), "{out}");
        assert!(out.contains("blocked"), "{out}");
        assert!(out.contains("clear the blocker"), "{out}");
        assert!(!out.contains("none assigned"), "{out}");
    }

    /// The actionable half: quiet WITH undelivered mail means the worker was
    /// handed work and has not woken.
    #[test]
    fn between_turns_line_carries_the_unread_inbox_count() {
        let none = format_between_turns_status(Some((600, "activity")), 0);
        assert!(none.contains("inbox empty"), "{none}");
        let one = format_between_turns_status(Some((600, "activity")), 1);
        assert!(one.contains("1 unread message waiting"), "{one}");
        let many = format_between_turns_status(Some((600, "activity")), 3);
        assert!(many.contains("3 unread messages waiting"), "{many}");
    }

    /// The genuine stall survives: a worker whose HEARTBEAT lapsed really has
    /// stopped, whatever its harness, so it keeps the alarm.
    #[test]
    fn lapsed_heartbeat_still_flags_stalled_on_a_turn_unobservable_harness() {
        let rendered = format_priority_worker_status_alert(
            true,
            Some((900, "checkpoint")),
            300,
            None,
            false,
            WORKER_STALE_SECS,
            inbox(0, None),
        )
        .expect("a lapsed-heartbeat worker must render an alert");

        assert!(rendered.contains("⚠ STALLED"), "{rendered}");
        assert!(!rendered.contains("between turns"), "{rendered}");
    }

    /// Harnesses that DO publish a turn-start artifact keep the old verdict
    /// verbatim — this change narrows the flag, it does not remove it.
    #[test]
    fn turn_observable_harnesses_keep_the_stalled_verdict() {
        for cli in [cas_mux::SupervisorCli::Codex, cas_mux::SupervisorCli::Grok] {
            assert!(
                harness_publishes_turn_start(cli),
                "{cli:?} publishes a turn-start artifact"
            );
            let rendered = format_priority_worker_status_alert(
                true,
                Some((900, "checkpoint")),
                300,
                None,
                harness_publishes_turn_start(cli),
                0,
                inbox(5, Some(9_000)),
            )
            .expect("alert");
            assert!(rendered.contains("⚠ STALLED"), "{cli:?}: {rendered}");
        }
    }

    // --- cas-f08d (GH #147): the mandated wait pattern is not a wedge.

    fn at(offset_secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
            + chrono::Duration::seconds(offset_secs)
    }

    fn reminder_row(created_offset: i64) -> UnseenDelivery {
        UnseenDelivery {
            is_reminder_delivery: true,
            created_at: at(created_offset),
        }
    }

    fn work_row(created_offset: i64) -> UnseenDelivery {
        UnseenDelivery {
            is_reminder_delivery: false,
            created_at: at(created_offset),
        }
    }

    /// The classifier must accept whatever the PRODUCER actually writes, not
    /// whatever this test file believes it writes. `fire_reminder` builds its
    /// prompt with `cas_store::format_reminder_delivery`, so the classifier is
    /// pointed at that same function's real output here: reword the format and
    /// this fails, rather than the false positive quietly coming back.
    #[test]
    fn the_classifier_accepts_what_the_producer_actually_writes() {
        for prompt in [
            cas_store::format_reminder_delivery(44, "check CI again", None),
            cas_store::format_reminder_delivery(7, "review the merge", Some("cas-1 completed")),
        ] {
            assert!(
                is_reminder_delivery(&prompt),
                "producer output must classify as a reminder delivery: {prompt}"
            );
        }
    }

    /// Only the daemon's `Reminder #<id>: ` delivery shape counts. A human (or
    /// supervisor) message that merely talks about reminders is real mail and
    /// must never be discounted.
    #[test]
    fn only_the_daemon_reminder_delivery_shape_is_treated_as_a_reminder() {
        assert!(is_reminder_delivery("Reminder #44: check CI again"));
        assert!(is_reminder_delivery(
            "  Reminder #7: (triggered by: task completed)"
        ));
        assert!(!is_reminder_delivery(
            "Reminder: stop polling and close the task"
        ));
        assert!(!is_reminder_delivery("Reminder #abc: malformed"));
        assert!(!is_reminder_delivery("Reminder #12 without a colon"));
        assert!(!is_reminder_delivery(
            "Please set a Reminder #44: for the CI run"
        ));
    }

    /// AC-2. The reported shape: reminders #44/#45 were delivered, acted on,
    /// and re-armed as #46 — but nothing ever marked them seen, so they sat in
    /// the unread count for 517s and read as undelivered work.
    #[test]
    fn acted_on_reminder_deliveries_leave_the_unread_count() {
        let wait = PendingReminderWait {
            id: 46,
            due_in_secs: 300,
            armed_at: at(-100),
        };
        let rows = [reminder_row(-517), reminder_row(-400)];

        let classified = classify_worker_inbox(2, Some(517), &rows, Some(wait), at(0));

        assert_eq!(classified.unread, 0, "both were consumed: {classified:?}");
        assert_eq!(classified.oldest_unread_secs, None, "{classified:?}");
        assert_eq!(classified.reminder_wait, Some(wait), "{classified:?}");
    }

    /// AC-1. That worker must read as waiting, and the line must name the
    /// reminder and when it fires — the two facts that tell a supervisor the
    /// silence has an owner and an end.
    #[test]
    fn reminder_wait_is_not_flagged_not_waking_and_names_the_reminder() {
        let rendered = format_priority_worker_status_alert(
            true,
            Some((517, "checkpoint")),
            300,
            None,
            false,
            0,
            WorkerInbox {
                unread: 0,
                oldest_unread_secs: None,
                reminder_wait: Some(PendingReminderWait {
                    id: 46,
                    due_in_secs: 185,
                    armed_at: at(-100),
                }),
            },
        )
        .expect("a reminder-waiting worker still renders a line");

        assert!(!rendered.contains("NOT WAKING"), "{rendered}");
        assert!(!rendered.contains("STALLED"), "{rendered}");
        assert!(rendered.contains("reminder #46"), "{rendered}");
        assert!(rendered.contains("due in 3m 5s"), "{rendered}");
    }

    /// AC-4. The banner exists to stop the reflex it caused: a turn-breaking
    /// urgent interrupt aimed at a healthy worker.
    #[test]
    fn reminder_wait_banner_steers_the_supervisor_away_from_interrupting() {
        let rendered = format_reminder_wait_status(
            Some((517, "checkpoint")),
            PendingReminderWait {
                id: 46,
                due_in_secs: 185,
                armed_at: at(-100),
            },
        );

        assert!(rendered.contains("do NOT interrupt"), "{rendered}");
        assert!(rendered.contains("sanctioned wait pattern"), "{rendered}");
        assert!(rendered.contains("nothing unread"), "{rendered}");
    }

    /// AC-3, half one: an unread WORK message is not discounted by a pending
    /// reminder. Suppressing on "a reminder exists" would mask a real wedge in
    /// any worker that happens to hold one.
    #[test]
    fn a_pending_reminder_does_not_discount_an_unread_work_message() {
        let wait = PendingReminderWait {
            id: 46,
            due_in_secs: 300,
            armed_at: at(-100),
        };
        let rows = [reminder_row(-900), work_row(-800)];

        let classified = classify_worker_inbox(2, Some(900), &rows, Some(wait), at(0));

        assert_eq!(
            classified.unread, 1,
            "the work row survives: {classified:?}"
        );
        assert_eq!(classified.oldest_unread_secs, Some(800), "{classified:?}");
        assert_eq!(
            classified.reminder_wait, None,
            "unread mail outranks the wait line: {classified:?}"
        );
    }

    /// AC-3, half two: end to end — that same worker still trips the alarm.
    #[test]
    fn genuine_wedge_still_flags_not_waking_while_a_reminder_is_pending() {
        let wait = PendingReminderWait {
            id: 46,
            due_in_secs: 300,
            armed_at: at(-100),
        };
        let classified = classify_worker_inbox(1, Some(900), &[work_row(-900)], Some(wait), at(0));

        let rendered = format_priority_worker_status_alert(
            true,
            Some((900, "checkpoint")),
            300,
            None,
            false,
            0,
            classified,
        )
        .expect("alert");

        assert!(rendered.contains("⚠ NOT WAKING"), "{rendered}");
        assert!(rendered.contains("1 message unread for 900s"), "{rendered}");
        assert!(rendered.contains("wedged harness"), "{rendered}");
    }

    /// A reminder delivery that arrived AFTER the worker last armed anything is
    /// mail it has not reacted to. Only the re-arm ordering proves wake, so
    /// this one keeps its alarm.
    #[test]
    fn a_reminder_delivered_after_the_last_re_arm_is_still_unread() {
        let wait = PendingReminderWait {
            id: 46,
            due_in_secs: 300,
            armed_at: at(-900),
        };
        let classified =
            classify_worker_inbox(1, Some(600), &[reminder_row(-600)], Some(wait), at(0));

        assert_eq!(classified.unread, 1, "{classified:?}");
        assert_eq!(classified.oldest_unread_secs, Some(600), "{classified:?}");
    }

    /// THE KNOWN RESIDUAL (documented deliberately rather than papered over).
    ///
    /// A worker that woke, acted, and chose NOT to re-arm — the final check of
    /// its lane — leaves reminder rows that are consumed in fact but unproven
    /// by the re-arm rule, so they keep counting as unread. Widening the rule
    /// to cover them would mean suppressing on the mere existence of a past
    /// reminder, which is exactly the hole that lets a real wedge hide.
    ///
    /// It is survivable because the alarm needs TWO conditions, not one:
    /// `is_worker_stalled` requires an IN-PROGRESS task before
    /// `format_priority_worker_status_alert` even consults the inbox. The
    /// worker in this state has normally just closed or parked its task, so
    /// `stalled` is false and no alarm fires however the rows are counted —
    /// asserted below.
    ///
    /// The residual therefore narrows to one shape: a worker that acted on its
    /// last reminder, did not re-arm, and left a task IN PROGRESS while going
    /// quiet past the stall threshold. That worker gets `NOT WAKING` with a
    /// misleading unread count — but it is also genuinely idle on live work and
    /// waiting for nothing scheduled, so a supervisor looking at it is not
    /// looking at the wrong thing. Reported to the supervisor rather than
    /// silently widened.
    #[test]
    fn a_consumed_reminder_without_a_re_arm_cannot_alarm_once_the_task_is_done() {
        let classified = classify_worker_inbox(1, Some(900), &[reminder_row(-900)], None, at(0));
        assert_eq!(
            classified.unread, 1,
            "unproven by the re-arm rule, so it stays counted: {classified:?}"
        );

        // No in-progress task -> not stalled -> the inbox is never consulted.
        assert!(!is_worker_stalled(false, Some(900), 300, false));
        assert_eq!(
            format_priority_worker_status_alert(
                false,
                Some((900, "checkpoint")),
                300,
                None,
                false,
                0,
                classified,
            ),
            None,
            "a worker with no live task cannot be accused of not waking"
        );
    }

    /// With no pending reminder at all, nothing is discounted — a worker that
    /// was woken and never re-armed has produced no evidence of life.
    #[test]
    fn without_a_pending_reminder_nothing_is_discounted() {
        let classified = classify_worker_inbox(
            2,
            Some(900),
            &[reminder_row(-900), reminder_row(-600)],
            None,
            at(0),
        );

        assert_eq!(classified.unread, 2, "{classified:?}");
        assert_eq!(classified.reminder_wait, None, "{classified:?}");
    }

    /// A backlog deeper than the peek window may only UNDER-discount: the
    /// store's count stays authoritative and the oldest-age fallback keeps the
    /// alarm's evidence intact.
    #[test]
    fn a_backlog_deeper_than_the_peek_window_cannot_manufacture_silence() {
        let wait = PendingReminderWait {
            id: 46,
            due_in_secs: 300,
            armed_at: at(-100),
        };
        let classified =
            classify_worker_inbox(5, Some(900), &[reminder_row(-900)], Some(wait), at(0));

        assert_eq!(classified.unread, 4, "{classified:?}");
        assert_eq!(
            classified.oldest_unread_secs,
            Some(900),
            "store age is kept when every peeked row was discounted: {classified:?}"
        );
        assert_eq!(classified.reminder_wait, None, "{classified:?}");
    }

    /// An empty inbox with a live reminder is the steady state of the wait
    /// pattern, so the wait line replaces the between-turns hedge there too.
    #[test]
    fn an_empty_inbox_with_a_live_reminder_reports_the_wait() {
        let wait = PendingReminderWait {
            id: 51,
            due_in_secs: 60,
            armed_at: at(-30),
        };
        let classified = classify_worker_inbox(0, None, &[], Some(wait), at(0));

        assert_eq!(classified.reminder_wait, Some(wait), "{classified:?}");
        let rendered =
            format_priority_worker_status_alert(true, None, 300, None, false, 0, classified)
                .expect("alert");
        assert!(rendered.contains("waiting on reminder #51"), "{rendered}");
        assert!(!rendered.contains("between turns"), "{rendered}");
    }

    /// The assigned-but-unstarted alarm is about an untouched assignment, not
    /// about silence — a pending reminder must not swallow it.
    #[test]
    fn reminder_wait_does_not_swallow_the_assigned_unstarted_alarm() {
        let rendered = format_priority_worker_status_alert(
            true,
            Some((900, "checkpoint")),
            300,
            Some(("cas-unstarted", 600, 300)),
            false,
            0,
            WorkerInbox {
                unread: 0,
                oldest_unread_secs: None,
                reminder_wait: Some(PendingReminderWait {
                    id: 46,
                    due_in_secs: 185,
                    armed_at: at(-100),
                }),
            },
        )
        .expect("alert");

        assert!(rendered.contains("ASSIGNED BUT UNSTARTED"), "{rendered}");
    }

    /// cas-e728: finished-awaiting-merge is named, and says who is blocking.
    #[test]
    fn parked_task_names_the_supervisor_as_the_blocker() {
        let merge = format_assigned_task_info(
            None,
            None,
            Some((
                "cas-1234",
                "Done work",
                cas_types::TaskStatus::AwaitingMerge,
            )),
            false,
        );
        assert!(merge.contains("cas-1234"), "{merge}");
        assert!(merge.contains("awaiting merge"), "{merge}");
        assert!(merge.contains("WAITING ON YOU"), "{merge}");
        assert!(!merge.contains("none assigned"), "{merge}");

    }

    /// A live in-progress task still outranks a parked one.
    #[test]
    fn in_progress_takes_precedence_over_a_parked_task() {
        let out = format_assigned_task_info(
            Some(("cas-live", "Current work")),
            None,
            Some((
                "cas-parked",
                "Old work",
                cas_types::TaskStatus::AwaitingMerge,
            )),
            false,
        );
        assert!(out.contains("cas-live"), "{out}");
        assert!(!out.contains("cas-parked"), "{out}");
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
    // Reproduction of the reported defect: a worker whose last Cassy event is
    // far in the past (frozen clock) but whose transcript is being actively
    // written. Before cas-a653, `worker_status` fed ONLY the event-store age
    // into the "last activity" line — indistinguishable from a genuinely
    // dead worker. cas-a653 fixed this for hook-less harnesses (Codex) only;
    // cas-c2c2 (below, after the Codex-specific tests) widened it to every
    // harness after the same freeze reproduced live on a Claude worker —
    // see the superseding-tests block further down for why "hook-capable"
    // was never the right gate.

    /// Codex, no Cassy events at all, but a transcript that was JUST written —
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

    /// Codex with a STALE Cassy event (e.g. 20 minutes old, well past any
    /// stall threshold) but a transcript mtime of just a few seconds ago —
    /// the exact "heads-down between Cassy calls" shape from the ozer repro.
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
            "fresher transcript mtime must win over a 20m-stale Cassy event; got {elapsed}s"
        );
        assert_eq!(phase, "activity");
    }

    /// Codex with a FRESH Cassy event and a stale transcript — the event-store
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
        assert!(
            elapsed <= 7,
            "expected the fresh event's ~5s age: {elapsed}"
        );
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
    // transcript had a record 2 seconds prior — because the Cassy event store
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
    // stale/absent Cassy-event reading for Claude and Grok exactly as it
    // already did for Codex. The old assertions (`is_none()` / stale-wins)
    // would now be WRONG, not merely obsolete — keeping them would silently
    // pin the cas-c2c2 defect back in place.

    /// Was `last_worker_activity_with_transcript_claude_ignores_fresh_transcript`
    /// (asserted `is_none()`). Now asserts the opposite on purpose: this is
    /// the literal shape of the interrupt-fixer incident — no Cassy event
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
             Cassy events, not report None (the interrupt-fixer STALLED-at-401s symptom)",
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

    /// Direct repro of the reported incident shape for Claude: a Cassy event
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
            "fresher transcript mtime must win over a 401s-stale Cassy event \
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
            TEST_SESSION,
            cas_mux::SupervisorCli::Claude,
        );
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
            TEST_SESSION,
            cas_mux::SupervisorCli::Claude,
        );
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
            TEST_SESSION,
            cas_mux::SupervisorCli::Claude,
        );
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
            TEST_SESSION,
            cas_mux::SupervisorCli::Claude,
        );
        let expected_path = projects
            .join("-home-usér-projet-café")
            .join(format!("{TEST_SESSION}.jsonl"));
        assert_eq!(got, TranscriptResolution::Resolved(expected_path));
    }

    #[test]
    fn resolve_transcript_no_projects_dir_is_synthesized() {
        // If we can't resolve the home dir (shouldn't happen in practice),
        // the function still returns a usable Synthesized fallback.
        let got = resolve_transcript(
            None,
            Some("/home/alice/x"),
            TEST_SESSION,
            cas_mux::SupervisorCli::Claude,
        );
        let expected = synthesized_transcript_path("/home/alice/x", TEST_SESSION);
        assert_eq!(got, TranscriptResolution::Synthesized(expected));
    }

    #[test]
    fn resolve_transcript_no_clone_path_falls_back_to_placeholder() {
        // When clone_path is None (worker registered without cwd metadata),
        // the Synthesized arm carries the placeholder label instead of a
        // reconstructed path.
        let (_tmp, projects) = fake_projects_dir(&[]);
        let got = resolve_transcript(
            Some(&projects),
            None,
            TEST_SESSION,
            cas_mux::SupervisorCli::Claude,
        );
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
    fn fake_grok_sessions_dir(dirs: &[(&str, &[&str])]) -> (tempfile::TempDir, std::path::PathBuf) {
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
        rollouts: &[(
            &str, /* relative path under sessions */
            &str, /* cwd */
        )],
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

    // --- cas-9e81 (GH #177): transcripts written under a non-default
    // CLAUDE_CONFIG_DIR must still resolve. A two-subscription factory runs
    // panes under `~/.claude-alt`; the single hardcoded `~/.claude/projects`
    // root returned None for every one of them, and the daemon's wake gate
    // reads an unresolvable transcript as "tool call in flight" — a silent,
    // permanent refusal to wake that recipient.

    #[test]
    fn claude_projects_dirs_include_the_alternate_config_dir() {
        let home = std::path::Path::new("/home/alice");
        let dirs = claude_projects_dirs_from(Some(home), Some("~/.claude-alt"));
        assert_eq!(
            dirs,
            vec![
                std::path::PathBuf::from("/home/alice/.claude/projects"),
                std::path::PathBuf::from("/home/alice/.claude-alt/projects"),
            ],
            "both the default and the active CLAUDE_CONFIG_DIR must be searched"
        );

        // No override, or an override that IS the default: exactly one root.
        assert_eq!(
            claude_projects_dirs_from(Some(home), None),
            vec![std::path::PathBuf::from("/home/alice/.claude/projects")]
        );
        assert_eq!(
            claude_projects_dirs_from(Some(home), Some("~/.claude")),
            vec![std::path::PathBuf::from("/home/alice/.claude/projects")]
        );
    }

    #[test]
    fn a_transcript_under_the_alternate_config_dir_resolves() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let default_root = tmp.path().join(".claude").join("projects");
        let alt_root = tmp.path().join(".claude-alt").join("projects");
        let clone = "/home/alice/work/.cas/worktrees/warm-stork-30";
        let slug = escaped_project_slug(clone);
        std::fs::create_dir_all(alt_root.join(&slug)).unwrap();
        std::fs::create_dir_all(default_root.join("some-other-project")).unwrap();
        let transcript = alt_root.join(&slug).join(format!("{TEST_SESSION}.jsonl"));
        std::fs::write(&transcript, b"{}\n").unwrap();

        // The default root alone — the pre-cas-9e81 behavior — finds nothing.
        assert!(
            transcript_path_from_resolution(resolve_transcript(
                Some(&default_root),
                Some(clone),
                TEST_SESSION,
                cas_mux::SupervisorCli::Claude,
            ))
            .is_none(),
            "precondition: the session is not under the default config dir"
        );

        let got = transcript_path_from_resolution(resolve_transcript_in_roots(
            &[default_root, alt_root],
            Some(clone),
            TEST_SESSION,
            cas_mux::SupervisorCli::Claude,
        ));
        assert_eq!(
            got.as_deref(),
            Some(transcript.as_path()),
            "searching every known projects root must find the real transcript"
        );
    }

    #[test]
    fn multi_root_resolution_does_not_double_count_a_repeated_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("projects");
        let clone = "/home/alice/work";
        let slug = escaped_project_slug(clone);
        std::fs::create_dir_all(root.join(&slug)).unwrap();
        let transcript = root.join(&slug).join(format!("{TEST_SESSION}.jsonl"));
        std::fs::write(&transcript, b"{}\n").unwrap();

        // The same root twice must resolve, not degrade into `Ambiguous`.
        let got = resolve_transcript_in_roots(
            &[root.clone(), root],
            Some(clone),
            TEST_SESSION,
            cas_mux::SupervisorCli::Claude,
        );
        assert_eq!(got, TranscriptResolution::Resolved(transcript));
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
        let rel =
            "2026/07/21/rollout-2026-07-21T08-38-21-019f84af-3121-7950-ba14-b01db2dad6c7.jsonl";
        let (_tmp, sessions) = fake_codex_sessions_dir(&[(rel, clone)]);
        // Cassy session id is NOT the rollout UUID — resolution must use cwd.
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
        let rel =
            "2026/07/21/rollout-2026-07-21T08-38-21-019f84af-3121-7950-ba14-b01db2dad6c7.jsonl";
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

    /// cas-8a55: the seam between the pure scanner and a registered worker.
    #[test]
    fn a_registered_codex_worker_whose_first_turn_was_unauthorized_reads_as_an_account_failure() {
        // The seam between the pure scanner and a real registered worker: the
        // agent's spawn-time CODEX_HOME has to reach the rollout, and the
        // rollout's terminal turn has to reach the supervisor as evidence.
        let clone = "/tmp/cas-8a55-unauthorized-worker";
        let rel = "2026/09/03/rollout-2026-09-03T14-12-58-unauthorized.jsonl";
        let (account_home, sessions) = fake_codex_sessions_dir(&[(rel, clone)]);
        let account_dir = account_home.path().to_str().expect("utf-8 account home");
        let rollout = sessions.join(rel);
        let mut contents = std::fs::read_to_string(&rollout).expect("fixture rollout");
        contents.push_str(
            "\n{\"timestamp\":\"2026-09-03T14:13:01.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"last_agent_message\":null,\"error\":{\"message\":\"Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again.\",\"codex_error_info\":\"unauthorized\"}}}\n",
        );
        std::fs::write(&rollout, contents).expect("write rollout");

        let mut agent = cas_types::Agent::new(
            "codex-zen-eagle-20-session".to_string(),
            "zen-eagle-20".to_string(),
        );
        agent
            .metadata
            .insert("worker_cli".to_string(), "codex".to_string());
        agent
            .metadata
            .insert("clone_path".to_string(), clone.to_string());
        agent
            .metadata
            .insert("worker_account_dir".to_string(), account_dir.to_string());

        let evidence = worker_auth_failure_evidence(account_home.path(), &agent);
        let crate::factory_auth_health::AuthFailureEvidence::Failed { message, .. } = evidence
        else {
            panic!("a revoked-token first turn must read as an account failure: {evidence:?}");
        };
        assert!(message.contains("refresh token was revoked"), "{message}");
        assert_eq!(worker_account_dir(&agent).as_deref(), Some(account_dir));
    }

    /// cas-66fd: the supervisor normally runs under its own (often default)
    /// CODEX_HOME.  A worker's persisted spawn account must win, otherwise
    /// worker_status, is-wedged, and debug all report its live rollout as
    /// unresolved despite a fresh named-account transcript on disk.
    #[test]
    fn codex_worker_account_dir_resolves_its_own_rollout_not_supervisor_home() {
        let clone = "/tmp/cas-66fd-named-account-worker";
        let rel = "2026/08/18/rollout-2026-08-18T17-22-01-live.jsonl";
        let (account_home, sessions) = fake_codex_sessions_dir(&[(rel, clone)]);
        let account_dir = account_home.path().to_str().expect("utf-8 account home");

        let resolved = resolve_worker_transcript_path_for_account(
            Some(clone),
            "codex-kind-owl-71-session",
            cas_mux::SupervisorCli::Codex,
            Some(account_dir),
        );
        assert_eq!(resolved, Some(sessions.join(rel)));

        let status_path = worker_status_transcript_path_for_account(
            Some(clone),
            "codex-kind-owl-71-session",
            cas_mux::SupervisorCli::Codex,
            Some(account_dir),
        );
        assert_eq!(status_path, Some(sessions.join(rel)));

        let mut agent = cas_types::Agent::new(
            "codex-kind-owl-71-session".to_string(),
            "kind-owl-71".to_string(),
        );
        agent
            .metadata
            .insert("worker_cli".to_string(), "codex".to_string());
        agent
            .metadata
            .insert("clone_path".to_string(), clone.to_string());
        agent
            .metadata
            .insert("worker_account_dir".to_string(), account_dir.to_string());
        assert_eq!(
            worker_transcript_path_for_agent(account_home.path(), &agent),
            Some(sessions.join(rel)),
            "the registered worker's spawn-time CODEX_HOME must reach all agent-aware callers"
        );
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
            resolve_codex_transcript(Some(&sessions), Some(clone), "codex-worker-cas-session",),
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
            resolve_codex_transcript(Some(&sessions), Some(clone), "codex-worker-cas-session",),
            TranscriptResolution::Resolved(sessions.join(worker_rel))
        );
    }

    #[test]
    fn resolve_codex_transcript_uses_freshest_cli_rollout_for_reused_cwd() {
        let clone = "/tmp/reused-codex-worker";
        let old_rel = "2026/07/27/rollout-2026-07-27T07-59-08-old-cli.jsonl";
        let current_rel = "2026/07/28/rollout-2026-07-28T08-03-58-current-cli.jsonl";
        let (_tmp, sessions) = fake_codex_sessions_dir_with_metadata(&[
            (old_rel, clone, "codex-tui", "cli"),
            (current_rel, clone, "codex-tui", "cli"),
        ]);
        let old_rollout = sessions.join(old_rel);
        let current_rollout = sessions.join(current_rel);
        filetime::set_file_mtime(
            &old_rollout,
            filetime::FileTime::from_system_time(
                std::time::SystemTime::now() - std::time::Duration::from_secs(60),
            ),
        )
        .unwrap();

        assert_eq!(
            resolve_codex_transcript(Some(&sessions), Some(clone), "codex-worker-cas-session",),
            TranscriptResolution::Resolved(current_rollout),
            "worktree reuse leaves old CLI rollouts at the same cwd; the current worker is the freshest CLI session"
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
        assert!(
            age <= 5,
            "activity must track the live worker rollout: {age}"
        );
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
        let _lock = crate::hooks::test_env_lock();
        let session = "grok-session-0000-0000-000000000000";
        let (tmp, sessions) =
            fake_grok_sessions_dir(&[("%2Fhome%2Falice%2Fworkspace", &[session])]);
        let expected = sessions
            .join("%2Fhome%2Falice%2Fworkspace")
            .join(session)
            .join("updates.jsonl");
        let old = std::env::var("GROK_HOME").ok();
        unsafe {
            std::env::set_var("GROK_HOME", tmp.path());
        }

        // This test name is retained from cas-fa69, where AC7 deliberately
        // pinned Grok's then-unreviewed fast-path behavior to `None`.
        // cas-a9ea characterized the real on-disk layout and found that pin
        // preserved a defect: worker_status used a Claude path while
        // is-wedged resolved this Grok updates.jsonl. The legitimate new
        // contract is agreement through the shared harness-aware resolver.
        let worker_status_path = worker_status_transcript_path(
            Some("/home/alice/workspace"),
            session,
            cas_mux::SupervisorCli::Grok,
        );
        let wedged_path = resolve_worker_transcript_path(
            Some("/home/alice/workspace"),
            session,
            cas_mux::SupervisorCli::Grok,
        );
        unsafe {
            match old {
                Some(value) => std::env::set_var("GROK_HOME", value),
                None => std::env::remove_var("GROK_HOME"),
            }
        }

        assert_eq!(
            worker_status_path,
            Some(expected.clone()),
            "worker_status must resolve the real Grok updates.jsonl"
        );
        assert_eq!(
            worker_status_path, wedged_path,
            "worker_status and is-wedged must use the same Grok evidence path"
        );
        assert!(
            expected.with_file_name("signals.json").exists(),
            "the resolved updates.jsonl must retain its sibling signals.json \
             for harness-aware activity age"
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
        assert!(
            age <= 5,
            "fresh rollout must beat the stale Cassy event: {age}"
        );

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
        let (_tmp, sessions) =
            fake_codex_sessions_dir(&[("2026/07/21/rollout-other.jsonl", "/tmp/other")]);
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
        let got = resolve_transcript(
            Some(&sessions),
            None,
            session,
            cas_mux::SupervisorCli::Codex,
        );
        assert_eq!(
            got,
            TranscriptResolution::Synthesized(synthesized_unknown_codex_clone_path(session))
        );
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
                format!(r#"{{"type":"session_meta","payload":{{"cwd":"/tmp/worker-{index}"}}}}"#),
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
    fn codex_worker_status_cache_amortizes_small_and_large_session_trees() {
        fn measure(count: usize) -> (u128, u128) {
            let tmp = tempfile::tempdir().unwrap();
            let sessions = tmp.path().join("sessions");
            let day = sessions.join("2026/07/28");
            std::fs::create_dir_all(&day).unwrap();
            let historical_mtime = filetime::FileTime::from_system_time(
                std::time::SystemTime::now() - std::time::Duration::from_secs(60),
            );
            for index in 0..count {
                let path = day.join(format!("rollout-history-{index:05}.jsonl"));
                std::fs::write(
                    &path,
                    format!(
                        r#"{{"type":"session_meta","payload":{{"cwd":"/tmp/history-{index}","originator":"codex-tui","source":"cli"}}}}"#
                    ),
                )
                .unwrap();
                filetime::set_file_mtime(&path, historical_mtime).unwrap();
            }
            let clone = format!("/tmp/live-worker-{count}");
            let session_id = format!("codex-live-session-{count}");
            let live = day.join(format!("rollout-live-{count}.jsonl"));
            std::fs::write(
                &live,
                format!(
                    r#"{{"type":"session_meta","payload":{{"cwd":"{clone}","originator":"codex-tui","source":"cli"}}}}"#
                ),
            )
            .unwrap();

            let cold_started = std::time::Instant::now();
            let resolved =
                worker_status_codex_transcript_path_in(Some(&sessions), Some(&clone), &session_id);
            let cold_micros = cold_started.elapsed().as_micros();
            assert_eq!(resolved, Some(live));

            let key = WorkerTranscriptCacheKey {
                cli: "codex",
                base_dirs: vec![sessions.clone()],
                clone_path: Some(clone.clone()),
                session_id: session_id.clone(),
            };
            let cached_at = worker_transcript_cache()
                .lock()
                .unwrap()
                .get(&key)
                .expect("cold lookup must populate cache")
                .resolved_at;
            let warm_started = std::time::Instant::now();
            for _ in 0..100 {
                assert!(
                    worker_status_codex_transcript_path_in(
                        Some(&sessions),
                        Some(&clone),
                        &session_id,
                    )
                    .is_some()
                );
            }
            let warm_avg_nanos = warm_started.elapsed().as_nanos() / 100;
            let still_cached_at = worker_transcript_cache()
                .lock()
                .unwrap()
                .get(&key)
                .expect("warm lookup must retain cache entry")
                .resolved_at;
            assert_eq!(
                cached_at, still_cached_at,
                "warm worker_status/activity polls must not rerun resolution"
            );
            (cold_micros, warm_avg_nanos)
        }

        let (small_cold, small_warm) = measure(25);
        let (large_cold, large_warm) = measure(1_000);
        eprintln!(
            "cas-7182 resolver: small tree cold={small_cold}µs warm_avg={small_warm}ns; \
             large tree cold={large_cold}µs warm_avg={large_warm}ns"
        );
    }

    #[test]
    fn codex_worker_status_cache_refreshes_after_ttl() {
        let clone = "/tmp/codex-cache-refresh";
        let session_id = "codex-cache-refresh-session";
        let first_rel = "2026/07/28/rollout-first.jsonl";
        let (_tmp, sessions) = fake_codex_sessions_dir(&[(first_rel, clone)]);
        let first = sessions.join(first_rel);
        assert_eq!(
            worker_status_codex_transcript_path_in(Some(&sessions), Some(clone), session_id),
            Some(first.clone())
        );

        let second = sessions.join("2026/07/28/rollout-second.jsonl");
        std::fs::write(
            &second,
            format!(
                r#"{{"type":"session_meta","payload":{{"cwd":"{clone}","originator":"codex-tui","source":"cli"}}}}"#
            ),
        )
        .unwrap();
        filetime::set_file_mtime(
            &first,
            filetime::FileTime::from_system_time(
                std::time::SystemTime::now() - std::time::Duration::from_secs(60),
            ),
        )
        .unwrap();

        let key = WorkerTranscriptCacheKey {
            cli: "codex",
            base_dirs: vec![sessions.clone()],
            clone_path: Some(clone.to_string()),
            session_id: session_id.to_string(),
        };
        worker_transcript_cache()
            .lock()
            .unwrap()
            .get_mut(&key)
            .expect("cache entry")
            .resolved_at = std::time::Instant::now() - WORKER_TRANSCRIPT_CACHE_TTL;

        assert_eq!(
            worker_status_codex_transcript_path_in(Some(&sessions), Some(clone), session_id),
            Some(second),
            "expired cache entries must discover a newer live rollout"
        );
    }

    #[test]
    fn grok_worker_status_cache_amortizes_and_refreshes_after_ttl() {
        let session_id = "grok-cache-refresh-session";
        let clone = "/tmp/grok-cache-refresh";
        let (_tmp, sessions) = fake_grok_sessions_dir(&[("first-cwd", &[session_id])]);
        let first = sessions
            .join("first-cwd")
            .join(session_id)
            .join("updates.jsonl");
        let cold = worker_status_cached_transcript_resolution_in(
            Some(&sessions),
            Some(clone),
            session_id,
            cas_mux::SupervisorCli::Grok,
        );
        assert_eq!(cold, TranscriptResolution::Resolved(first.clone()));

        let key = WorkerTranscriptCacheKey {
            cli: "grok",
            base_dirs: vec![sessions.clone()],
            clone_path: Some(clone.to_string()),
            session_id: session_id.to_string(),
        };
        let cached_at = worker_transcript_cache()
            .lock()
            .unwrap()
            .get(&key)
            .expect("cold Grok lookup must populate cache")
            .resolved_at;

        std::fs::remove_file(&first).unwrap();
        let second_dir = sessions.join("second-cwd").join(session_id);
        std::fs::create_dir_all(&second_dir).unwrap();
        let second = second_dir.join("updates.jsonl");
        std::fs::write(&second, b"").unwrap();

        let warm = worker_status_cached_transcript_resolution_in(
            Some(&sessions),
            Some(clone),
            session_id,
            cas_mux::SupervisorCli::Grok,
        );
        assert_eq!(
            warm,
            TranscriptResolution::Resolved(first),
            "warm worker_status/activity polls must reuse the Grok resolution"
        );
        assert_eq!(
            worker_transcript_cache()
                .lock()
                .unwrap()
                .get(&key)
                .expect("warm Grok lookup must retain cache entry")
                .resolved_at,
            cached_at
        );

        worker_transcript_cache()
            .lock()
            .unwrap()
            .get_mut(&key)
            .expect("Grok cache entry")
            .resolved_at = std::time::Instant::now() - WORKER_TRANSCRIPT_CACHE_TTL;
        let refreshed = worker_status_cached_transcript_resolution_in(
            Some(&sessions),
            Some(clone),
            session_id,
            cas_mux::SupervisorCli::Grok,
        );
        assert_eq!(
            refreshed,
            TranscriptResolution::Resolved(second),
            "expired Grok cache entries must discover the moved transcript"
        );
    }

    #[test]
    fn hard_dead_codex_status_surfaces_cached_synthesized_and_ambiguous_salvage() {
        let empty = tempfile::tempdir().unwrap();
        let session_id = "codex-hard-dead-synthesized";
        let clone = "/tmp/codex-hard-dead";
        let synthesized = WorkerStatusTranscriptResolution {
            resolution: worker_status_cached_transcript_resolution_in(
                Some(empty.path()),
                Some(clone),
                session_id,
                cas_mux::SupervisorCli::Codex,
            ),
            base_dir_resolved: true,
        };
        let synthesized_output = hard_dead_worker_transcript_block(
            Some(&synthesized),
            Some(clone),
            session_id,
            cas_mux::SupervisorCli::Codex,
        );
        assert!(synthesized_output.contains("Likely transcript:"));
        assert!(synthesized_output.contains(&format!("Session: {session_id}")));
        assert!(!synthesized_output.contains("unresolved Codex rollout"));

        let collision_id = "019f84af-3121-7950-ba14-b01db2dad6c7";
        let first_rel = format!("2026/07/28/rollout-first-{collision_id}.jsonl");
        let second_rel = format!("2026/07/29/rollout-second-{collision_id}.jsonl");
        let (_tmp, sessions) =
            fake_codex_sessions_dir(&[(&first_rel, clone), (&second_rel, clone)]);
        let ambiguous = WorkerStatusTranscriptResolution {
            resolution: worker_status_cached_transcript_resolution_in(
                Some(&sessions),
                Some(clone),
                collision_id,
                cas_mux::SupervisorCli::Codex,
            ),
            base_dir_resolved: true,
        };
        let ambiguous_output = hard_dead_worker_transcript_block(
            Some(&ambiguous),
            Some(clone),
            collision_id,
            cas_mux::SupervisorCli::Codex,
        );
        assert!(ambiguous_output.contains("Transcript candidates"));
        assert!(ambiguous_output.contains(&first_rel));
        assert!(ambiguous_output.contains(&second_rel));
        assert!(ambiguous_output.contains("Likely synthesized:"));
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
    fn worker_status_names_a_fresh_compacting_checkpoint_as_heads_down() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            "context_checkpoint_state".to_string(),
            "compacting".to_string(),
        );
        metadata.insert(
            "context_checkpoint_task_id".to_string(),
            "cas-41c28".to_string(),
        );
        metadata.insert(
            "context_checkpoint_branch".to_string(),
            "factory/worker".to_string(),
        );
        metadata.insert(
            "context_checkpoint_at".to_string(),
            chrono::Utc::now().to_rfc3339(),
        );

        let rendered = format_context_checkpoint_status(&metadata);
        assert!(rendered.contains("compacting / heads-down"));
        assert!(rendered.contains("cas-41c28"));
        assert!(rendered.contains("factory/worker"));
    }

    #[test]
    fn context_band_ok_below_50_pct() {
        assert_eq!(context_band(0, 200_000), "ok");
        assert_eq!(context_band(49_999, 100_000), "ok");
        assert_eq!(context_band(99_999, 200_000), "ok");
    }

    #[test]
    fn context_band_approaching_50_to_79_pct() {
        assert_eq!(context_band(100_000, 200_000), "approaching");
        assert_eq!(context_band(150_000, 200_000), "approaching");
        assert_eq!(context_band(159_999, 200_000), "approaching");
    }

    #[test]
    fn context_band_near_limit_at_80_pct_and_above() {
        assert_eq!(context_band(160_000, 200_000), "near-limit");
        assert_eq!(context_band(200_000, 200_000), "near-limit");
        assert_eq!(context_band(210_000, 200_000), "near-limit");
    }

    #[test]
    fn context_usage_display_reports_actual_headroom() {
        assert_eq!(
            format_context_usage(ContextUsage {
                input_tokens: 40_000,
                model_context_window: Some(1_000_000),
            }),
            "\n    context: ok (~40k / 1000k tk; ~96% headroom)"
        );
    }

    #[test]
    fn context_usage_display_never_guesses_a_window() {
        let display = format_context_usage(ContextUsage {
            input_tokens: 203_000,
            model_context_window: None,
        });

        assert_eq!(
            display,
            "\n    context input: ~203k tk (model window unavailable)"
        );
        assert!(!display.contains("near-limit"));
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

        let usage = read_context_usage_from_tail_for_cli(tmp.path(), cas_mux::SupervisorCli::Codex)
            .expect("Codex token_count event should produce a context reading");
        assert_eq!(usage.input_tokens, 123_456);
        assert_eq!(usage.model_context_window, Some(258_400));
        assert_eq!(
            context_band(usage.input_tokens, usage.model_context_window.unwrap()),
            "ok"
        );
    }

    #[test]
    fn codex_context_after_compaction_uses_the_new_live_window_occupancy() {
        use std::io::Write;

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        // The first event is the pre-compaction high-water prompt. The second
        // is the new, compacted turn and must be the only value supervisors use.
        writeln!(
            tmp.as_file(),
            r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":203000}},"model_context_window":1000000}}}}}}"#
        )
        .unwrap();
        writeln!(
            tmp.as_file(),
            r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":40000}},"model_context_window":1000000}}}}}}"#
        )
        .unwrap();

        let usage = read_context_usage_from_tail_for_cli(tmp.path(), cas_mux::SupervisorCli::Codex)
            .expect("latest compacted turn should produce a context reading");
        assert_eq!(usage.input_tokens, 40_000);
        assert_eq!(usage.model_context_window, Some(1_000_000));
        assert_eq!(
            context_band(usage.input_tokens, usage.model_context_window.unwrap()),
            "ok"
        );
    }

    #[test]
    fn read_context_usage_from_codex_rollout_recovers_split_utf8_tail_boundary() {
        use std::io::Write;

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let token_count = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":123456}}}}"#;
        let filler_len = 8190usize
            .checked_sub(2 + token_count.len())
            .expect("token fixture fits in the tail window");

        tmp.write_all(b"prefix").unwrap();
        tmp.write_all("✓".as_bytes()).unwrap();
        tmp.write_all(b"\n").unwrap();
        tmp.write_all(&vec![b'x'; filler_len]).unwrap();
        tmp.write_all(b"\n").unwrap();
        tmp.write_all(token_count.as_bytes()).unwrap();

        let bytes = std::fs::read(tmp.path()).unwrap();
        let tail_start = bytes.len() - 8192;
        assert_eq!(tail_start, b"prefix".len() + 1);
        assert!(
            std::str::from_utf8(&bytes[tail_start..]).is_err(),
            "fixture must make the old read_to_string path fail at a continuation byte"
        );

        let total = read_context_usage_from_tail_for_cli(tmp.path(), cas_mux::SupervisorCli::Codex)
            .expect("a split UTF-8 code point at the seek boundary must not hide usage");
        assert_eq!(total.input_tokens, 123_456);
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
            .args(["config", "user.name", "Cassy Test"])
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
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();

        (tmp, sha)
    }

    fn setup_git_repo_with_pushed_factory_branch(worker: &str) -> tempfile::TempDir {
        let (tmp, _expected_sha) = setup_git_repo_with_factory_branch(worker);
        let remote_ref = format!("refs/remotes/origin/factory/{worker}");
        run_git_ok(tmp.path(), &["update-ref", remote_ref.as_str(), "HEAD"]);
        tmp
    }

    #[cfg(unix)]
    fn write_gh_stub(dir: &std::path::Path, script: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("gh-stub");
        std::fs::write(&path, script).expect("write gh stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make gh stub executable");
        path
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
            .args(["config", "user.name", "Cassy Test"])
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
            "head_sha must match git rev-parse HEAD (full width, cas-ea51)"
        );
    }

    fn setup_mid_merge_with_incoming_and_worker_contributions() -> tempfile::TempDir {
        let repo = tempfile::TempDir::new().expect("tempdir");
        let path = repo.path();
        run_git_ok(path, &["init", "-b", "main"]);
        run_git_ok(path, &["config", "user.email", "test@cas"]);
        run_git_ok(path, &["config", "user.name", "Cassy Test"]);

        std::fs::write(path.join("shared.txt"), "base\n").unwrap();
        run_git_ok(path, &["add", "."]);
        run_git_ok(path, &["commit", "-m", "base"]);

        run_git_ok(path, &["checkout", "-b", "factory/m1"]);
        std::fs::write(path.join("worker-only.rs"), "// m1 contribution\n").unwrap();
        std::fs::write(path.join("shared.txt"), "m1 side\n").unwrap();
        run_git_ok(path, &["add", "."]);
        run_git_ok(path, &["commit", "-m", "m1 contribution"]);

        run_git_ok(path, &["checkout", "main"]);
        std::fs::write(path.join("incoming-m2.rs"), "// m2 incoming\n").unwrap();
        std::fs::write(path.join("shared.txt"), "m2 side\n").unwrap();
        run_git_ok(path, &["add", "."]);
        run_git_ok(path, &["commit", "-m", "m2 contribution"]);

        run_git_ok(path, &["checkout", "factory/m1"]);
        let merge = std::process::Command::new("git")
            .args(["merge", "main"])
            .current_dir(path)
            .output()
            .expect("start conflicting merge");
        assert!(!merge.status.success(), "fixture merge must conflict");
        assert!(
            run_git(path, &["rev-parse", "--verify", "-q", "MERGE_HEAD"]).is_ok(),
            "fixture must retain MERGE_HEAD"
        );
        repo
    }

    /// cas-d04f: cleanly merged M2 paths are staged during a conflict, but are
    /// not M1 drift. The status classifier must inspect M1's committed range.
    #[test]
    fn merge_head_scope_excludes_staged_incoming_paths() {
        let repo = setup_mid_merge_with_incoming_and_worker_contributions();
        let porcelain =
            run_git(repo.path(), &["status", "--porcelain"]).expect("read fixture porcelain");
        assert!(
            porcelain.contains("incoming-m2.rs"),
            "precondition: Git staged the clean incoming M2 file: {porcelain}"
        );

        let paths = worker_scope_paths(repo.path()).expect("scope paths");
        assert!(
            paths.iter().any(|path| path == "worker-only.rs"),
            "{paths:?}"
        );
        assert!(paths.iter().any(|path| path == "shared.txt"), "{paths:?}");
        assert!(
            !paths.iter().any(|path| path == "incoming-m2.rs"),
            "an incoming staged merge path is not a worker contribution: {paths:?}"
        );
    }

    /// The MERGE_HEAD exception must not turn into an all-clear: a real worker
    /// contribution remains visible to worker-status even while the index is
    /// full of another lane's cleanly merged files.
    #[test]
    fn merge_head_scope_keeps_genuine_worker_contribution_flagged() {
        let repo = setup_mid_merge_with_incoming_and_worker_contributions();
        let status = collect_worker_git_status(repo.path());
        assert!(
            status.dirty,
            "worker-only.rs is a real factory/m1 contribution and must remain flagged"
        );
        assert!(
            worker_scope_paths(repo.path())
                .expect("scope paths")
                .iter()
                .any(|path| path == "worker-only.rs"),
            "the real worker contribution must remain in the contribution diff"
        );
    }

    /// AC1 (cas-ea51): the emitter stores a FULL 40-char SHA, not git's
    /// dynamic `--short` abbreviation.
    ///
    /// This is the regression guard for the defect the cas-7ad6 spec measured:
    /// the live DB holds a mix of 7- and 8-char SHAs because `--short` width
    /// grows with repo size, so any consumer slicing `sha[0..8]` silently
    /// missed 60% of usable rows. Asserting the exact length (not just
    /// "longer than 8") is what keeps a future `--short` from creeping back in.
    #[test]
    fn collect_git_status_head_sha_is_full_40_chars() {
        let (tmp, _) = setup_git_repo_with_factory_branch("test-worker");
        let status = collect_worker_git_status(tmp.path());
        assert_eq!(
            status.head_sha.len(),
            40,
            "head_sha must be a full 40-char SHA so it is an exact join key; got {} chars: '{}'",
            status.head_sha.len(),
            status.head_sha
        );
        assert!(
            status.head_sha.chars().all(|c| c.is_ascii_hexdigit()),
            "head_sha must be all hex digits, got '{}'",
            status.head_sha
        );
    }

    /// AC2 (cas-ea51): storage stays full-width while rendering truncates, and
    /// the "?" error sentinel survives truncation rather than panicking on an
    /// out-of-bounds slice.
    #[test]
    fn head_sha_for_display_truncates_and_is_length_safe() {
        let full = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(head_sha_for_display(full), "01234567");
        // The documented degradation sentinel is shorter than the display
        // width — it must pass through, not panic.
        assert_eq!(head_sha_for_display("?"), "?");
        assert_eq!(head_sha_for_display(""), "");
        // A legacy short SHA already in the DB renders unchanged.
        assert_eq!(head_sha_for_display("abc1234"), "abc1234");
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
            pr_url: WorkerPrUrl::Url("https://github.com/org/repo/pull/42".to_string()),
            is_shared_checkout: false,
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

    /// cas-ecf7 (GH #118): a worktree behind its base must say so in words, not
    /// just as a number in a status line the supervisor scrolls past — the
    /// reported incident ran three workers on 25-commit-old history because the
    /// `behind:` count was the only signal.
    #[test]
    fn format_git_status_calls_out_a_stale_base_loudly() {
        let stale = WorkerGitStatus {
            branch: "factory/late".to_string(),
            head_sha: "0bd4a26".to_string(),
            ahead: 0,
            behind: 25,
            base_branch: "origin/main".to_string(),
            dirty: false,
            pushed_ref: "none".to_string(),
            pr_url: WorkerPrUrl::None,
            is_shared_checkout: false,
        };
        let out = format_worker_git_status(&stale);
        assert!(
            out.contains("STALE BASE"),
            "a behind worktree must carry an explicit stale-base callout: {out}"
        );
        assert!(
            out.contains("25 commit(s) behind origin/main"),
            "the callout must quantify the gap and name the base: {out}"
        );

        let current = WorkerGitStatus { behind: 0, ..stale };
        assert!(
            !format_worker_git_status(&current).contains("STALE BASE"),
            "an up-to-date worktree must not be flagged"
        );
    }

    // ── cas-5bef (GH #120): shared checkout parked on factory/* ──────────

    /// AC1 (cas-5bef): the shared primary checkout sitting on a `factory/*`
    /// branch must be called out by name, with the trunk it is NOT on and the
    /// remedy — the incident's `git merge --ff-only` / `git push origin main`
    /// both reported success while landing nothing on main.
    #[test]
    fn format_git_status_calls_out_a_parked_shared_checkout_loudly() {
        let parked = WorkerGitStatus {
            branch: "factory/bright-eagle-91".to_string(),
            head_sha: "0bd4a26".to_string(),
            ahead: 1,
            behind: 0,
            base_branch: "origin/main".to_string(),
            dirty: false,
            pushed_ref: "none".to_string(),
            pr_url: WorkerPrUrl::None,
            is_shared_checkout: true,
        };
        let out = format_worker_git_status(&parked);
        assert!(
            out.contains("SHARED CHECKOUT PARKED"),
            "a parked shared checkout needs an explicit callout: {out}"
        );
        assert!(
            out.contains("factory/bright-eagle-91"),
            "the callout must name the parked branch: {out}"
        );
        assert!(
            out.contains("'main'"),
            "the callout must name the trunk it is not on: {out}"
        );
        assert!(
            out.contains("git switch main"),
            "the callout must state the remedy: {out}"
        );
        assert!(
            out.contains("git worktree add"),
            "the callout must steer to a worktree instead of a branch in place: {out}"
        );
    }

    /// The warning is specific to the two conditions that produced GH #120 —
    /// an isolated worktree on factory/* is the normal case and a shared
    /// checkout on trunk is the healthy one; neither may be flagged, or the
    /// callout becomes noise the supervisor scrolls past.
    #[test]
    fn parked_warning_only_fires_for_a_shared_checkout_on_a_factory_branch() {
        assert!(
            shared_checkout_parked_warning(true, "factory/bright-eagle-91", "origin/main")
                .is_some(),
            "shared checkout on factory/* is the GH #120 shape"
        );
        assert!(
            shared_checkout_parked_warning(false, "factory/bright-eagle-91", "origin/main")
                .is_none(),
            "an isolated worktree on its own factory branch is normal"
        );
        assert!(
            shared_checkout_parked_warning(true, "main", "origin/main").is_none(),
            "a shared checkout on trunk is the healthy state"
        );
        // Non-origin bases and bare trunk names both resolve to a usable
        // `git switch <trunk>` remedy.
        let staging =
            shared_checkout_parked_warning(true, "factory/w", "upstream/staging").expect("warn");
        assert!(
            staging.contains("git switch staging"),
            "remedy must name the actual trunk: {staging}"
        );
        let bare = shared_checkout_parked_warning(true, "factory/w", "main").expect("warn");
        assert!(
            bare.contains("git switch main"),
            "a bare base branch needs no stripping: {bare}"
        );
    }

    /// AC3 (cas-5bef): reproduce the GH #120 sequence against real git — a
    /// worker creates `factory/*` IN the shared checkout and leaves it checked
    /// out — and assert the supervisor-visible status carries the warning
    /// BEFORE the merge/tag step could misfire. The linked worktree in the same
    /// repo must stay quiet.
    #[test]
    fn collect_git_status_flags_the_gh120_shared_checkout_repro() {
        let tmp = make_git_repo_for_status();
        let shared = tmp.path().join("repo");
        let shared = shared.as_path();

        // The GH #120 escape: refused on main, the worker branches in place.
        run_git_ok(shared, &["checkout", "-b", "factory/bright-eagle-91"]);

        let shared_status = collect_worker_git_status(shared);
        assert!(
            shared_status.is_shared_checkout,
            "the primary checkout must be identified as shared, got branch {}",
            shared_status.branch
        );
        let rendered = format_worker_git_status(&shared_status);
        assert!(
            rendered.contains("SHARED CHECKOUT PARKED"),
            "worker_status must warn before the supervisor's merge/tag step: {rendered}"
        );
        assert!(
            rendered.contains("factory/bright-eagle-91"),
            "the warning must name the parked branch: {rendered}"
        );

        // A linked worktree on the very same factory branch shape is normal.
        let linked = tmp.path().join("linked-worktree");
        run_git_ok(
            shared,
            &[
                "worktree",
                "add",
                "-b",
                "factory/isolated-worker",
                &linked.to_string_lossy(),
            ],
        );
        let linked_status = collect_worker_git_status(&linked);
        assert!(
            !linked_status.is_shared_checkout,
            "a linked worktree must not be reported as the shared checkout"
        );
        assert!(
            !format_worker_git_status(&linked_status).contains("SHARED CHECKOUT PARKED"),
            "isolated workers must not be flagged"
        );
    }

    fn run_git_ok(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A committed git repo at `<tempdir>/repo`, so a linked worktree can be
    /// added as a sibling inside the same temp dir.
    fn make_git_repo_for_status() -> tempfile::TempDir {
        let outer = tempfile::TempDir::new().expect("tempdir");
        let repo = outer.path().join("repo");
        std::fs::create_dir(&repo).expect("mkdir repo");
        run_git_ok(&repo, &["init", "-b", "main"]);
        run_git_ok(&repo, &["config", "user.email", "t@example.com"]);
        run_git_ok(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f.txt"), "x").expect("write");
        run_git_ok(&repo, &["add", "."]);
        run_git_ok(&repo, &["commit", "-m", "init"]);
        outer
    }

    /// A missing `gh` lookup is not a successful no-PR result.  Keep the
    /// pushed-ref short-circuit covered separately below, while this fixture
    /// makes the branch look pushed and injects an absent executable.
    #[test]
    fn collect_git_status_reports_unknown_when_gh_is_unavailable() {
        let (tmp, _expected_sha) = setup_git_repo_with_factory_branch("test-worker");
        run_git_ok(
            tmp.path(),
            &[
                "update-ref",
                "refs/remotes/origin/factory/test-worker",
                "HEAD",
            ],
        );

        let missing_gh = tmp.path().join("gh-not-installed");
        let status = collect_worker_git_status_with_gh(tmp.path(), &missing_gh);
        assert_eq!(status.pr_url, WorkerPrUrl::Unknown("gh unavailable"));
        let rendered = format_worker_git_status(&status);
        assert!(
            rendered.contains("PR: unknown (gh unavailable)"),
            "failed gh lookup must be visible as unknown: {rendered}"
        );
        assert!(
            !rendered.contains("PR: none"),
            "failed gh lookup must not masquerade as no PR: {rendered}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn collect_git_status_reports_gh_failure_as_redacted_unknown() {
        let tmp = setup_git_repo_with_pushed_factory_branch("gh-failure-worker");
        let secret = "synthetic-gh-secret";
        let gh = write_gh_stub(
            tmp.path(),
            &format!(
                "#!/bin/sh\nprintf '%s' 'auth/network failure: {secret}' >&2\nexit 7\n"
            ),
        );

        let status = collect_worker_git_status_with_gh(tmp.path(), &gh);
        assert_eq!(status.pr_url, WorkerPrUrl::Unknown("gh lookup failed"));
        let rendered = format_worker_git_status(&status);
        assert!(
            rendered.contains("PR: unknown (gh lookup failed)"),
            "auth/network gh failure must render as unknown: {rendered}"
        );
        assert!(
            !rendered.contains("PR: none"),
            "auth/network gh failure must not masquerade as no PR: {rendered}"
        );
        assert!(
            !rendered.contains(secret),
            "gh stderr secrets must not leak into worker status: {rendered}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn collect_git_status_reports_successful_empty_gh_as_definite_none() {
        let tmp = setup_git_repo_with_pushed_factory_branch("gh-empty-worker");
        let gh = write_gh_stub(tmp.path(), "#!/bin/sh\nexit 0\n");

        let status = collect_worker_git_status_with_gh(tmp.path(), &gh);
        assert_eq!(status.pr_url, WorkerPrUrl::None);
        let rendered = format_worker_git_status(&status);
        assert!(
            rendered.contains("PR: none"),
            "successful empty gh output must render as no PR: {rendered}"
        );
        assert!(
            !rendered.contains("PR: unknown"),
            "successful empty gh output must not render as unknown: {rendered}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn collect_git_status_preserves_successful_gh_url() {
        let tmp = setup_git_repo_with_pushed_factory_branch("gh-url-worker");
        let expected_url = "https://github.com/org/repo/pull/99";
        let gh = write_gh_stub(
            tmp.path(),
            &format!("#!/bin/sh\nprintf '%s\\n' '{expected_url}'\n"),
        );

        let status = collect_worker_git_status_with_gh(tmp.path(), &gh);
        assert_eq!(status.pr_url, WorkerPrUrl::Url(expected_url.to_string()));
        let rendered = format_worker_git_status(&status);
        assert!(
            rendered.contains(&format!("PR: {expected_url}")),
            "successful gh URL must be preserved by the renderer: {rendered}"
        );
    }

    #[test]
    fn collect_git_status_reports_unknown_when_branch_ref_is_unfetched() {
        let (tmp, _expected_sha) = setup_git_repo_with_factory_branch("unfetched-worker");
        let missing_gh = tmp.path().join("gh-not-installed");
        let status = collect_worker_git_status_with_gh(tmp.path(), &missing_gh);
        assert_eq!(
            status.pr_url,
            WorkerPrUrl::Unknown("branch not on origin locally")
        );
        let rendered = format_worker_git_status(&status);
        assert!(
            rendered.contains("PR: unknown (branch not on origin locally)"),
            "unfetched branch must be visible as unknown: {rendered}"
        );
        assert!(
            !rendered.contains("PR: none"),
            "unfetched branch must not masquerade as no PR: {rendered}"
        );
    }

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
        assert_eq!(status.pr_url, WorkerPrUrl::Unknown("branch unknown"));
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
            pr_url: WorkerPrUrl::None,
            is_shared_checkout: false,
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
        let (_tmp, project) = setup_factory_project_with_worker_worktrees(&["named-a", "named-b"]);
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

    /// GH #255 round 2: a Codex edit can leave no WorkerFileEdited event at
    /// all. The query-time snapshot is therefore an activity floor, not a
    /// decoration for an already-live event row.
    #[test]
    fn worker_activity_dirty_worktree_floor_is_non_idle_without_events() {
        let (_tmp, project) = setup_factory_project_with_worker_worktrees(&["codex-editor"]);
        let cas_root = project.join(".cas");
        let worktree = cas_root.join("worktrees/codex-editor");
        std::fs::write(worktree.join("README"), "edited without an event\n").unwrap();

        let agent = cas_types::Agent::new_with_role(
            "codex-session".to_string(),
            "codex-editor".to_string(),
            AgentRole::Worker,
        );
        let worker_events: Vec<cas_types::Event> = Vec::new();
        assert!(
            worker_events.is_empty(),
            "fixture intentionally has no events"
        );

        let snapshot = collect_worker_activity_worktree_snapshot(&cas_root, &agent)
            .expect("dirty worktree must floor activity when there are no events");
        let rendered = format_worker_activity_worktree_snapshot(&snapshot);

        assert!(
            !worker_activity_has_no_rows(0, 0, 1, 0, 0),
            "the live floor must bypass worker_activity's empty response"
        );
        assert!(
            !worker_activity_has_no_rows(0, 0, 0, 0, 1),
            "a dead harness must bypass worker_activity's empty response"
        );
        assert!(
            !rendered.contains("No recent worker activity"),
            "a dirty zero-event worker must render as non-idle: {rendered}"
        );
        assert!(
            rendered.contains("1 dirty file"),
            "missing dirty count: {rendered}"
        );
        assert!(
            rendered.contains("diffstat:"),
            "missing diffstat: {rendered}"
        );
        assert!(
            rendered.contains("README"),
            "diffstat must name changed file: {rendered}"
        );
        assert!(
            rendered.contains("last commit:"),
            "missing last commit: {rendered}"
        );
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
        assert!(
            sync_skip_reason_for_clone_resolve(
                "w1",
                &WorkerClonePathResolve::Ready(std::path::PathBuf::from("/x"))
            )
            .is_none()
        );
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

#[cfg(test)]
mod sync_safety_tests {
    //! cas-0a6f (GH #103): sync_all_workers used to rebase every worker
    //! worktree unconditionally — stashing live WIP without consent, stranding
    //! it silently when the pop failed, and leaving worktrees mid-rebase.

    use super::*;
    use std::process::Command;

    fn git(repo: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("GIT_EDITOR", "true")
            .output()
            .unwrap_or_else(|e| panic!("git {args:?} failed to run: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(repo: &std::path::Path, file: &str, contents: &str) {
        std::fs::write(repo.join(file), contents).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", &format!("add {file}")]);
    }

    /// Repo with `main` carrying one commit ahead of the worker branch, so a
    /// rebase onto `main` actually does work.
    fn repo_with_upstream_commit() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().to_path_buf();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@test.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        commit(&repo, "base.txt", "base");
        git(&repo, &["checkout", "-q", "-b", "factory/worker"]);
        git(&repo, &["checkout", "-q", "main"]);
        commit(&repo, "upstream.txt", "upstream work");
        git(&repo, &["checkout", "-q", "factory/worker"]);
        (temp, repo)
    }

    // ---- gate decisions (pure) --------------------------------------------

    #[test]
    fn dirty_worktree_is_skipped_without_force() {
        let gate = sync_gate_for_worker("w1", 3, false, None, false);
        let SyncGate::Refuse(reason) = gate else {
            panic!("a dirty worktree must not be rebased without consent");
        };
        assert!(
            reason.contains("3 uncommitted change(s)") && reason.contains("force=true"),
            "reason must state the count and the way forward: {reason}"
        );
    }

    #[test]
    fn dirty_worktree_proceeds_with_force() {
        assert_eq!(
            sync_gate_for_worker("w1", 3, false, None, true),
            SyncGate::Proceed
        );
    }

    #[test]
    fn worker_mid_task_is_skipped_without_force_and_named() {
        let SyncGate::Refuse(reason) =
            sync_gate_for_worker("w1", 0, false, Some("cas-1234"), false)
        else {
            panic!("rebasing under a working agent needs consent");
        };
        assert!(
            reason.contains("cas-1234"),
            "reason must name the task holding the worktree: {reason}"
        );
        assert_eq!(
            sync_gate_for_worker("w1", 0, false, Some("cas-1234"), true),
            SyncGate::Proceed,
            "force is the documented override"
        );
    }

    #[test]
    fn mid_rebase_worktree_is_refused_even_with_force() {
        for force in [false, true] {
            let SyncGate::Refuse(reason) =
                sync_gate_for_worker("w1", 0, true, Some("cas-1234"), force)
            else {
                panic!("sync must never rebase on top of an unfinished rebase (force={force})");
            };
            assert!(
                reason.contains("MID-REBASE") && reason.contains("rebase --abort"),
                "reason must flag the state and how to clear it: {reason}"
            );
        }
    }

    #[test]
    fn clean_idle_worktree_proceeds() {
        assert_eq!(
            sync_gate_for_worker("w1", 0, false, None, false),
            SyncGate::Proceed
        );
    }

    // ---- branch affinity (cas-5884, pure) ---------------------------------

    #[test]
    fn standalone_worker_is_skipped_by_an_epic_wide_sweep() {
        let SyncGate::Refuse(reason) = sync_affinity_gate(
            "jolly-salmon-55",
            Some("cas-0c0a"),
            &BranchAffinity::Trunk,
            "epic/cas-2627",
            "main",
            false,
        ) else {
            panic!("a trunk-targeted standalone worker must not be rebased onto an epic branch");
        };
        assert!(
            reason.contains("branch affinity mismatch")
                && reason.contains("cas-0c0a")
                && reason.contains("main")
                && reason.contains("epic/cas-2627")
                && reason.contains("worker_names="),
            "reason must name the task, both branches, and the override: {reason}"
        );
    }

    #[test]
    fn epic_lane_worker_still_syncs_to_its_own_epic_branch() {
        assert_eq!(
            sync_affinity_gate(
                "w1",
                Some("cas-1234"),
                &BranchAffinity::Branch("epic/cas-2627".into()),
                "epic/cas-2627",
                "main",
                false,
            ),
            SyncGate::Proceed
        );
    }

    #[test]
    fn worker_affinity_uses_its_epics_recorded_parent_branch_cas_580e() {
        let temp = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let store = crate::store::open_task_store(&cas_dir).unwrap();
        let mut epic = cas_types::Task::new("cas-580e-epic".into(), "stacked epic".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.branch = Some("epic/stale-base".into());
        epic.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:active-project".into(),
            target_branch: "main".into(),
        });
        store.add(&epic).unwrap();

        let mut child = cas_types::Task::new("cas-580e-child".into(), "worker task".into());
        child.assignee = Some("worker".into());
        store.add(&child).unwrap();
        store
            .add_dependency(&cas_types::Dependency {
                from_id: child.id.clone(),
                to_id: epic.id.clone(),
                dep_type: cas_types::DependencyType::ParentChild,
                created_at: chrono::Utc::now(),
                created_by: None,
            })
            .unwrap();

        assert_eq!(
            task_branch_affinity(store.as_ref(), &child),
            BranchAffinity::Branch("main".into()),
            "sync must use the epic's declared integration parent, not its stale coordination ref"
        );
    }

    #[test]
    fn worker_on_a_different_epic_is_skipped() {
        let SyncGate::Refuse(reason) = sync_affinity_gate(
            "w1",
            Some("cas-1234"),
            &BranchAffinity::Branch("epic/cas-9999".into()),
            "epic/cas-2627",
            "main",
            false,
        ) else {
            panic!("a worker on another epic's lane must not be rebased onto this epic");
        };
        assert!(reason.contains("epic/cas-9999"), "{reason}");
    }

    #[test]
    fn standalone_worker_syncs_when_the_target_is_trunk() {
        for target in ["main", "origin/main", "refs/heads/main"] {
            assert_eq!(
                sync_affinity_gate(
                    "w1",
                    Some("cas-0c0a"),
                    &BranchAffinity::Trunk,
                    target,
                    "main",
                    false
                ),
                SyncGate::Proceed,
                "trunk sweep must still reach standalone workers (target={target})"
            );
        }
    }

    #[test]
    fn worker_without_a_task_has_no_affinity_constraint() {
        assert_eq!(
            sync_affinity_gate(
                "w1",
                None,
                &BranchAffinity::Unknown,
                "epic/cas-2627",
                "main",
                false
            ),
            SyncGate::Proceed
        );
    }

    #[test]
    fn explicit_worker_names_targeting_overrides_affinity() {
        assert_eq!(
            sync_affinity_gate(
                "w1",
                Some("cas-0c0a"),
                &BranchAffinity::Trunk,
                "epic/cas-2627",
                "main",
                true,
            ),
            SyncGate::Proceed,
            "naming a worker explicitly is the documented override"
        );
    }

    #[test]
    fn branch_refs_normalize_across_remote_and_fully_qualified_forms() {
        assert_eq!(normalize_branch_ref("refs/heads/epic/x"), "epic/x");
        assert_eq!(normalize_branch_ref("origin/epic/x"), "epic/x");
        assert_eq!(normalize_branch_ref("refs/remotes/origin/main"), "main");
        assert_eq!(normalize_branch_ref("  main  "), "main");
    }

    // ---- git-level behaviour ----------------------------------------------

    #[test]
    fn dirty_count_and_rebase_probe_read_real_worktree_state() {
        let (_temp, repo) = repo_with_upstream_commit();
        assert_eq!(dirty_file_count(&repo).unwrap(), 0);
        assert!(!rebase_in_progress(&repo));

        std::fs::write(repo.join("wip.txt"), "uncommitted").unwrap();
        assert_eq!(
            dirty_file_count(&repo).unwrap(),
            1,
            "untracked WIP counts as dirty — it is exactly what auto-stash would sweep up"
        );
    }

    #[test]
    fn clean_sync_rebases_without_touching_a_stash() {
        let (_temp, repo) = repo_with_upstream_commit();
        let details = sync_worker_clone(&repo, "main").expect("clean rebase should succeed");
        assert_eq!(details, "rebased cleanly");
        assert!(
            git(&repo, &["stash", "list"]).is_empty(),
            "nothing should have been stashed"
        );
        assert!(
            repo.join("upstream.txt").exists(),
            "sync should have landed"
        );
    }

    #[test]
    fn stash_pop_failure_reports_the_stash_ref_and_does_not_lose_wip() {
        let (_temp, repo) = repo_with_upstream_commit();

        // This worker may already have unrelated stashed work. The recovery
        // must name the new stash object, not rely on a position in its reflog.
        std::fs::write(repo.join("unrelated.txt"), "unrelated prior WIP").unwrap();
        git(
            &repo,
            &[
                "stash",
                "push",
                "--include-untracked",
                "-m",
                "unrelated prior WIP",
            ],
        );
        let unrelated_stash = git(&repo, &["rev-parse", "refs/stash"]);

        // WIP that collides with the incoming upstream commit: the rebase
        // succeeds (the file is untracked locally) and the pop then fails.
        std::fs::write(repo.join("upstream.txt"), "local uncommitted version").unwrap();

        let failure =
            sync_worker_clone(&repo, "main").expect_err("stash pop must fail on this collision");
        assert!(
            failure.message.contains("stash pop failed"),
            "failure must say what happened: {}",
            failure.message
        );
        let stash_ref = failure
            .stranded_stash
            .as_deref()
            .expect("a stranded stash must be reported with its ref");
        assert!(
            !stash_ref.contains(' '),
            "the ref is spliced into a shell command — it must be a single token, got {stash_ref:?}"
        );
        let reported_sha = stash_ref
            .strip_suffix("^0")
            .expect("the reported SHA must be disambiguated from a numeric stash index");
        let current_stash = git(&repo, &["rev-parse", "refs/stash"]);
        assert_eq!(
            reported_sha,
            &current_stash[..reported_sha.len()],
            "the recovery token must identify the stash created by this sync"
        );
        assert_ne!(
            reported_sha,
            &unrelated_stash[..reported_sha.len()],
            "the reported stash must not point at the worker's unrelated prior WIP"
        );
        assert_eq!(
            git(&repo, &["stash", "list"]).lines().count(),
            2,
            "the fixture must exercise recovery with an unrelated pre-existing stash"
        );

        let line = failure.report_line();
        assert!(
            line.contains("WIP IS NOT LOST") && line.contains(stash_ref),
            "report line must carry recovery instructions with the ref: {line}"
        );

        // Walk the documented recovery for real, in order.
        //
        // 1. `apply` refuses while the colliding file is present — that
        //    collision is exactly why the pop failed, and the guidance says so
        //    rather than sending the operator into the same wall.
        let blocked = Command::new("git")
            .args(["stash", "apply", stash_ref])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            !blocked.status.success(),
            "precondition: the collision that stranded the stash still blocks apply"
        );
        // 2. Move the collision aside as instructed, then apply succeeds and
        //    the WIP comes back. (`pop` is never instructed: git rejects a bare
        //    SHA there with "is not a stash reference".)
        std::fs::rename(repo.join("upstream.txt"), repo.join("upstream.rebased")).unwrap();
        let applied = Command::new("git")
            .args(["stash", "apply", stash_ref])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            applied.status.success(),
            "after the documented step the recovery must succeed: {}",
            String::from_utf8_lossy(&applied.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("upstream.txt")).unwrap(),
            "local uncommitted version",
            "the worker's WIP must be back on disk"
        );

        // The stash really is still there, and really does hold the WIP.
        assert!(
            !git(&repo, &["stash", "list"]).is_empty(),
            "the stash entry must survive for recovery"
        );
        // `--include-untracked` is required in the inspect command: the WIP was
        // untracked, and plain `git stash show -p` prints nothing for it —
        // which reads as "my work is gone".
        assert!(
            git(
                &repo,
                &["stash", "show", "-p", "--include-untracked", stash_ref]
            )
            .contains("local uncommitted version"),
            "the stranded stash must contain the worker's WIP"
        );
        // Scope the `pop` check to the recovery instruction: the diagnostic
        // half of the line legitimately quotes the git command that failed.
        let guidance = line
            .split("recover with")
            .nth(1)
            .expect("report must contain recovery guidance");
        assert!(
            guidance.contains("--include-untracked") && !guidance.contains("stash pop"),
            "guidance must inspect untracked WIP and must not use `pop`, which rejects a SHA: {guidance}"
        );
    }

    #[test]
    fn durable_stash_ref_disambiguates_an_all_decimal_sha_from_a_stash_index() {
        let decimal_sha = "1234567890123456789012345678901234567890";
        assert_eq!(
            durable_stash_ref(decimal_sha),
            "123456789012^0",
            "Git stash treats a bare all-decimal token as stash@{{N}}, not a commit"
        );
    }

    #[test]
    fn failed_rebase_restores_wip_and_reports_no_stranded_stash() {
        let (_temp, repo) = repo_with_upstream_commit();

        // Make the rebase itself conflict: commit a change to a file the
        // upstream commit also introduces.
        std::fs::write(repo.join("upstream.txt"), "worker version").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "worker version"]);
        // Plus recoverable WIP on top.
        std::fs::write(repo.join("wip.txt"), "worker wip").unwrap();

        let failure = sync_worker_clone(&repo, "main").expect_err("conflicting rebase must fail");
        assert!(
            failure.message.contains("rebase failed"),
            "failure must name the phase: {}",
            failure.message
        );
        assert!(
            !failure.mid_rebase,
            "the abort succeeded, so the worktree must not be flagged mid-rebase"
        );
        assert!(
            failure.stranded_stash.is_none(),
            "the stash was popped back, so nothing is stranded"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("wip.txt")).unwrap(),
            "worker wip",
            "the worker's uncommitted WIP must be back in the worktree"
        );
        assert!(
            !rebase_in_progress(&repo),
            "the worktree must be left usable"
        );
    }

    #[test]
    fn report_line_flags_a_worktree_left_mid_rebase() {
        let failure = SyncFailure {
            message: "rebase failed: conflict".to_string(),
            stranded_stash: Some("abc123def456 (stash@{0} at sync time)".to_string()),
            mid_rebase: true,
        };
        let line = failure.report_line();
        assert!(
            line.contains("WORKTREE LEFT MID-REBASE") && line.contains("rebase --abort"),
            "an unfinished rebase must be explicit in the report: {line}"
        );
        assert!(
            line.contains("abc123def456"),
            "the stash ref must ride along: {line}"
        );
    }
}
