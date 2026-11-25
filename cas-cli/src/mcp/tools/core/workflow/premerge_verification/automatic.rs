//! Daemon-owned transport for the existing task verifier. Only the final
//! verdict arguments cross the harness boundary; authority stays server-side.
use super::*;
use std::process::Stdio;

const OWNER_KEY: &str = cas_store::AUTOMATIC_PREMERGE_OWNER_KEY;
const PROOF_DRIFT_CODE: &str = "verifier_dispatch_proof_drift";
const REPORT_PROOF_MISSING_CODE: &str = "verifier_dispatch_report_proof_missing";

pub(super) fn owns(root: &Path, dispatch: &VerificationDispatch) -> bool {
    dispatch.verifier_agent_id.as_deref().is_some_and(|id| {
        crate::store::open_agent_store(root)
            .ok()
            .and_then(|store| store.get(id).ok())
            .is_some_and(|agent| agent.metadata.get(OWNER_KEY) == Some(&dispatch.id))
    })
}

pub(crate) fn prompt(core: &CasCore, task: &Task, dispatch: &VerificationDispatch) -> Result<String, String> {
    let store = core.open_task_store().map_err(|e| e.to_string())?;
    let parent = store.get_parent_epic(&task.id).map_err(|e| e.to_string())?;
    let recorded = crate::mcp::tools::core::task::lifecycle::report_proof::ReportProof::recorded(&core.cas_root, &dispatch.id)?;
    if recorded.is_none() && crate::mcp::tools::core::task::lifecycle::report_proof::is_report(task) {
        return Err(format!("{REPORT_PROOF_MISSING_CODE}: dispatch {} has no recorded report proof", dispatch.id));
    }
    // No-code close reviews bypass candidate preparation. Resolve committed
    // proof here, on the common launch path, before constructing the verifier
    // command. fetch_task checks task scope, revocation, size and SHA-256 and
    // derives this receiver's project namespace instead of a publisher path.
    let committed = crate::artifacts::proof::committed_task_artifacts(&core.cas_root, &task.id)
        .map_err(|error| error.to_string())?;
    let drift = |id: &str| format!(
        "{PROOF_DRIFT_CODE}: artifact {id} no longer matches the proof recorded on dispatch {}", dispatch.id
    );
    let mut reviewed = task.clone();
    let inventory = match recorded.as_ref() {
        Some(proof) => {
            reviewed.external_ref = proof.external_ref.clone();
            if let Some((id, _)) = proof.artifacts.iter().find(|(id, sha256)| {
                committed.iter().all(|artifact| &artifact.id != id || &artifact.sha256 != sha256)
            }) {
                return Err(drift(id));
            }
            proof.artifacts.clone()
        }
        None => committed.into_iter().map(|artifact| (artifact.id, artifact.sha256)).collect(),
    };
    let task = &reviewed;
    let mut artifacts = Vec::new();
    for (id, sha256) in inventory {
        let (record, path) = crate::artifacts::proof::fetch_task(&core.cas_root, &task.id, &id)
            .map_err(|error| {
                let drifted = error.downcast_ref::<crate::artifacts::ArtifactRevoked>().is_some()
                    || error.downcast_ref::<crate::artifacts::slack::PostError>().is_some_and(|download| {
                        matches!(download.code.as_str(), "artifact_checksum_mismatch" | "artifact_size_mismatch")
                    });
                if recorded.is_some() && drifted {
                    drift(&id)
                } else {
                    format!("Verifier artifact {id} unavailable: {error:#}")
                }
            })?;
        if recorded.is_some() && (record.id != id || record.sha256 != sha256) {
            return Err(drift(&id));
        }
        artifacts.push(serde_json::json!({
            "task_id":record.task_id, "artifact_id":record.id, "path":path, "sha256":record.sha256,
        }));
    }
    artifacts.extend(materialize_parent_plan_artifacts(&core.cas_root, task)?);
    let inherited = candidate_for_dispatch(&core.cas_root, dispatch)?
        .and_then(|candidate| candidate.inherited_supervisor_call);
    let supervisor_calls = core.open_verification_store().map_err(|e| e.to_string())?
        .get_for_task(&task.id).map_err(|e| e.to_string())?.into_iter()
        .filter(|call| call.dispatch_id.as_deref() == Some(dispatch.id.as_str())
            && call.status == VerificationStatus::SupervisorCall)
        .collect::<Vec<_>>();
    let inputs = serde_json::json!({
        "task": task, "parent": parent, "dispatch": dispatch,
        "inherited_supervisor_call": inherited,
        "qa": {"mode": core.load_config().qa().mode},
        "supervisor_calls": supervisor_calls,
        "artifacts": artifacts,
    });
    let mut input = serde_json::to_string(&inputs).map_err(|e| e.to_string())?;
    // GH #2047: history grows on every repair round. Keep every source byte,
    // but move large context out of the harness's 1 MiB turn/start request.
    // A content-addressed file keeps an attempt's exact inputs immutable.
    if input.len() > 262_144 {
        let directory = core.cas_root.join("verification-runs").join(&dispatch.id);
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let path = directory.join(format!("review-input-{:x}.json", Sha256::digest(input.as_bytes())));
        write_once(&path, &inputs)?;
        input = serde_json::json!({"review_input_file": path}).to_string();
    }
    let body = format!(
        "{}\n\n{}\n\nDaemon-owned task-verifier invocation. The task and parent below include acceptance criteria, receipts and supervisor adversarial notes. Review the bound candidate using read-only filesystem commands. Do not edit, register, close, spawn agents, or call MCP. Return exactly one JSON object containing the arguments to verification action=add: status (approved, rejected, supervisor_call, or error), summary, confidence, files_reviewed (comma-separated), issues (JSON-encoded array). Omit task_id, dispatch_id and verifier_capability: the daemon supplies the immutable dispatch identity and records your verdict through the existing sealed handoff. This daemon-owned transport replaces the MCP identity check and recording steps above; the daemon authenticates your distinct child and exact dispatch. Use qa.mode in the structured review context below. In batch mode, approve when the only open evidence is a NOT EXERCISED journey assigned solely to release batch QA and every other criterion is VERIFIED; record each deferred row and its owner. Other missing evidence, per_task unexercised journeys and any question beyond that policy still require supervisor_call. Never waive rejection conditions or treat a deferred walk as PASS.\nAn inherited supervisor call is present only when inherited_supervisor_call is not null. It carries a prior answer for an unchanged delivery and must be applied only to that exact NOT EXERCISED row/question. Cite that inherited answer in your verdict; if the current question is different, make a new supervisor_call.\nTask evidence directory (LEDGER.md, logs and captures; read this exact path instead of deriving <project-key>): {}. If evidence is absent there, name this absolute path in the verdict.\nReview inputs:\n{}\nIf review_input_file is set, read that exact JSON file before reviewing; it contains the complete task, parent, dispatch, inherited answer and review context. Read large histories in sections as needed. The file replaces inline context, without dropping criteria or proof.\n",
        include_str!("../../../../../builtins/agents/task-verifier.body.md"),
        include_str!(
            "../../../../../builtins/skills/cas-qa-craft/references/verifier-evidence-gate.md"
        ),
        crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(
            &core.cas_root,
            task,
        )?
        .display(),
        input,
    );
    Ok(format!("{body}\nThe supervisor_calls array is this dispatch's persisted questions and answers. Apply supervisor_direct answers as scope context, then independently review and return the final verdict; an answer is not an approval.\n"))
}

/// The account spawn_workers would launch (cas-c386). With no eligible home,
/// refuse with each home's state rather than blame the default home.
fn launch_account(cli: cas_mux::SupervisorCli, root: &Path) -> Result<Option<String>, String> {
    let provider = match cli {
        cas_mux::SupervisorCli::Claude => crate::accounts::Provider::Claude,
        cas_mux::SupervisorCli::Codex => crate::accounts::Provider::Codex,
        _ => return Ok(None),
    };
    let Some(accounts) = dirs::home_dir()
        .and_then(|home| crate::account_registry::collect_for(provider, &home, false).ok())
    else {
        return Ok(None);
    };
    let policy = crate::account_selection::load_policy(Some(root));
    let selection =
        crate::account_selection::launch_choice(&accounts, provider, chrono::Utc::now(), &policy);
    if let Some(choice) = selection.recommended {
        return Ok(Some(choice.account.name.clone()));
    }
    let homes = selection
        .excluded
        .iter()
        .map(|(account, why)| format!("{}: {why}", account.folder.display()))
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "no eligible {} account ({})",
        provider.name(),
        if homes.is_empty() { "no accounts found" } else { &homes }
    ))
}

/// The verifier command, plus a notice when it fell back to Claude.
fn command(root: &Path, output: &Path) -> Result<(Command, Option<String>), String> {
    let route = cas_factory::resolve_lane("standard", &cas_factory::CapabilitySnapshot::default())
        .map_err(|e| e.to_string())?;
    let mut cli = route.spec.cli;
    let mut model = route.spec.model.ok_or("verifier route has no model")?;
    let mut effort = route.spec.effort.ok_or("verifier route has no effort")?;
    // A daemon may have no provider account selector, even when the host has
    // authenticated named accounts. Choose the account spawn_workers would
    // (cas-c386), then use the checked launch environment supervisors use,
    // rather than inheriting the daemon's default home. With no eligible
    // Codex home, the same verifier runs on an eligible Claude account
    // (cas-bdde); only when neither has one is the supervisor asked.
    let mut notice = None;
    let account = match launch_account(cli, root) {
        Ok(account) => account,
        Err(codex) if cli == cas_mux::SupervisorCli::Codex => {
            match launch_account(cas_mux::SupervisorCli::Claude, root) {
                Ok(Some(account)) => {
                    let recipe = cas_factory::routing::registry()
                        .map_err(|e| e.to_string())?
                        .recipes
                        .get("claude_opus_5_5")
                        .ok_or("verifier Claude recipe missing")?;
                    cli = cas_mux::SupervisorCli::Claude;
                    model = recipe.model.clone();
                    effort = recipe.default_effort;
                    notice = Some(format!(
                        "Automatic verifier started on Claude account {account} ({model}, {}): {codex}.",
                        effort.as_str()
                    ));
                    Some(account)
                }
                Ok(None) => {
                    return Err(format!(
                        "{codex}; Claude accounts could not be read"
                    ));
                }
                Err(claude) => {
                    return Err(format!("{codex}; {claude}"));
                }
            }
        }
        Err(_) => None,
    };
    let environment =
        crate::hub::launch_env::resolve_for_project(cli, account.as_deref(), Some(root))
            .map_err(|e| e.to_string())?;
    let mut command = Command::new(environment.executable);
    for key in environment.remove {
        command.env_remove(key);
    }
    command.envs(environment.set);
    match cli {
        cas_mux::SupervisorCli::Codex => {
            sandbox_preflight()?;
            command.args(codex_exec_args(&model, effort.as_str(), output));
        }
        cas_mux::SupervisorCli::Claude => {
            command.args(["-p", "--model"]).arg(model).args([
                "--effort",
                effort.as_str(),
                "--allowedTools",
                "Read,Grep,Glob,Bash",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
            ]);
        }
        other => return Err(format!("unsupported verifier harness: {other:?}")),
    }
    crate::internal_llm::isolate_command(&mut command);
    for key in ["CAS_AGENT_ID", "CLAUDECODE", "CODEX_THREAD_ID"] {
        command.env_remove(key);
    }
    Ok((command, notice))
}

/// The Codex verifier's `exec` arguments, after the sandbox preflight.
fn codex_exec_args(model: &str, effort: &str, output: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = ["exec", "--sandbox", "read-only", "--skip-git-repo-check", "--model"]
        .map(std::ffi::OsString::from)
        .into();
    args.push(model.into());
    for value in [
        format!("model_reasoning_effort=\"{effort}\""),
        "project_doc_max_bytes=0".into(),
        // GH #2023: `exec` runs with approval: never, which refuses every MCP
        // call that needs approval. Pre-approve only Cassy's canonical server;
        // its own identity checks still apply. Command and args keep the
        // table valid when the account's config has no `cs` server.
        "mcp_servers.cs.command=\"cas\"".into(),
        "mcp_servers.cs.args=[\"serve\"]".into(),
        "mcp_servers.cs.default_tools_approval_mode=\"approve\"".into(),
    ] {
        args.push("-c".into());
        args.push(value.into());
    }
    args.push("--output-last-message".into());
    args.push(output.into());
    args.push("-".into());
    args
}

/// Probe the same namespace primitive before paying for a model that cannot read.
fn sandbox_preflight() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use crate::bounded_process::{Deadline, run_command};
        let output = run_command(
            Command::new("bwrap").args(["--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev", "--", "/bin/true"]),
            Deadline::after(std::time::Duration::from_secs(5)), std::time::Duration::from_secs(5),
        ).map_err(|e| format!("verifier_bwrap_preflight: probe_start_failed: {e:?}; install bubblewrap and inspect the launch context"))?;
        if !output.status.success() {
            let read = |path| std::fs::read_to_string(path).ok().map(|s| s.trim().to_string());
            let uid_map = read("/proc/self/uid_map");
            let initial = uid_map.as_deref().is_some_and(|map| {
                map.split_whitespace().eq(["0", "0", "4294967295"])
            });
            let status = read("/proc/self/status").unwrap_or_default().lines()
                .filter(|line| line.starts_with("NoNewPrivs:") || line.starts_with("Seccomp:") || line.starts_with("CapBnd:"))
                .map(str::to_string).collect::<Vec<_>>();
            return Err(format!("verifier_bwrap_preflight: {}", serde_json::json!({
                "cause": if initial { "host_namespace_policy" } else { "nested_user_namespace_context" },
                "pid": std::process::id(), "status": status, "uid_map": uid_map,
                "user_namespace": std::fs::read_link("/proc/self/ns/user").ok(),
                "apparmor_profile": read("/proc/self/attr/current"),
                "max_user_namespaces": read("/proc/sys/user/max_user_namespaces"),
                "kernel.unprivileged_userns_clone": read("/proc/sys/kernel/unprivileged_userns_clone"),
                "kernel.apparmor_restrict_unprivileged_userns": read("/proc/sys/kernel/apparmor_restrict_unprivileged_userns"),
                "stderr": tail(&output.stderr),
                "remedy": if initial {
                    "Inspect the named host user-namespace limits and AppArmor policy; keep the read-only sandbox enabled"
                } else {
                    "Launch the verifier from the factory daemon outside worker store isolation; do not disable either sandbox or change host policy"
                }
            })));
        }
    }
    Ok(())
}

const LAUNCH_ATTEMPTS: u32 = 4;
const LAUNCH_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(15);
/// cas-111e: contention waits double from the retry delay up to this cap and
/// stop after the window, without spending launch attempts.
const CONTENTION_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(300);
const CONTENTION_WINDOW: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// How the request side treats a refused launch (cas-4fd6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Retry {
    /// Retrying cannot change the answer.
    Never,
    /// A wait that clears on its own; queue behind it without spending an attempt.
    Contended,
    /// Anything else: a bounded number of attempts with a growing delay.
    Backoff,
}

/// A refused launch and what retrying it can achieve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LaunchError {
    pub retry: Retry,
    pub message: String,
}

/// The daemon names each refusal with a leading code. The code, matched whole,
/// is the classification; the message text after it is never searched.
pub(super) fn retry_for_code(message: &str) -> Retry {
    match refusal_code(message) {
        PROOF_DRIFT_CODE
        | REPORT_PROOF_MISSING_CODE
        | "verifier_factory_scope"
        | "verifier_factory_task_terminal"
        | "verifier_factory_owner"
        | "verifier_factory_unsupported"
        | "verifier_handoff_not_active" => Retry::Never,
        "verifier_factory_store_busy" | "verifier_factory_launch_dropped" => Retry::Contended,
        _ => Retry::Backoff,
    }
}

fn refusal_code(message: &str) -> &str {
    message.split_once(':').map_or(message, |(code, _)| code).trim()
}

/// A store refusal with its code, by the typed error: contention is named
/// `verifier_factory_store_busy`, everything else keeps its own text.
pub(super) fn store_refusal(error: &cas_store::StoreError) -> String {
    use cas_store::StoreError;
    let contended = match error {
        StoreError::Database(sqlite) => cas_store::shared_db::is_busy_error(sqlite),
        StoreError::Io(io) => io.kind() == std::io::ErrorKind::WouldBlock,
        StoreError::VerifierHandoffPending => true,
        _ => false,
    };
    if contended {
        format!("verifier_factory_store_busy: {error}")
    } else {
        error.to_string()
    }
}

/// cas-d824: a failed start is re-attempted here, on the thread that already
/// owns the request, rather than waiting for an operator to repeat the merge.
/// A refusal is classified by its typed [`Retry`], not by its text: a
/// permanent one stops at once, contention queues behind its holder with a
/// doubling delay for up to `window` and spends no attempt (cas-111e), and
/// anything else is retried with a growing delay. Returns the attempts spent
/// and the true blocker.
fn request_with_retry(
    attempts: u32,
    delay: std::time::Duration,
    window: std::time::Duration,
    mut request: impl FnMut() -> Result<(), LaunchError>,
) -> Result<(), (u32, String)> {
    let started = std::time::Instant::now();
    let (mut attempt, mut waits) = (1, 0u32);
    loop {
        match request() {
            Ok(()) => return Ok(()),
            Err(LaunchError { retry, message: error }) => {
                let permanent = retry == Retry::Never;
                if retry == Retry::Contended {
                    let pause = delay.saturating_mul(1 << waits.min(16)).min(CONTENTION_BACKOFF_CAP);
                    if started.elapsed() + pause >= window {
                        return Err((attempt, format!(
                            "{error}; still contended after {} s and {} waits",
                            started.elapsed().as_secs(), waits + 1,
                        )));
                    }
                    std::thread::sleep(pause);
                    waits += 1;
                    continue;
                }
                if permanent || attempt >= attempts {
                    return Err((attempt, error));
                }
                std::thread::sleep(delay * attempt);
                attempt += 1;
            }
        }
    }
}

pub(super) fn recover_expired_claim(core: &CasCore, dispatch: &VerificationDispatch) -> Result<VerificationDispatch, String> {
    recover_expired_claim_with_probes(core, dispatch,
        crate::mcp::daemon::pid_alive, crate::mcp::daemon::read_pid_starttime)
}

pub(super) fn recover_expired_claim_with_probes(
    core: &CasCore, dispatch: &VerificationDispatch,
    pid_alive: impl Fn(u32) -> bool, read_starttime: impl Fn(u32) -> Option<u64>,
) -> Result<VerificationDispatch, String> {
    let now = chrono::Utc::now();
    if dispatch.deadline_at > now
        || !matches!(dispatch.state, cas_types::VerificationDispatchState::Claimed | cas_types::VerificationDispatchState::TimedOut)
    { return Ok(dispatch.clone()); }
    let agents = core.open_agent_store().map_err(|e| e.to_string())?;
    let owner = agents.get(&core.get_agent_id().map_err(|e| e.to_string())?).map_err(|e| store_refusal(&e))?;
    if owner.id != dispatch.owner_agent_id || owner.role != cas_types::AgentRole::Supervisor {
        return Ok(dispatch.clone());
    }
    let Some(child) = dispatch.verifier_agent_id.as_deref().and_then(|id| agents.get(id).ok()) else {
        return Ok(dispatch.clone());
    };
    let machine = cas_types::Agent::get_or_generate_machine_id();
    let daemon_owned = child.metadata.get(OWNER_KEY) == Some(&dispatch.id);
    let local = child.machine_id.as_deref() == Some(machine.as_str())
        || (child.machine_id.is_none() && daemon_owned && owner.machine_id.as_deref() == Some(machine.as_str()));
    if !local {
        return Ok(dispatch.clone());
    }
    let stalled = daemon_owned
        && !crate::factory_review_scratch::verifier_progressing(&core.cas_root, dispatch, Some(&child), now);
    let confirmed_dead = stalled || child.status == cas_types::AgentStatus::Shutdown
        || child.pid.is_some_and(|pid| {
            if !pid_alive(pid) { return true; }
            child.pid_starttime.or_else(|| child.metadata
                .get(crate::mcp::daemon::PID_STARTTIME_KEY).and_then(|value| value.parse::<u64>().ok()))
                .is_some_and(|expected| read_starttime(pid).is_some_and(|actual| actual != expected))
        });
    if !confirmed_dead { return Ok(dispatch.clone()); }
    cas_store::recover_expired_dead_verifier_dispatch(&core.cas_root, &dispatch.id, &owner, &child, now)
        .map_err(|e| store_refusal(&e))
}

/// Claim synchronously, then do provider work off the MCP/control loop. The
/// existing dispatch claim prevents a second launch, including manual races.
pub(super) fn launch_dispatch(root: &Path, id: &str, session: Option<&str>) -> Result<VerificationDispatch, String> {
    let dispatch = cas_store::get_verification_dispatch(root, id).map_err(|e| store_refusal(&e))?;
    let agents = crate::store::open_agent_store(root).map_err(|e| e.to_string())?;
    if agents.get(&dispatch.owner_agent_id).is_ok() {
        return Ok(dispatch);
    }
    let machine = cas_types::Agent::get_or_generate_machine_id();
    let retired = cas_store::SqliteAgentStore::open(root).map_err(|e| store_refusal(&e))?;
    let dead = |agent: &cas_types::Agent| {
        agent.machine_id.as_deref() == Some(machine.as_str())
            && (agent.status == cas_types::AgentStatus::Shutdown || matches!(
                crate::mcp::daemon::evaluate_liveness(agent,
                    crate::mcp::daemon::pid_alive, crate::mcp::daemon::pid_matches_fingerprint),
                crate::mcp::daemon::LivenessOutcome::Dead { .. }
            ))
    };
    for successor in agents.list(None).map_err(|e| store_refusal(&e))? {
        if successor.role != cas_types::AgentRole::Supervisor || !successor.is_alive()
            || successor.machine_id.as_deref() != Some(machine.as_str()) || dead(&successor)
            || session.is_some_and(|session| successor.factory_session.as_deref() != Some(session))
        { continue; }
        if retired.supervisor_handoffs_claimed_by(&successor).map_err(|e| store_refusal(&e))?
            .iter().any(|handoff| handoff.agent_id == dispatch.owner_agent_id)
        {
            let verifier = dispatch.verifier_agent_id.as_deref()
                .and_then(|id| agents.get(id).ok()).filter(&dead);
            return cas_store::adopt_retired_verification_dispatch(
                root, id, &successor, verifier.as_ref(), chrono::Utc::now(),
            ).map_err(|e| store_refusal(&e));
        }
    }
    Err("verification_owner_handoff_scope: missing owner's claimed local successor is unavailable".into())
}

pub(crate) fn launch(
    core: &CasCore, task: &Task, dispatch: &VerificationDispatch,
) -> Result<Option<std::thread::JoinHandle<()>>, String> {
    let dispatch = launch_dispatch(&core.cas_root, &dispatch.id, None)?;
    #[cfg(unix)]
    if let Some(session) = core.open_agent_store().map_err(|e| e.to_string())?
        .get(&dispatch.owner_agent_id).map_err(|e| store_refusal(&e))?.factory_session
    {
        // The MCP backend may inherit the worker's userns. Only the factory
        // daemon owns the context from which the harness sandbox can start.
        let worker = core.clone();
        let dispatch = dispatch.clone();
        return std::thread::Builder::new().name(format!("verifier-request-{}", task.id))
            .spawn(move || {
                // cas-5d2e: the same retry guidance as a local launch failure.
                if let Err((attempts, error)) = request_with_retry(
                    LAUNCH_ATTEMPTS,
                    LAUNCH_RETRY_DELAY,
                    CONTENTION_WINDOW,
                    || super::factory_launch::request(&session, &dispatch.id),
                ) {
                    let _ = attention(&worker, &dispatch, &format!(
                        "Automatic verifier could not start: {error} (launch attempt {attempts} of {LAUNCH_ATTEMPTS}). {}",
                        recovery(&worker.cas_root, &dispatch)
                    ));
                }
            }).map(Some).map_err(|e| e.to_string());
    }
    launch_local(core, task, &dispatch)
}

pub(super) fn launch_local(
    core: &CasCore,
    task: &Task,
    dispatch: &VerificationDispatch,
) -> Result<Option<std::thread::JoinHandle<()>>, String> {
    if cas_store::get_verification_dispatch(&core.cas_root, &dispatch.id)
        .map_err(|e| store_refusal(&e))?.state != cas_types::VerificationDispatchState::Pending {
        return Ok(None);
    }
    // A candidate review reads its disposable candidate; a no-code close
    // dispatch has no repository proof and reviews from the project checkout.
    let worktree = match dispatch.repository.as_ref() {
        Some(proof) => PathBuf::from(&proof.worktree_root),
        None => core.cas_root.parent().unwrap_or(&core.cas_root).to_path_buf(),
    };
    // A replacement must not read an earlier attempt's verdict or overwrite
    // its stderr. The exact dispatch and candidate remain unchanged.
    let directory = core.cas_root.join("verification-runs").join(&dispatch.id)
        .join(format!("attempt-{:032x}", rand::random::<u128>()));
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let output = directory.join("verdict.json");
    let root = core.cas_root.clone();
    launch_with_preparation(core, task, dispatch, output, move |output| {
        let (mut command, notice) = command(&root, output)?;
        command.current_dir(worktree);
        Ok((command, notice))
    })
}

pub(crate) fn launch_with_preparation(
    core: &CasCore,
    task: &Task,
    dispatch: &VerificationDispatch,
    output: PathBuf,
    prepare: impl FnOnce(&Path) -> Result<(Command, Option<String>), String> + Send + 'static,
) -> Result<Option<std::thread::JoinHandle<()>>, String> {
    let live = cas_store::get_verification_dispatch(&core.cas_root, &dispatch.id)
        .map_err(|e| store_refusal(&e))?;
    if live.state != cas_types::VerificationDispatchState::Pending {
        return Ok(None);
    }
    let mut child = cas_types::Agent::new_sub_agent(
        format!("task-verifier-{:032x}", rand::random::<u128>()),
        "task-verifier".into(),
        live.owner_agent_id.clone(),
    );
    // The daemon owns this identity and submits the harness's structured
    // verdict. The subprocess never receives a credential or this identity.
    child.pid = Some(std::process::id());
    child.machine_id = Some(cas_types::Agent::get_or_generate_machine_id());
    crate::mcp::daemon::stamp_pid_fingerprint(&mut child, std::process::id());
    child.metadata.insert(OWNER_KEY.into(), live.id.clone());
    child.factory_session = core
        .open_agent_store()
        .map_err(|e| e.to_string())?
        .get(&live.owner_agent_id)
        .map_err(|e| store_refusal(&e))?
        .factory_session;
    // One transaction: a bind lost to contention must not strand a pending
    // handoff that refuses every later start for this owner (cas-111e).
    cas_store::issue_and_bind_server_verifier_handoff(
        &core.cas_root,
        &task.id,
        &live.id,
        &format!("daemon:{}", live.id),
        &child,
    )
    .map_err(|e| store_refusal(&e))?;
    // Issuing the handoff may have refreshed an expired pending deadline; the
    // prompt, review budget and recovery guard must use the refreshed row.
    let live = cas_store::get_verification_dispatch(&core.cas_root, &live.id)
        .map_err(|e| store_refusal(&e))?;
    let worker = core.clone();
    let task = task.clone();
    let handle = std::thread::Builder::new().name(format!("verifier-{}", task.id)).spawn(move || {
        // Account/login probes, review context and notices may block. The
        // exact dispatch is already claimed, so the merge can return its hold
        // while startup continues; retries cannot launch another verifier.
        let result = (|| {
            let body = prompt(&worker, &task, &live)?;
            let input = output.with_file_name("prompt.txt");
            std::fs::write(&input, body).map_err(|e| e.to_string())?;
            let (mut command, notice) = prepare(&output)?;
            if let Some(notice) = notice
                && let Err(error) = attention(&worker, &live, &notice)
            {
                tracing::warn!(dispatch_id = %live.id, %error, "Verifier startup notice failed");
            }
            review(&worker, &live, &child.id, &mut command, &input, &output)
        })();
        // The existing daemon expiry sweep owns timeout escalation and its
        // durable dedupe key; do not emit a second timeout wake here.
        if let Err(error) = result && chrono::Utc::now() < live.deadline_at && !resolved(&worker, &live) {
            let error = crate::mcp::tools::service::agent_search_system::system::redact_known_credentials(&error);
            let code = refusal_code(&error);
            if matches!(code, PROOF_DRIFT_CODE | REPORT_PROOF_MISSING_CODE) {
                let retired = cas_store::invalidate_verification_dispatch_for_proof_drift(&worker.cas_root, &live.id);
                let retry = retry_call(&worker.cas_root, &live);
                let recovery = if code == REPORT_PROOF_MISSING_CODE {
                    format!("Reconnect or restart the Cassy MCP session that requested the review so it uses the installed binary, then retry {retry}.")
                } else {
                    format!("Retry {retry} to request a new review of the current report.")
                };
                let detail = match retired {
                    Ok(_) => format!(
                        "Automatic verifier stopped: {error}. This dispatch was retired. {recovery}"
                    ),
                    Err(retire) => format!("Automatic verifier stopped: {error}. Retiring this dispatch failed: {retire}. Inspect it before retrying."),
                };
                if let Err(notice) = attention(&worker, &live, &detail) {
                    tracing::warn!(dispatch_id = %live.id, %notice, "{detail}");
                }
            } else { match cas_store::release_failed_verifier_dispatch(&worker.cas_root, &live.id, &child.id, &error) {
                Ok(true) => {
                    let retry = recovery(&worker.cas_root, &live);
                    let _ = attention(&worker, &live, &format!(
                        "Automatic verifier stopped: {error}. Its claim was released and audited. {retry}"
                    ));
                }
                Ok(false) => {} // A verdict, timeout or replacement already owns recovery.
                Err(recovery) => {
                    let _ = attention(&worker, &live, &format!("Automatic verifier stopped: {error}. Claim recovery failed: {recovery}. Inspect this exact dispatch before retrying."));
                }
            } }
        }
        if let Ok(agents) = worker.open_agent_store() {
            let _ = agents.graceful_shutdown(&child.id);
        }
    }).map_err(|e| e.to_string())?;
    Ok(Some(handle))
}

/// A retry is executable only while its recorded owner can launch it.
pub(super) fn recovery_retry(root: &Path, dispatch: &VerificationDispatch) -> Option<String> {
    let owner_available = cas_store::get_verification_dispatch(root, &dispatch.id).ok()
        .and_then(|live| crate::store::open_agent_store(root).ok()?.get(&live.owner_agent_id).ok())
        .is_some_and(|owner| owner.role == cas_types::AgentRole::Supervisor && owner.is_alive());
    owner_available.then(|| retry_call(root, dispatch))
}

/// The executable recovery after an automatic verifier could not finish.
pub(crate) fn recovery(root: &Path, dispatch: &VerificationDispatch) -> String {
    let Some(retry) = recovery_retry(root, dispatch) else {
        return format!("Dispatch {} needs its recorded supervisor handoff repaired. Inspect the retired owner's session-end claim and live successor before launching; ownership could not be established.", dispatch.id);
    };
    format!(
        "Retry {retry}; one replacement will claim this exact dispatch. {}",
        direct_verdict(dispatch)
    )
}

/// The call that starts this dispatch's verifier: the train merge that owns
/// the target holds single merges (G-101), so it is retried instead.
pub(crate) fn retry_call(root: &Path, dispatch: &VerificationDispatch) -> String {
    let prefix = crate::mcp::tools::core::guidance::supervisor_prefix();
    match candidate_for_dispatch(root, dispatch).ok().flatten() {
        Some(candidate) => match crate::mcp::tools::core::workflow::train_merge::owner(
            root,
            &candidate.repository,
            &candidate.target_branch,
        ) {
            Some(train) => format!("{prefix}factory action=train_merge id={train}"),
            None => format!(
                "{prefix}factory action=worktree_merge id={} task_id={}",
                candidate.source_branch, dispatch.task_id
            ),
        },
        None => format!("{prefix}task action=close id={}", dispatch.task_id),
    }
}

/// The other supported route. A native Task/Agent child on the supervisor's
/// MCP connection shares its caller and cannot record this verdict.
pub(crate) fn direct_verdict(dispatch: &VerificationDispatch) -> String {
    format!(
        "For mechanical evidence only, the registered supervisor may record it directly: {}verification action=add task_id={} dispatch_id={} status=approved|rejected summary=\"<review and verdict>\". Do not spawn a native Task/Agent verifier; it shares your MCP caller and cannot record.",
        crate::mcp::tools::core::guidance::supervisor_prefix(),
        dispatch.task_id,
        dispatch.id,
    )
}

/// A dispatch that already holds a verdict, or whose task is terminal, needs no
/// supervisor notice when a second verifier finds it settled.
fn resolved(core: &CasCore, dispatch: &VerificationDispatch) -> bool {
    cas_store::get_verification_for_dispatch(&core.cas_root, &dispatch.id).is_ok_and(|v| v.is_some_and(|v| !matches!(v.status, VerificationStatus::Error | VerificationStatus::Pending))) || core
        .open_task_store()
        .ok()
        .and_then(|s| s.get(&dispatch.task_id).ok())
        .is_some_and(|t| t.is_terminal())
}

fn review(
    core: &CasCore,
    dispatch: &VerificationDispatch,
    child: &str,
    command: &mut Command,
    input: &Path,
    output: &Path,
) -> Result<(), String> {
    let remaining = (dispatch.deadline_at - chrono::Utc::now())
        .to_std()
        .map_err(|_| "dispatch expired")?;
    let stdin = std::fs::File::open(input).map_err(|e| e.to_string())?;
    let result = crate::bounded_process::run_command_with_stdin(
        command,
        crate::bounded_process::Deadline::after(remaining),
        remaining,
        Stdio::from(stdin),
    )
    .map_err(|e| format!("harness failed or timed out: {e:?}"))?;
    let stderr_log = output.with_file_name("harness.log");
    let stdout_log = output.with_file_name("harness.stdout");
    std::fs::write(&stderr_log, &result.stderr).map_err(|e| e.to_string())?;
    // cas-2b80b: keep what the harness printed, so a refused response can be
    // diagnosed from the dispatch audit instead of rerunning the review.
    std::fs::write(&stdout_log, &result.stdout).map_err(|e| e.to_string())?;
    // GH #2141: a provider safety refusal is this dispatch's infrastructure
    // ERROR, never a code verdict; the refused output stays in the logs.
    let refused = !result.status.success()
        && (provider_policy_refusal(&result.stderr) || provider_policy_refusal(&result.stdout));
    if !result.status.success() && !refused {
        return Err(format!(
            "harness exited {}; stderr tail ({}):\n{}",
            result.status,
            stderr_log.display(),
            tail(&result.stderr),
        ));
    }
    let bytes = if refused {
        serde_json::json!({"status": "error", "summary": format!(
            "{PROVIDER_POLICY_CODE}: the provider's safety filter refused this review before any verdict; refused output kept at {}",
            stderr_log.display(),
        )}).to_string().into_bytes()
    } else if output.exists() {
        std::fs::read(output).map_err(|e| e.to_string())?
    } else {
        result.stdout
    };
    let mut arguments = final_json_object(&bytes).ok_or_else(|| {
        format!(
            "invalid verifier response: no JSON object in {} bytes; stdout tail ({}):\n{}\nstderr tail ({}):\n{}",
            bytes.len(),
            stdout_log.display(),
            tail(&bytes),
            stderr_log.display(),
            tail(&result.stderr),
        )
    })?;
    arguments.insert("task_id".into(), serde_json::json!(dispatch.task_id));
    arguments.insert("dispatch_id".into(), serde_json::json!(dispatch.id));
    arguments.insert("verifier_capability".into(), serde_json::Value::Null);
    arguments.insert("duration_ms".into(), serde_json::Value::Null);
    arguments.insert("verification_type".into(), serde_json::json!("task"));
    let request: crate::mcp::tools::VerificationAddRequest =
        serde_json::from_value(arguments.into())
            .map_err(|e| format!("invalid verifier response: {e}"))?;
    if !matches!(
        request.status.as_str(),
        "approved" | "rejected" | "supervisor_call" | "error"
    ) {
        return Err("invalid terminal verifier status".into());
    }
    std::fs::write(output, &bytes).map_err(|e| e.to_string())?;
    let held = (request.status != "approved").then(|| if refused {
        format!("Automatic verifier recorded an infrastructure ERROR, not a code verdict: {}. {}", request.summary, direct_verdict(dispatch))
    } else {
        format!(
            "Automatic verifier recorded {}: {}. Findings: {}. The close stays held; answer or resolve this exact verdict on dispatch {}.",
            request.status, request.summary, request.issues.as_deref().unwrap_or("[]"), dispatch.id,
        )
    });
    let verifier = CasCore::with_daemon(core.cas_root.clone(), None, core.daemon.clone());
    verifier
        .bind_agent_identity(
            child.into(),
            crate::mcp::AgentIdentitySource::ServerInternal,
        )
        .map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let verdict = runtime
        .block_on(crate::async_runtime::heap_future(|| {
            verifier.cas_verification_add(rmcp::handler::server::wrapper::Parameters(request))
        }))
        .map_err(|e| e.to_string())?;
    if verdict.is_error == Some(true) {
        return Err(format!("verdict refused: {verdict:?}"));
    }
    // A candidate verdict continues or holds its merge and notifies there.
    // A close dispatch's approval resumes its close; anything else needs
    // the owner, who would otherwise never see this daemon-recorded verdict.
    // A refusal holds either kind until the supervisor records the verdict.
    if let Some(held) = held
        && !verdict.structured_content.as_ref().is_some_and(|data| data["repeat_review_held"].as_bool() == Some(true))
        && (refused || candidate_for_dispatch(&core.cas_root, dispatch)?.is_none())
    {
        attention(core, dispatch, &held)?;
    }
    Ok(())
}

/// The provider's safety filter refused the review prompt itself (GH #2141,
/// Codex `exec`). It is the provider's wording, not a Cassy code, and only
/// decides that the run is infrastructure; it never becomes a code verdict.
const PROVIDER_POLICY_REFUSAL: &str = "flagged for possible cybersecurity risk";
const PROVIDER_POLICY_CODE: &str = "verifier_provider_policy";

fn provider_policy_refusal(output: &[u8]) -> bool {
    String::from_utf8_lossy(output).lines()
        .any(|line| line.trim_start().starts_with("ERROR:") && line.contains(PROVIDER_POLICY_REFUSAL))
}

/// The final eight lines within a 2 KiB window, redacted. The full output
/// stays on disk, so an echoed prompt cannot flood the owner's notice.
fn tail(bytes: &[u8]) -> String {
    let window = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(2048)..]);
    let mut lines = window.lines().rev().take(8).collect::<Vec<_>>();
    lines.reverse();
    let tail = crate::mcp::tools::service::agent_search_system::system::redact_known_credentials(
        &lines.join("\n"),
    );
    if tail.trim().is_empty() { "(empty)".into() } else { tail }
}

/// The last top-level JSON object a harness printed. `claude -p` returns the
/// model's final text, which may put prose or a Markdown fence around it.
fn final_json_object(bytes: &[u8]) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut found = None;
    let mut start = 0;
    while let Some(offset) = bytes[start..].iter().position(|byte| *byte == b'{') {
        let at = start + offset;
        let mut stream = serde_json::Deserializer::from_slice(&bytes[at..])
            .into_iter::<serde_json::Map<String, serde_json::Value>>();
        match stream.next() {
            Some(Ok(object)) => {
                found = Some(object);
                start = at + stream.byte_offset();
            }
            _ => start = at + 1,
        }
    }
    found
}

// gate[N-16 notice]: Automatic verifier could not start
pub(super) fn attention(
    core: &CasCore,
    dispatch: &VerificationDispatch,
    detail: &str,
) -> Result<(), String> {
    let rows = core
        .open_agent_store()
        .map_err(|e| e.to_string())?
        .list(None)
        .map_err(|e| e.to_string())?;
    let recipient = crate::supervisor_notices::resolve(&rows, Some(&dispatch.owner_agent_id), None);
    crate::supervisor_notices::enqueue(
        &core.cas_root,
        &recipient,
        "premerge_verification",
        &format!(
            "Task {} dispatch {}: {detail}",
            dispatch.task_id, dispatch.id
        ),
        &format!("{}:{detail}", dispatch.id),
    )
    .map_err(|e| e.to_string())
}

pub(super) async fn approved(
    core: &CasCore,
    dispatch: &VerificationDispatch,
) -> Result<String, McpError> {
    let candidate = candidate_for_dispatch(&core.cas_root, dispatch)
        .map_err(fail)?
        .ok_or_else(|| fail("approved candidate missing"))?;
    let owner = CasCore::with_daemon(core.cas_root.clone(), None, core.daemon.clone());
    owner.bind_agent_identity(
        dispatch.owner_agent_id.clone(),
        crate::mcp::AgentIdentitySource::ServerInternal,
    )?;
    let _phase = crate::mcp::tools::service::mutation_receipt::phase("verification_auto_merge");
    let result = crate::async_runtime::heap_future(|| {
        owner.worktree_merge(
            &candidate.source_branch,
            false,
            Some(&dispatch.task_id),
            false,
            Some(false),
            false,
            None,
        )
    })
    .await;
    let landed = matches!(&result, Ok(result) if result.is_error != Some(true));
    let detail = match result {
        Ok(result) if result.is_error != Some(true) => {
            "Approved; the merge continued automatically.".to_string()
        }
        Err(error) if error.message.contains("TRAIN MERGE ACTIVE") => {
            "Approved; the train queue continues this merge.".to_string()
        }
        other => {
            let detail = format!("Approved; automatic merge held: {other:?}");
            attention(core, dispatch, &detail).map_err(fail)?;
            return Ok(detail);
        }
    };
    // GH #2365: this merge runs from the verdict, inside no supervisor tool
    // call, so no merge receipt reports it. Its landing opens the supervisor's
    // next step (promotion, notes, dependents): one wake per dispatch.
    if landed {
        let status = core.open_task_store().ok().and_then(|tasks| tasks.get(&dispatch.task_id).ok())
            .map_or_else(|| "unknown".to_string(), |task| task.status.to_string());
        let woke = (|| -> Result<(), String> {
            let rows = core.open_agent_store().map_err(|e| e.to_string())?.list(None).map_err(|e| e.to_string())?;
            let recipient = crate::supervisor_notices::resolve(&rows, Some(&dispatch.owner_agent_id), None);
            crate::supervisor_notices::enqueue(&core.cas_root, &recipient, "merge_outcome",
                &format!("Task {} dispatch {}: approved; the merge landed automatically and the task status is now `{status}`.",
                    dispatch.task_id, dispatch.id),
                &format!("automatic-approval:{}", dispatch.id)).map_err(|e| e.to_string())
        })();
        if let Err(error) = woke {
            tracing::warn!(task = %dispatch.task_id, %error, "automatic merge landed; supervisor wake not recorded");
        }
    }
    // A readable receipt without a prompt; a landed merge already woke the supervisor above.
    let queue = crate::store::open_supervisor_queue_store(&core.cas_root).map_err(fail)?;
    let notification = queue.notify_idempotent(&dispatch.owner_agent_id, "verification_approved",
        &serde_json::json!({"task_id":dispatch.task_id,"dispatch_id":dispatch.id,"detail":detail}).to_string(),
        cas_store::NotificationPriority::Normal, &format!("automatic-verifier-approved:{}", dispatch.id)).map_err(fail)?;
    let id = match notification {
        cas_store::NotifyIdempotentResult::Created(id)
        | cas_store::NotifyIdempotentResult::AlreadyExists { id, .. } => id,
    };
    queue.mark_prompt_delivered(id).map_err(fail)?;
    Ok(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A no-code close bypasses candidate preparation. Its real launcher must
    /// resolve peer proof before a verifier command can be prepared.
    #[tokio::test(flavor = "multi_thread")]
    async fn peer_artifacts_are_verified_before_dispatch_cas_f6ca() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let bytes = b"peer design evidence\n";
        for (parent_scope, mode) in [
            (false, "good"), (false, "corrupt"), (false, "missing"),
            (true, "good"), (true, "corrupt"), (true, "missing"),
            (true, "revoked"), (true, "foreign-artifact"),
            (true, "foreign-project"), (true, "id-only"), (true, "unreferenced"),
            // GH #2397 (Nov 23): the epic's prose names an artifact another in-scope task published.
            (true, "other-task"),
        ] {
            let server = MockServer::start().await;
            let (_dir, core, _owner, mut rows) = contention_fixture(&["cas-f6ca-fixture"]);
            env.set("CAS_ROOT", &core.cas_root);
            env.set("CAS_AGENT_ROLE", "supervisor");
            std::fs::write(core.cas_root.join("cloud.json"), serde_json::json!({
                "endpoint":server.uri(), "token":"fixture-token"
            }).to_string()).unwrap();
            let (mut task, dispatch) = rows.pop().unwrap();
            let publisher_dir = tempfile::tempdir().unwrap();
            let publisher = crate::store::init_cas_dir(publisher_dir.path()).unwrap();
            let mut parent = Task::new("cas-peer-parent".into(), "Peer epic plan".into());
            parent.task_type = cas_types::TaskType::Epic;
            parent.design = if mode == "unreferenced" { String::new() } else {
                format!("Plan: {}", if mode == "id-only" { "art-id-only" } else { "art-peer-plan" })
            };
            // Keep the identity with which contention_fixture created the
            // child; changing the receiver's pin afterwards makes it foreign.
            let project = crate::cloud::resolve_canonical_id(&core.cas_root).unwrap();
            parent.origin_project = Some(if mode == "foreign-project" { "foreign".into() } else { project });
            let mut sibling = Task::new("cas-peer-sibling".into(), "Another task's design brief".into());
            sibling.origin_project = parent.origin_project.clone();
            let owner = if mode == "other-task" { &sibling } else { &parent };
            let record = cas_store::PublishedArtifact {
                id:"art-peer-plan".into(), task_id:if parent_scope { owner.id.clone() } else { task.id.clone() }, name:"decision-brief.md".into(),
                mime:"text/markdown".into(), size_bytes:bytes.len() as u64,
                sha256:format!("{:x}", Sha256::digest(bytes)),
                cloud_artifact_id:Some("cloud-peer-plan".into()), status:"committed".into(),
                cloud_url:None, slack_permalink:None, created_at:"2026-11-21T15:00:00Z".into(), revocation:None,
            };
            if parent_scope {
                // Only publication notes cross machines: the reviewer never
                // shares the publisher's artifact database or cached bytes.
                let publishing_tasks = crate::store::open_task_store(&publisher).unwrap();
                publishing_tasks.add(owner).unwrap();
                let publishing_artifacts = cas_store::SqliteArtifactStore::open(&publisher).unwrap();
                publishing_artifacts.record_local(&cas_store::NewArtifact {
                    id:record.id.clone(), task_id:record.task_id.clone(), name:record.name.clone(),
                    mime:record.mime.clone(), size_bytes:record.size_bytes, sha256:record.sha256.clone(),
                }).unwrap();
                publishing_artifacts.mark_uploaded(&record.id, record.cloud_artifact_id.as_deref().unwrap()).unwrap();
                publishing_artifacts.mark_committed(&record.id, None).unwrap();
                crate::artifacts::proof::record_published(&publisher, &record).unwrap();
                let mut synced = publishing_tasks.get(&owner.id).unwrap();
                let mut notes = std::mem::take(&mut synced.notes);
                if mode == "foreign-artifact" {
                    notes = notes.replace(&parent.id, "cas-unrelated-owner");
                }
                let receiving_tasks = core.open_task_store().unwrap();
                receiving_tasks.add(&synced).unwrap();
                receiving_tasks.append_trusted_note(&owner.id, &notes).unwrap();
                if mode == "other-task" {
                    // The epic only mentions the sibling's artifact in an appended note.
                    receiving_tasks.add(&parent).unwrap();
                    receiving_tasks.append_trusted_note(&parent.id, "Design sign-off cites art-peer-plan from the sibling task").unwrap();
                }
                receiving_tasks.add_dependency(&cas_types::Dependency::new(
                    task.id.clone(), parent.id.clone(), cas_types::DependencyType::ParentChild,
                )).unwrap();
            } else {
                let note = format!("ARTIFACT_PROOF: {}", serde_json::to_string(&record).unwrap());
                core.open_task_store().unwrap().append_trusted_note(&task.id, &note).unwrap();
            }
            task = core.open_task_store().unwrap().get(&task.id).unwrap();
            let expected = crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(
                &core.cas_root, if parent_scope { owner } else { &task }).unwrap().join("art-peer-plan-decision-brief.md");
            assert!(!expected.exists());
            let view = if mode == "missing" { ResponseTemplate::new(404) } else if mode == "revoked" { ResponseTemplate::new(410) } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "artifact_id":"cloud-peer-plan", "url":format!("{}/blob",server.uri()), "size_bytes":bytes.len()
                }))
            };
            Mock::given(method("GET")).and(path("/api/artifacts/cloud-peer-plan/url"))
                .respond_with(view).mount(&server).await;
            Mock::given(method("GET")).and(path("/blob"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(
                    if mode == "corrupt" { vec![b'x'; bytes.len()] } else { bytes.to_vec() }
                )).mount(&server).await;
            // Repeat a successful launch with only an imported metadata row:
            // deleting its cached bytes must not turn that row into evidence.
            for attempt in 0..if mode == "good" { 2 } else { 1 } {
                let (prepared, received) = std::sync::mpsc::channel();
                let probe = expected.clone();
                let output = core.cas_root.join(format!("attempt-{attempt}.json"));
                let handle = launch_with_preparation(&core, &task, &dispatch, output, move |_| {
                    prepared.send(std::fs::read(probe)).unwrap();
                    Err("stop before launching the fixture verifier".into())
                }).unwrap().unwrap();
                handle.join().unwrap();
                let prepared = received.try_recv().ok();
                if matches!(mode, "good" | "other-task") {
                    assert!(prepared.as_ref().is_some_and(|result| result.is_ok()),
                        "parent={parent_scope} attempt={attempt}: {prepared:?}; notices={:?}",
                        crate::store::open_prompt_queue_store(&core.cas_root).unwrap().peek_all(10).unwrap());
                    assert_eq!(prepared.unwrap().unwrap(), bytes, "parent={parent_scope} attempt {attempt}");
                    assert!(cas_store::SqliteArtifactStore::open(&core.cas_root).unwrap().get(&record.id).unwrap().is_some());
                    let input = std::fs::read_to_string(core.cas_root.join("prompt.txt")).unwrap();
                    let inputs: serde_json::Value = input.lines()
                        .filter_map(|line| serde_json::from_str(line).ok())
                        .find(|value: &serde_json::Value| value.get("artifacts").is_some()).unwrap();
                    let reference = inputs["artifacts"].as_array().unwrap().iter()
                        .find(|reference| reference["artifact_id"] == record.id).unwrap();
                    assert_eq!(Path::new(reference["path"].as_str().unwrap()), expected);
                    if parent_scope { assert_eq!(reference["task_id"], owner.id); }
                    std::fs::remove_file(&expected).unwrap();
                    if parent_scope && attempt == 1 && mode == "good" {
                        let before = context_digest(&core.cas_root, &task).unwrap();
                        let mut sequence = record.clone();
                        sequence.id = "art-peer-sequence".into();
                        sequence.cloud_artifact_id = Some("cloud-peer-sequence".into());
                        core.open_task_store().unwrap().append_trusted_note(&parent.id, &format!(
                            "Sequence art-peer-sequence\nARTIFACT_PROOF: {}", serde_json::to_string(&sequence).unwrap(),
                        )).unwrap();
                        assert_ne!(context_digest(&core.cas_root, &task).unwrap(), before,
                            "new referenced parent proof invalidates the candidate context");
                    }
                } else if mode == "unreferenced" {
                    assert!(prepared.is_some());
                    assert!(!expected.exists());
                    assert!(server.received_requests().await.unwrap().is_empty());
                } else if matches!(mode, "foreign-artifact" | "id-only") {
                    // GH #2397: a mention no in-scope task owns is missing context, not a refusal.
                    assert!(prepared.is_some(), "an unresolvable mention refused the merge: {mode}");
                    assert!(!expected.exists());
                    assert!(server.received_requests().await.unwrap().is_empty());
                    let input = std::fs::read_to_string(core.cas_root.join("prompt.txt")).unwrap();
                    let inputs: serde_json::Value = input.lines()
                        .filter_map(|line| serde_json::from_str(line).ok())
                        .find(|value: &serde_json::Value| value.get("artifacts").is_some()).unwrap();
                    assert!(inputs["artifacts"].as_array().unwrap().iter()
                        .any(|reference| reference["missing_context"].is_string()), "{mode}: {inputs}");
                } else {
                    assert!(prepared.is_none(), "unverified bytes reached verifier preparation: {mode}");
                    assert!(!expected.exists());
                    if mode == "foreign-project" {
                        assert!(server.received_requests().await.unwrap().is_empty());
                    }
                }
                assert_eq!(cas_store::get_verification_dispatch(&core.cas_root, &dispatch.id).unwrap().state,
                    cas_types::VerificationDispatchState::Pending);
                assert!(cas_store::get_verification_for_dispatch(&core.cas_root, &dispatch.id).unwrap().is_none());
            }
        }
    }

    /// GH #2023: Codex `exec` runs with approval: never, so the verifier's
    /// Cassy server must be pre-approved and launchable from its own overrides.
    #[test]
    fn codex_verifier_preapproves_only_the_cassy_server_gh_2023() {
        let args = codex_exec_args("model", "high", Path::new("/out/verdict.json"));
        let mut config = toml::Table::new();
        for pair in args.windows(2).filter(|pair| pair[0] == "-c") {
            let (key, value) = pair[1].to_str().unwrap().split_once('=').unwrap();
            let value: toml::Table = toml::from_str(&format!("v = {value}")).unwrap();
            let mut table = &mut config;
            let path: Vec<_> = key.split('.').collect();
            for part in &path[..path.len() - 1] {
                table = table
                    .entry(*part)
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                    .as_table_mut()
                    .unwrap();
            }
            table.insert(path[path.len() - 1].into(), value["v"].clone());
        }
        let servers = config.get("mcp_servers").and_then(|v| v.as_table());
        let cs = servers.and_then(|servers| servers.get("cs"));
        assert_eq!(
            cs.cloned(),
            Some(toml::Value::Table(
                toml::from_str(
                    "command = \"cas\"\nargs = [\"serve\"]\ndefault_tools_approval_mode = \"approve\"",
                )
                .unwrap()
            )),
            "{config:?}"
        );
        assert_eq!(servers.map(|servers| servers.len()), Some(1), "{config:?}");
        assert!(args.windows(2).any(|pair| pair[0] == "--sandbox" && pair[1] == "read-only"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn merge_retry_replaces_only_a_failed_or_stalled_verifier_gh_2023_gh_2325() {
        for case in ["harness-failure", "stalled", "legacy-stalled", "heartbeat", "lane"] {
            merge_retry(case);
        }
    }

    #[cfg(target_os = "linux")]
    fn age_claim(root: &Path, claim: &VerificationDispatch, by: chrono::Duration) {
        let aged = |at: &str| (chrono::DateTime::parse_from_rfc3339(at).unwrap() - by).to_rfc3339();
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        for (table, key, id, columns) in [
            ("verification_dispatches", "id", Some(&claim.id), &["requested_at", "deadline_at"][..]),
            ("verification_capabilities", "id", claim.capability_id.as_ref(), &["issued_at", "expires_at", "bound_at"][..]),
            ("verification_handoffs", "capability_id", claim.capability_id.as_ref(), &["created_at", "bound_at"][..]),
            ("agents", "id", claim.verifier_agent_id.as_ref(), &["registered_at", "last_heartbeat"][..]),
        ] {
            for column in columns {
                let at: String = conn.query_row(
                    &format!("SELECT {column} FROM {table} WHERE {key}=?1"), [id], |row| row.get(0),
                ).unwrap();
                conn.execute(&format!("UPDATE {table} SET {column}=?2 WHERE {key}=?1"),
                    rusqlite::params![id, aged(&at)]).unwrap();
            }
        }
        let checkout = Path::new(&claim.repository.as_ref().unwrap().worktree_root)
            .parent().unwrap().join("owner.json");
        let mut owner: serde_json::Value = serde_json::from_slice(&std::fs::read(&checkout).unwrap()).unwrap();
        for field in ["created_at", "last_used_at"] {
            owner[field] = aged(owner[field].as_str().unwrap()).into();
        }
        std::fs::write(&checkout, owner.to_string()).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn merge_retry(case: &'static str) {
        use crate::ui::factory::{
            ClientMessage, DaemonMessage, FRAME_HEADER_SIZE, PROTOCOL_VERSION, SessionState,
            decode_length, encode_message, gui_socket_path, local_gui_capabilities,
        };
        use std::io::{Read, Write};
        use std::os::fd::AsRawFd;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let run = |args: &[&str]| super::super::tests::run(repo, args);
        run(&["init", "-q", "-b", "epic/train"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.invalid"]);
        let root = crate::store::init_cas_dir(repo).unwrap();
        std::fs::write(repo.join(".git/info/exclude"), ".cas\n").unwrap();
        std::fs::write(root.join("config.toml"), "[worktrees]\nenabled = true\n[qa]\nmode = 'batch'\n[verification]\nenabled = true\n[code_review]\nowner = 'worker'\n").unwrap();
        super::super::tests::commit_file(repo, "README.md", "base\n", "base");
        let lane = root.join("worktrees/worker");
        run(&["worktree", "add", "-q", "-b", "factory/worker", lane.to_str().unwrap()]);
        let source = super::super::tests::commit_file(&lane, "README.md", "delivery\n", "delivery");
        env.set("CAS_ROOT", &root);
        env.set("CAS_AGENT_ROLE", "supervisor");
        let daemon = crate::mcp::daemon::EmbeddedDaemon::new(crate::mcp::daemon::EmbeddedDaemonConfig {
            cas_root: root.clone(),
            ..Default::default()
        });
        let core = CasCore::with_daemon(root.clone(), None, Some(std::sync::Arc::new(daemon)));
        let mut owner = cas_types::Agent::new_with_role(
            "owner".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
        );
        owner.factory_session = Some("gh-2023".into());
        owner.machine_id = Some(cas_types::Agent::get_or_generate_machine_id());
        core.open_agent_store().unwrap().register(&owner).unwrap();
        core.bind_agent_identity(owner.id.clone(), crate::mcp::AgentIdentitySource::ServerInternal).unwrap();
        let mut task = Task::new("cas-2023-retry".into(), "Retry after harness failure".into());
        task.status = cas_types::TaskStatus::AwaitingMerge;
        task.depth = cas_types::TaskDepth::Deep;
        task.assignee = Some("worker".into());
        task.deliverables.factory_branch_anchor = Some(source);
        task.deliverables.parked_branch = Some("factory/worker".into());
        core.open_task_store().unwrap().add(&task).unwrap();
        let socket = gui_socket_path("gh-2023");
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let serve_launch = |attempt: usize, expected_dispatch: &str| {
            let mut ready = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            assert!(unsafe { libc::poll(&mut ready, 1, 10000) } > 0, "{case}: launch {attempt}");
            let (mut client, _) = listener.accept().unwrap();
            client.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            client.set_write_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            client.write_all(&encode_message(&DaemonMessage::Welcome {
                session_name: "gh-2023".into(),
                state: SessionState { focused_pane: None, panes: vec![], epic_id: None,
                    epic_title: None, cols: 80, rows: 24 },
                scrollback: None, protocol_version: PROTOCOL_VERSION,
                capabilities: local_gui_capabilities(), pane_bootstrap: vec![],
            }).unwrap()).unwrap();
            let mut header = [0; FRAME_HEADER_SIZE];
            client.read_exact(&mut header).unwrap();
            let mut body = vec![0; decode_length(&header)];
            client.read_exact(&mut body).unwrap();
            let ClientMessage::LaunchVerifier { dispatch_id } = serde_json::from_slice(&body).unwrap()
            else {
                panic!("unexpected factory control");
            };
            assert_eq!(dispatch_id, expected_dispatch, "{case}");
            let harness_fails = case == "harness-failure" && attempt == 1;
            let output = root.join("verification-runs").join(&dispatch_id)
                .join(format!("attempt-{attempt}")).join("verdict.json");
            std::fs::create_dir_all(output.parent().unwrap()).unwrap();
            let (resume, provider_held) = std::sync::mpsc::channel::<()>();
            let started = super::factory_launch::launch_with(&root, "gh-2023", &dispatch_id,
                |core, task, dispatch| launch_with_preparation(core, task, dispatch, output, move |_| {
                    if harness_fails {
                        let mut harness = Command::new("sh");
                        harness.args(["-c", "exit 1"]);
                        return Ok((harness, None));
                    }
                    provider_held.recv().ok();
                    Err("provider released".into())
                }));
            client.write_all(&encode_message(&match &started {
                Ok(_) => DaemonMessage::VerifierLaunched { dispatch_id },
                Err(message) => DaemonMessage::Error {
                    message: message.clone(), client_ref: Some(dispatch_id),
                },
            }).unwrap()).unwrap();
            started.unwrap().map(|review| (review, resume))
        };

        assert!(core.prepare_premerge_verification(&task, repo, "factory/worker", "epic/train").is_err());
        let dispatch = cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().unwrap();
        let (first, resume_first) = serve_launch(1, &dispatch.id).expect("the first request claims the dispatch");
        let stalled = if case == "harness-failure" {
            first.join().unwrap();
            None
        } else {
            let claimed = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
            assert_eq!(claimed.state, cas_types::VerificationDispatchState::Claimed);
            assert!(owns(&root, &claimed));
            age_claim(&root, &claimed, chrono::Duration::minutes(11));
            let child = claimed.verifier_agent_id.unwrap();
            match case {
                "legacy-stalled" => {
                    rusqlite::Connection::open(root.join("cas.db")).unwrap()
                        .execute("UPDATE agents SET machine_id=NULL WHERE id=?1", [&child]).unwrap();
                }
                "heartbeat" => core.open_agent_store().unwrap().heartbeat(&child).unwrap(),
                "lane" => drop(crate::factory_review_scratch::create(
                    &root, repo, &task.id, Some(&dispatch.id),
                    &dispatch.repository.as_ref().unwrap().head_commit,
                ).unwrap()),
                _ => {}
            }
            Some(first)
        };
        let before = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
        assert_eq!(before.state, if stalled.is_some() {
            cas_types::VerificationDispatchState::Claimed
        } else {
            cas_types::VerificationDispatchState::Pending
        });

        let held = core
            .prepare_premerge_verification(&task, repo, "factory/worker", "epic/train")
            .err()
            .expect("an unreviewed delivery stays held");

        let second = serve_launch(2, &dispatch.id);
        let after = cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().unwrap();
        let count = |sql: &str| -> i64 {
            rusqlite::Connection::open(root.join("cas.db")).unwrap()
                .query_row(sql, [&dispatch.id], |row| row.get(0)).unwrap()
        };
        let recoveries = count("SELECT COUNT(*) FROM events WHERE json_extract(metadata,'$.code')='expired_verifier_claim_recovered' AND json_extract(metadata,'$.dispatch_id')=?1");
        let claims = count("SELECT COUNT(*) FROM verification_capabilities WHERE dispatch_id=?1 AND verifier_agent_id IS NOT NULL");
        if matches!(case, "heartbeat" | "lane") {
            assert!(second.is_none(), "{case}");
            assert_eq!(serde_json::to_value(&after).unwrap(), serde_json::to_value(&before).unwrap(), "{case}");
            assert_eq!((recoveries, claims), (0, 1), "{case}");
            assert!(held.data.is_none(), "{case}: {held:?}");
            drop(resume_first);
            stalled.unwrap().join().unwrap();
            return;
        }
        assert_eq!(
            held.data.as_ref().map(|data| data["reason"].clone()),
            Some(serde_json::json!("verifier_replacement_scheduled")),
            "{case}: {held:?}"
        );
        let (replacement, finish) = second.expect("the retry's request claims the released dispatch");
        assert_eq!((&after.id, &after.repository), (&dispatch.id, &dispatch.repository), "{case}");
        assert_eq!(after.state, cas_types::VerificationDispatchState::Claimed, "{case}");
        assert!(after.verifier_agent_id.is_some() && after.verifier_agent_id != before.verifier_agent_id, "{case}");
        assert_eq!((recoveries, claims), (i64::from(stalled.is_some()), 2), "{case}");
        if let Some(stalled) = stalled {
            drop(resume_first);
            stalled.join().unwrap();
            let kept = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
            assert_eq!(serde_json::to_value(&kept).unwrap(), serde_json::to_value(&after).unwrap(), "{case}");
        }
        drop(finish);
        replacement.join().unwrap();
    }

    #[test]
    fn failed_launch_is_retried_unless_permanent_cas_d824() {
        let refusal = |retry, message: &str| LaunchError { retry, message: message.into() };
        let calls = std::cell::Cell::new(0);
        let transient = request_with_retry(4, std::time::Duration::ZERO, std::time::Duration::ZERO, || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 { Err(refusal(Retry::Backoff, "verifier_factory_launch: busy")) } else { Ok(()) }
        });
        assert_eq!((transient, calls.get()), (Ok(()), 3));
        calls.set(0);
        let permanent = request_with_retry(4, std::time::Duration::ZERO, std::time::Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Err(refusal(Retry::Never, "verifier_factory_scope: not this session"))
        });
        assert_eq!((permanent.is_err(), calls.get()), (true, 1));
        calls.set(0);
        let exhausted = request_with_retry(4, std::time::Duration::ZERO, std::time::Duration::ZERO, || {
            calls.set(calls.get() + 1);
            Err(refusal(Retry::Backoff, "verifier_factory_launch: busy"))
        });
        assert_eq!((exhausted.map_err(|(attempts, _)| attempts), calls.get()), (Err(4), 4));
    }

    /// cas-4fd6: the retry class comes from the code the daemon names, read
    /// through the real request, not from words in the message. A NotLive owner
    /// and an old daemon are never retried; six contention refusals still leave
    /// the first attempt, then an exact-dispatch mismatch is terminal; a
    /// transient failure whose text merely mentions a permanent code is retried.
    #[cfg(target_os = "linux")]
    #[test]
    fn launch_retry_class_is_the_daemons_code_not_its_text_cas_4fd6() {
        use crate::ui::factory::{
            ClientMessage, DaemonMessage, FRAME_HEADER_SIZE, PROTOCOL_VERSION, SessionState,
            decode_length, encode_message, gui_socket_path, local_gui_capabilities,
        };
        use std::io::{Read, Write};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let owner = "verifier_factory_owner: live registered supervisor required (agent a is supervisor with status Shutdown)";
        let busy = "verifier_factory_store_busy: held";
        let mut cases: Vec<(&str, Vec<Option<&str>>, Result<(), u32>)> = vec![
            ("owner", vec![Some(owner)], Err(1)),
            ("old-daemon", vec![None], Err(1)),
            ("text-only", vec![Some("verifier_factory_launch_failed: see verifier_factory_scope"); 4], Err(4)),
        ];
        let mut contended = vec![Some(busy); 6];
        contended.push(Some("verifier_handoff_not_active: vdispatch-test"));
        cases.push(("contended", contended, Err(1)));
        for (session, replies, expected) in cases {
            let socket = gui_socket_path(session);
            std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
            let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
            let connections = replies.len();
            let daemon = std::thread::spawn(move || {
                for reply in replies {
                    let (mut client, _) = listener.accept().unwrap();
                    let capabilities = if reply.is_some() { local_gui_capabilities() } else { vec![] };
                    client.write_all(&encode_message(&DaemonMessage::Welcome {
                        session_name: session.into(),
                        state: SessionState { focused_pane: None, panes: vec![], epic_id: None,
                            epic_title: None, cols: 80, rows: 24 },
                        scrollback: None, protocol_version: PROTOCOL_VERSION,
                        capabilities, pane_bootstrap: vec![],
                    }).unwrap()).unwrap();
                    let Some(message) = reply else { continue };
                    let mut header = [0; FRAME_HEADER_SIZE];
                    client.read_exact(&mut header).unwrap();
                    let mut body = vec![0; decode_length(&header)];
                    client.read_exact(&mut body).unwrap();
                    let ClientMessage::LaunchVerifier { dispatch_id } = serde_json::from_slice(&body).unwrap()
                    else { panic!("unexpected factory control") };
                    client.write_all(&encode_message(&DaemonMessage::Error {
                        message: message.into(), client_ref: Some(dispatch_id),
                    }).unwrap()).unwrap();
                }
            });
            let attempts = std::cell::Cell::new(0);
            let result = request_with_retry(4, std::time::Duration::ZERO, std::time::Duration::from_secs(5), || {
                attempts.set(attempts.get() + 1);
                super::factory_launch::request(session, "vdispatch-test")
            });
            assert_eq!(result.map_err(|(spent, _)| spent), expected, "{session}");
            assert_eq!(attempts.get() as usize, connections, "{session}: one connection per attempt");
            daemon.join().unwrap();
        }
    }

    /// cas-7caf (GH2241): on a loaded Mac the daemon took longer than the
    /// request's response deadline, the timed-out socket read surfaced as
    /// "Resource temporarily unavailable (os error 35)", and four attempts gave
    /// up while the daemon went on to launch the verifier anyway. A busy daemon
    /// is contention: the request waits and asks again, spending no attempt,
    /// and the replay sees the launch the daemon finished meanwhile.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_daemon_slower_than_the_response_deadline_is_waited_for_cas_7caf() {
        use crate::ui::factory::{
            ClientMessage, DaemonMessage, FRAME_HEADER_SIZE, PROTOCOL_VERSION, SessionState,
            decode_length, encode_message, gui_socket_path, local_gui_capabilities,
        };
        use std::io::{Read, Write};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let session = "slow-daemon";
        let socket = gui_socket_path(session);
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let daemon = std::thread::spawn(move || {
            let mut held = Vec::new();
            for answer in [false, true] {
                let (mut client, _) = listener.accept().unwrap();
                client.write_all(&encode_message(&DaemonMessage::Welcome {
                    session_name: session.into(),
                    state: SessionState { focused_pane: None, panes: vec![], epic_id: None,
                        epic_title: None, cols: 80, rows: 24 },
                    scrollback: None, protocol_version: PROTOCOL_VERSION,
                    capabilities: local_gui_capabilities(), pane_bootstrap: vec![],
                }).unwrap()).unwrap();
                let mut header = [0; FRAME_HEADER_SIZE];
                client.read_exact(&mut header).unwrap();
                let mut body = vec![0; decode_length(&header)];
                client.read_exact(&mut body).unwrap();
                let ClientMessage::LaunchVerifier { dispatch_id } = serde_json::from_slice(&body).unwrap()
                else { panic!("unexpected factory control") };
                if answer {
                    client.write_all(&encode_message(&DaemonMessage::VerifierLaunched { dispatch_id })
                        .unwrap()).unwrap();
                } else {
                    held.push(client); // busy: no answer before the deadline
                }
            }
        });
        let connections = std::cell::Cell::new(0);
        let result = request_with_retry(1, std::time::Duration::ZERO, std::time::Duration::from_secs(60), || {
            connections.set(connections.get() + 1);
            super::factory_launch::request(session, "vdispatch-test")
        });
        assert_eq!(result, Ok(()));
        assert_eq!(connections.get(), 2, "the deadline costs a wait, not the only attempt");
        daemon.join().unwrap();
    }

    #[test]
    fn daemon_bound_owner_is_a_live_supervisor_even_when_its_process_is_another_harness_cas_d824() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        env.set("CAS_ROOT", &root);
        // The supervisor's own harness process, alive and not this daemon.
        let mut harness = Command::new("sleep").arg("30").spawn().unwrap();
        let mut owner = cas_types::Agent::new("supervisor-owner-d824".into(), "supervisor-owner".into());
        owner.role = cas_types::AgentRole::Supervisor;
        owner.pid = Some(harness.id());
        crate::mcp::daemon::stamp_pid_fingerprint(&mut owner, harness.id());
        crate::store::open_agent_store(&root).unwrap().register(&owner).unwrap();
        let bound = CasCore::with_daemon(root.clone(), None, None);
        bound.bind_agent_identity(owner.id.clone(), crate::mcp::AgentIdentitySource::ServerInternal).unwrap();
        let resolved = bound.resolve_live_supervisor_authority();
        // An identity inherited from the environment is still refused.
        let inherited = CasCore::with_daemon(root.clone(), None, None);
        env.set("CAS_SESSION_ID", &owner.id);
        let refused = inherited.resolve_live_supervisor_authority();
        harness.kill().unwrap();
        harness.wait().unwrap();
        assert_eq!(resolved.map(|agent| agent.id).ok(), Some(owner.id));
        assert!(refused.is_err());
    }

    #[test]
    fn automatic_context_carries_configured_qa_mode_cas_8f8b() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        env.set("CAS_ROOT", &root);
        let core = CasCore::with_daemon(root.clone(), None, None);
        let task = Task::new("cas-8f8b-context".into(), "Review context".into());
        core.open_task_store().unwrap().add(&task).unwrap();
        let dispatch = cas_store::create_verification_dispatch_bound(
            &root, &task.id, "owner", "owner",
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10), false,
        ).unwrap();
        for mode in [crate::config::QaMode::Batch, crate::config::QaMode::PerTask] {
            std::fs::write(root.join("config.toml"), format!("[qa]\nmode = '{mode}'\n")).unwrap();
            // Each configured server loads its mode once; this is not a
            // hot-reload contract for an already-running server.
            let core = CasCore::with_daemon(root.clone(), None, None);
            let input = prompt(&core, &task, &dispatch).unwrap();
            let context = input.lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find(|value| value.get("qa").is_some())
                .unwrap_or(serde_json::Value::Null);
            assert_eq!(context["qa"]["mode"], serde_json::to_value(mode).unwrap());
            // GH #2047: repair history must not exceed the harness's 1 MiB input
            // limit. Keep the full source available rather than dropping criteria.
            let mut large = task.clone();
            large.notes = "history 🦀\n".repeat(120_000);
            large.acceptance_criteria = "Keep the caller's result exactly.".into();
            let input = prompt(&core, &large, &dispatch).unwrap();
            assert!(input.len() < 524_288, "prompt bytes: {}", input.len());
            let pointer = input.lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find_map(|value| value["review_input_file"].as_str().map(str::to_owned))
                .expect("oversized input stays available in a bound file");
            let full: serde_json::Value = serde_json::from_slice(&std::fs::read(pointer).unwrap()).unwrap();
            assert_eq!(full["task"], serde_json::to_value(&large).unwrap());
            assert_eq!(full["dispatch"], serde_json::to_value(&dispatch).unwrap());
            assert_eq!(full["qa"]["mode"], serde_json::to_value(mode).unwrap());
        }
    }

    /// A real store with one live supervisor owner and a pending dispatch per task.
    fn contention_fixture(tasks: &[&str]) -> (tempfile::TempDir, CasCore, cas_types::Agent, Vec<(Task, VerificationDispatch)>) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let owner = cas_types::Agent::new_with_role(
            "owner".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
        );
        core.open_agent_store().unwrap().register(&owner).unwrap();
        let dispatches = tasks.iter().map(|id| {
            let task = Task::new((*id).into(), (*id).into());
            core.open_task_store().unwrap().add(&task).unwrap();
            let dispatch = cas_store::create_verification_dispatch_bound(
                &root, &task.id, &owner.id, &owner.id,
                &cas_types::VerificationProofBoundary::task(),
                chrono::Utc::now() + chrono::Duration::minutes(10), false,
            ).unwrap();
            (task, dispatch)
        }).collect();
        (dir, core, owner, dispatches)
    }

    /// The daemon launch, stopped right after its claim so no harness runs.
    fn claim(core: &CasCore, task: &Task, dispatch: &VerificationDispatch) -> Result<(), String> {
        let output = core.cas_root.join(format!("{}.json", dispatch.id));
        let handle = launch_with_preparation(core, task, dispatch, output, |_| Err("stop after claim".into()))?;
        handle.expect("the dispatch was pending").join().unwrap();
        Ok(())
    }

    /// cas-111e (cas-2820, Nov 17 10:44 AM ET): a second dispatch for the same
    /// supervisor hit "already awaiting SubagentStart" four times in seconds.
    /// It now waits for the first spawn to bind and then launches.
    #[test]
    fn second_dispatch_queues_behind_a_pending_subagent_start_cas_111e() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let (_dir, core, owner, dispatches) = contention_fixture(&["cas-111e-a", "cas-111e-b"]);
        env.set("CAS_ROOT", &core.cas_root);
        let [(first_task, first), (task, second)] = dispatches.try_into().unwrap();
        cas_store::issue_server_verifier_handoff(&core.cas_root, &first_task.id, &first.id, &owner.id, "toolu_first").unwrap();
        let root = core.cas_root.clone();
        let parent = owner.id.clone();
        let binder = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let child = cas_types::Agent::new_sub_agent("task-verifier-first".into(), "task-verifier".into(), parent.clone());
            cas_store::bind_server_verifier_handoff_and_register_child(&root, &parent, &child).unwrap();
        });
        let calls = std::cell::Cell::new(0);
        let launched = request_with_retry(LAUNCH_ATTEMPTS, std::time::Duration::from_millis(50), std::time::Duration::from_secs(20), || {
            calls.set(calls.get() + 1);
            claim(&core, &task, &second).map_err(|message| LaunchError { retry: retry_for_code(&message), message })
        });
        binder.join().unwrap();
        assert_eq!(launched, Ok(()));
        assert!(calls.get() > 1, "the second start waited for the first to bind");
    }

    #[test]
    fn slow_dispatch_preparation_returns_claim_before_provider_ready_cas_2efe() {
        use std::sync::mpsc;
        use std::time::Duration;

        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        env.set("CAS_ROOT", &root);
        let core = CasCore::with_daemon(root.clone(), None, None);
        let owner = cas_types::Agent::new_with_role(
            "owner".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
        );
        core.open_agent_store().unwrap().register(&owner).unwrap();
        let task = Task::new("cas-2efe".into(), "Slow verifier preparation".into());
        core.open_task_store().unwrap().add(&task).unwrap();
        let dispatch = cas_store::create_verification_dispatch_bound(
            &root, &task.id, &owner.id, &owner.id,
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10), false,
        ).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (returned_tx, returned_rx) = mpsc::channel();
        let caller = core.clone();
        let review_task = task.clone();
        let review_dispatch = dispatch.clone();
        // Gate the external provider preparation, not the verifier/store logic.
        let request = std::thread::spawn(move || {
            let result = launch_with_preparation(
                &caller, &review_task, &review_dispatch,
                caller.cas_root.join("verdict.json"), move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Err("injected provider startup failure".into())
                },
            );
            returned_tx.send(result).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let returned = returned_rx.recv_timeout(Duration::from_secs(5));
        let live = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
        let duplicate = if returned.is_ok() {
            Some(launch_with_preparation(
                &core, &task, &dispatch, root.join("duplicate.json"),
                |_| panic!("a claimed dispatch must not probe another provider"),
            ).unwrap())
        } else { None };
        release_tx.send(()).unwrap();
        request.join().unwrap();
        let handle = returned.expect("merge must return while provider preparation is gated")
            .unwrap().unwrap();
        assert_eq!(live.id, dispatch.id);
        assert_eq!(live.state, cas_types::VerificationDispatchState::Claimed);
        assert!(owns(&root, &live));
        assert!(duplicate.unwrap().is_none());
        handle.join().unwrap();
        let recovered = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
        assert_eq!(recovered.state, cas_types::VerificationDispatchState::Pending);
        assert!(!owns(&root, &recovered));
    }

    #[test]
    fn cas_ce3e_launched_verifier_survives_attention_failure() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        env.set("CAS_ROOT", &root);
        let core = CasCore::with_daemon(root.clone(), None, None);
        let owner = cas_types::Agent::new_with_role(
            "owner".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
        );
        core.open_agent_store().unwrap().register(&owner).unwrap();
        let task = Task::new("cas-ce3e-launch".into(), "Launch notification failure".into());
        core.open_task_store().unwrap().add(&task).unwrap();
        let dispatch = cas_store::create_verification_dispatch_bound(
            &root, &task.id, &owner.id, &owner.id,
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10), false,
        ).unwrap();
        crate::store::open_supervisor_queue_store(&root).unwrap();
        rusqlite::Connection::open(root.join("cas.db")).unwrap().execute_batch(
            "CREATE TRIGGER fail_notice BEFORE INSERT ON supervisor_queue
             BEGIN SELECT RAISE(FAIL, 'injected notification failure'); END;",
        ).unwrap();
        assert!(attention(&core, &dispatch, "fallback").is_err());
        let directory = root.join("verification-runs").join(&dispatch.id);
        std::fs::create_dir_all(&directory).unwrap();
        // The provider boundary exits immediately; the real launch, dispatch
        // claim, notice and child cleanup still run against the actual store.
        let mut command = Command::new("sh");
        command.args(["-c", "exit 1"]);
        let result = launch_with_preparation(
            &core, &task, &dispatch, directory.join("verdict.json"),
            move |_| Ok((command, Some("fallback".into()))),
        );
        let handle = result.expect("a notice failure must not report a failed launch")
            .expect("the verifier was launched");
        handle.join().unwrap();
        let recovered = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
        assert!(!owns(&root, &recovered));
        assert_eq!(recovered.state, cas_types::VerificationDispatchState::Pending);
    }

    /// GH #2141: Codex's safety filter refused a review of a credential-read
    /// guard; the harness exited 1 and the dispatch was released with no
    /// review. The refusal is now the dispatch's infrastructure ERROR, its
    /// output stays on disk, and the supervisor gets the exact-dispatch route.
    #[test]
    fn provider_policy_refusal_is_an_infrastructure_error_with_a_supervisor_route_gh_2141() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        env.set("CAS_ROOT", &root);
        let daemon = crate::mcp::daemon::EmbeddedDaemon::new(crate::mcp::daemon::EmbeddedDaemonConfig {
            cas_root: root.clone(),
            ..Default::default()
        });
        let core = CasCore::with_daemon(root.clone(), None, Some(std::sync::Arc::new(daemon)));
        let owner = cas_types::Agent::new_with_role(
            "owner".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
        );
        core.open_agent_store().unwrap().register(&owner).unwrap();
        let task = Task::new("cas-2141-guard".into(), "Credential-read guard".into());
        core.open_task_store().unwrap().add(&task).unwrap();
        let dispatch = cas_store::create_verification_dispatch_bound(
            &root, &task.id, &owner.id, &owner.id,
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10), false,
        ).unwrap();
        let directory = root.join("verification-runs").join(&dispatch.id);
        std::fs::create_dir_all(&directory).unwrap();
        let output = directory.join("verdict.json");
        // The captured provider failure from the incident.
        let mut refused = Command::new("sh");
        refused.args(["-c", "echo 'ERROR: This content was flagged for possible cybersecurity risk. If this seems wrong, try rephrasing your request.' >&2; exit 1"]);
        launch_with_preparation(&core, &task, &dispatch, output.clone(), move |_| Ok((refused, None)))
            .unwrap().unwrap().join().unwrap();

        let verdict = cas_store::get_verification_for_dispatch(&root, &dispatch.id).unwrap().unwrap();
        assert_eq!(verdict.status, VerificationStatus::Error);
        assert!(verdict.summary.starts_with(PROVIDER_POLICY_CODE), "{}", verdict.summary);
        let pending = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
        assert_eq!(pending.state, cas_types::VerificationDispatchState::Pending);
        assert!(std::fs::read_to_string(output.with_file_name("harness.log")).unwrap().contains(PROVIDER_POLICY_REFUSAL));
        let prompts = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap();
        assert_eq!(prompts.len(), 1, "{prompts:?}");
        assert!(prompts[0].prompt.contains(&format!("dispatch_id={}", dispatch.id)), "{:?}", prompts[0]);
    }

    #[test]
    fn automatic_verifier_records_exact_dispatch_without_hand_launch_cas_381f() {
        let child = std::env::var_os("CAS_TEST_PREMERGE_ACCOUNT_CHILD").is_some();
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        // cas-3f0a: the review broker binds the real GUI socket path,
        // HOME/.cas/factory-<session>.gui.sock (ui/factory/session.rs:252).
        // Production refuses an over-long one at startup (validate_daemon_start,
        // session.rs:238); the fixture's HOME under the capped runner's deep
        // TMPDIR exceeds macOS SUN_LEN, so HOME is a short alias of it.
        let socket_home = crate::test_paths::short_socket_tempdir();
        let home_alias = socket_home.path().join("h");
        std::os::unix::fs::symlink(env.home(), &home_alias).unwrap();
        env.set("HOME", &home_alias);
        let bin = env.home().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        env.set("CAS_TEST_HUB_PATH", &bin);
        env.remove("CODEX_HOME");
        // cas-c386: the default home is logged out and the logged-in account
        // has no usage yet, so no account is recommended on capacity alone.
        std::fs::create_dir_all(env.home().join(".codex")).unwrap();
        let account = env.home().join(".codex-review");
        std::fs::create_dir_all(account.join("sessions")).unwrap();
        std::fs::write(account.join("auth.json"), "{}").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let script = bin.join("codex");
        std::fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = login ]; then
    [ -f "$CODEX_HOME/auth.json" ] || exit 1
    echo 'Logged in using ChatGPT'
    exit 0
fi
if [ "$CAS_TEST_VERIFIER_STATUS" = late_error ] || [ "$CAS_TEST_VERIFIER_STATUS" = harness_failed ] || [ "$CAS_TEST_VERIFIER_STATUS" = malformed ] || [ "$CAS_TEST_VERIFIER_STATUS" = target_move ] || [ "$CAS_TEST_VERIFIER_STATUS" = no_code ]; then
    while [ ! -f "$CAS_TEST_VERIFIER_GATE" ]; do sleep 0.05; done
fi
if [ "$CAS_TEST_VERIFIER_STATUS" = target_move ] && ! mkdir "$CAS_TEST_VERIFIER_GATE-reviewed" 2>/dev/null; then
    echo '{"status":"supervisor_call","summary":"Review the new target before publication","confidence":0.95,"files_reviewed":"README.md","issues":"[]"}'
    exit 0
fi
if [ "$CAS_TEST_VERIFIER_STATUS" = late_error ]; then
    echo 'Task is already Closed' >&2
    exit 1
fi
if [ "$CAS_TEST_VERIFIER_STATUS" = harness_failed ]; then
    echo 'old diagnostic excluded from bounded tail' >&2
    i=0; while [ "$i" -lt 12 ]; do echo 'retrying transport' >&2; i=$((i+1)); done
    echo '401 Unauthorized: Missing bearer or basic authentication in header' >&2
    exit 1
fi
[ -f "$CODEX_HOME/auth.json" ] || { echo '401 Unauthorized: Missing bearer' >&2; exit 1; }
[ "$CAS_INTERNAL_LLM" = 1 ] && [ -z "$CAS_AGENT_ROLE$CAS_AGENT_ID$CAS_SESSION_ID" ] || exit 2
cat "$CAS_TEST_VERIFIER_RESPONSE"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // cas-bdde: a logged-in Claude CLI, written before any child runs.
        let claude = bin.join("claude");
        std::fs::write(
            &claude,
            "#!/bin/sh\n[ \"$1\" = --version ] && { echo '9.9.9 (Claude Code)'; exit 0; }\necho '{\"loggedIn\":true}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        // cas-40040: the read-only sandbox preflight probes `bwrap` on PATH.
        // This test proves dispatch recording, not the host's namespace
        // policy, so the isolated PATH answers the probe as a capable host.
        let bwrap = bin.join("bwrap");
        std::fs::write(&bwrap, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&bwrap, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Registry login probes use the CLI on PATH. Isolate that PATH in a
        // child test process, never mutate the library test process's PATH.
        if !child {
            let path =
                std::env::join_paths([bin.clone(), "/usr/bin".into(), "/bin".into()]).unwrap();
            let module = module_path!().split_once("::").unwrap().1;
            let result = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &format!("{module}::automatic_verifier_records_exact_dispatch_without_hand_launch_cas_381f"), "--nocapture"])
                .env("PATH", path)
                .env("CAS_TEST_PREMERGE_ACCOUNT_CHILD", "1")
                .output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            eprintln!("{}", String::from_utf8_lossy(&result.stderr));
            return;
        }
        for status in [
            "harness_failed",
            "approved",
            "rejected",
            "supervisor_call",
            "malformed",
            "late_error",
            "target_move",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path();
            let run = |args: &[&str]| super::super::tests::run(repo, args);
            run(&["init", "-q", "-b", "epic/train"]);
            run(&["config", "user.name", "Test"]);
            run(&["config", "user.email", "test@example.invalid"]);
            let root = crate::store::init_cas_dir(repo).unwrap();
            std::fs::write(repo.join(".git/info/exclude"), ".cas\n").unwrap();
            std::fs::write(root.join("config.toml"), "[worktrees]\nenabled = true\n[qa]\nmode = 'batch'\n[verification]\nenabled = true\n[code_review]\nowner = 'worker'\n").unwrap();
            super::super::tests::commit_file(repo, "README.md", "heading\n\nbase\n\nfooter\n", "base");
            let lane = root.join("worktrees/worker");
            run(&[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/worker",
                lane.to_str().unwrap(),
            ]);
            let source =
                super::super::tests::commit_file(&lane, "README.md", "heading\n\ndelivery\n\nfooter\n", "delivery");
            env.set("CAS_ROOT", &root);
            env.set("CAS_AGENT_ROLE", "supervisor");
            let daemon =
                crate::mcp::daemon::EmbeddedDaemon::new(crate::mcp::daemon::EmbeddedDaemonConfig {
                    cas_root: root.clone(),
                    ..Default::default()
                });
            let core = CasCore::with_daemon(root.clone(), None, Some(std::sync::Arc::new(daemon)));
            let owner = cas_types::Agent::new_with_role(
                "owner".into(),
                "supervisor".into(),
                cas_types::AgentRole::Supervisor,
            );
            core.open_agent_store().unwrap().register(&owner).unwrap();
            core.bind_agent_identity(
                owner.id.clone(),
                crate::mcp::AgentIdentitySource::ServerInternal,
            )
            .unwrap();
            let mut task = Task::new("cas-381f-auto".into(), "Automatic verifier fixture".into());
            // This fixture has a local-only target and no publication remote.
            task.delivery_mode = cas_types::DeliveryMode::LocalMerge;
            task.status = cas_types::TaskStatus::AwaitingMerge;
            task.depth = cas_types::TaskDepth::Deep;
            task.assignee = Some("worker".into());
            task.acceptance_criteria = "Preserve the delivery".into();
            task.notes =
                "Supervisor adversarial note: preserve drafts. Receipt: scoped worker proof."
                    .into();
            task.deliverables.factory_branch_anchor = Some(source.clone());
            task.deliverables.parked_branch = Some("factory/worker".into());
            task.deliverables.parked_close_reason = Some("PASS fixture".into());
            // The delivery lane the approved merge lands on is the parent epic's.
            let mut epic = Task::new("cas-381f-epic".into(), "Train".into());
            epic.task_type = cas_types::TaskType::Epic;
            epic.branch = Some("epic/train".into());
            let tasks = core.open_task_store().unwrap();
            tasks.add(&epic).unwrap();
            tasks.add(&task).unwrap();
            tasks
                .add_dependency(&cas_types::Dependency::new(
                    task.id.clone(),
                    epic.id.clone(),
                    cas_types::DependencyType::ParentChild,
                ))
                .unwrap();
            core.open_worktree_store()
                .unwrap()
                .add(&cas_types::Worktree::new(
                    "factory/worker".into(),
                    "factory/worker".into(),
                    "epic/train".into(),
                    lane,
                ))
                .unwrap();
            let response = if status == "malformed" {
                "not a verdict".into()
            } else {
                let verdict_status = if status == "target_move" { "approved" } else { status };
                let verdict = serde_json::json!({"status": verdict_status, "summary": format!("{status}: operator draft probe"),
                    "confidence":0.95,"files_reviewed":"README.md","issues":"[]",
                    "task_id":"cas-other","dispatch_id":"vdispatch-other", "verifier_capability":"forged",
                    "verification_type":"epic", "duration_ms":0}).to_string();
                // cas-2b80b: `claude -p` prints the model's final text, which
                // can lead with prose and fence the object (serde: "expected
                // value at line 1 column 1"). The verdict is the last object.
                if verdict_status == "approved" {
                    format!("Review complete; {{draft}} preserved.\n\n```json\n{verdict}\n```\n")
                } else {
                    verdict
                }
            };
            let response_path = root.join("provider-response.json");
            std::fs::write(&response_path, response).unwrap();
            let gate = root.join("verifier-gate");
            env.set("CAS_TEST_VERIFIER_GATE", &gate);
            env.set("CAS_TEST_VERIFIER_RESPONSE", &response_path);
            env.set("CAS_TEST_VERIFIER_STATUS", status);
            // cas-f2dc: drive the real merge refusal with automatic review on;
            // the refusal leaves the one running verifier's dispatch in place.
            assert!(
                core.prepare_premerge_verification(&task, repo, "factory/worker", "epic/train").is_err(),
                "{status}: an unreviewed delivery cannot merge"
            );
            let dispatch = cas_store::get_latest_verification_dispatch(&root, &task.id)
                .unwrap()
                .unwrap();
            let candidate = candidate_for_dispatch(&root, &dispatch).unwrap().unwrap();
            let directory = std::fs::read_dir(root.join("verification-runs").join(&dispatch.id))
                .unwrap().next().unwrap().unwrap().path();
            let output = directory.join("verdict.json");
            assert!(
                core.emit_premerge_review_handoff(&task, &dispatch)
                    .unwrap()
                    .is_none(),
                "a retried merge cannot launch a second reviewer"
            );
            if status == "late_error" {
                // The dispatch settles while the verifier is still in flight.
                let tasks = core.open_task_store().unwrap();
                let mut settled = tasks.get(&task.id).unwrap();
                settled.status = cas_types::TaskStatus::Closed;
                tasks.update(&settled).unwrap();
            }
            if status == "target_move" {
                // GH #1837: the approved review is about to finish against an
                // old target. The adjacent edit touches the delivered path,
                // so the stale result cannot approve or publish a new candidate.
                super::super::tests::commit_file(repo, "README.md",
                    "new heading\n\nbase\n\nfooter\n", "target moved during review");
            }
            std::fs::write(&gate, "go").unwrap();
            let child_done = || {
                !core.open_agent_store().unwrap().list(None).unwrap().iter().any(|agent| {
                    agent.metadata.contains_key(OWNER_KEY)
                        && agent.status == cas_types::AgentStatus::Active
                })
            };
            // The verdict paths finish their merge or notice around the child's
            // exit, so also wait for each status's own end state.
            let settled = || {
                child_done()
                    && match status {
                        "approved" => Command::new("git")
                            .current_dir(repo)
                            .args(["merge-base", "--is-ancestor", &source, "epic/train"])
                            .status()
                            .is_ok_and(|s| s.success())
                            && !crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap().is_empty(),
                        "target_move" => cas_store::get_latest_verification_dispatch(&root, &task.id)
                            .unwrap().is_some_and(|latest| latest.id != dispatch.id
                                && cas_store::get_verification_for_dispatch(&root, &latest.id).unwrap()
                                    .is_some_and(|verdict| verdict.status == VerificationStatus::SupervisorCall)),
                        "rejected" | "supervisor_call" => !crate::store::open_prompt_queue_store(&root)
                            .unwrap()
                            .peek_all(10)
                            .unwrap()
                            .is_empty(),
                        _ => true,
                    }
            };
            for _ in 0..600 {
                if settled() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert!(
                settled(),
                "{status}: verifier never finished; child_done={} dispatch={:?} prompts={:?}",
                child_done(),
                cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap().state,
                crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap()
            );
            let claimed = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
            if status == "target_move" {
                assert_eq!(claimed.state, cas_types::VerificationDispatchState::Invalidated);
                let latest = cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().unwrap();
                assert_ne!(latest.id, dispatch.id);
                assert_ne!(latest.verifier_agent_id, claimed.verifier_agent_id);
                let verdict = cas_store::get_verification_for_dispatch(&root, &latest.id).unwrap().unwrap();
                assert_eq!(verdict.status, VerificationStatus::SupervisorCall);
                assert_eq!(verdict.provenance, cas_types::VerificationProvenance::TaskVerifier);
                assert!(!Command::new("git").current_dir(repo)
                    .args(["merge-base", "--is-ancestor", &source, "epic/train"])
                    .status().unwrap().success());
                continue;
            }
            if matches!(status, "harness_failed" | "malformed") {
                assert_eq!(claimed.state, cas_types::VerificationDispatchState::Pending);
                assert!(!owns(&root, &claimed));
            } else {
                assert!(owns(&root, &claimed));
            }
            let verdict = cas_store::get_verification_for_dispatch(&root, &dispatch.id).unwrap();
            if matches!(status, "malformed" | "harness_failed" | "late_error") {
                assert!(verdict.is_none(), "failed transport cannot approve");
            } else {
                let verdict = verdict.unwrap_or_else(|| {
                    panic!(
                        "{status}: no verdict; output {:?}",
                        crate::store::open_prompt_queue_store(&root)
                            .unwrap()
                            .peek_all(10)
                            .unwrap()
                    )
                });
                assert_eq!(verdict.dispatch_id.as_deref(), Some(dispatch.id.as_str()));
                assert_eq!(
                    verdict.provenance,
                    cas_types::VerificationProvenance::TaskVerifier
                );
                assert_eq!(verdict.agent_id, claimed.verifier_agent_id);
            }
            let body = std::fs::read_to_string(output.with_file_name("prompt.txt")).unwrap();
            assert!(body.contains(&candidate.proof.worktree_root));
            assert!(body.contains("preserve drafts"));
            assert!(body.contains("scoped worker proof"));
            // cas-e17a: the verifier reads the evidence where park wrote it.
            let base = crate::config::resolved_factory_artifacts_root(None);
            let [evidence, _] = crate::config::factory_task_artifact_dirs(&root, &base, &task.id);
            assert!(body.contains(&evidence.display().to_string()), "{status}: {}", evidence.display());
            let prompts = crate::store::open_prompt_queue_store(&root)
                .unwrap()
                .peek_all(20)
                .unwrap();
            if status == "approved" {
                assert!(
                    super::super::tests::run(
                        repo,
                        &["merge-base", "--is-ancestor", &source, "epic/train"]
                    )
                    .is_empty()
                );
                // GH #2365: the landed merge opens the supervisor's next step,
                // so it wakes it exactly once.
                assert_eq!(prompts.len(), 1, "a landed automatic merge wakes the supervisor once: {prompts:?}");
                assert!(prompts[0].prompt.contains(&dispatch.id), "{prompts:?}");
            } else if status == "late_error" {
                // A late verifier error on a settled dispatch stays silent.
                assert!(prompts.is_empty(), "no recovery notice: {prompts:?}");
            } else {
                assert_eq!(
                    prompts.len(),
                    1,
                    "{status}: exactly one actionable notice: {prompts:?}"
                );
                assert!(
                    prompts[0].prompt.contains(match status {
                        // The refused output itself, not just the parse error.
                        "malformed" => "not a verdict",
                        "harness_failed" =>
                            "401 Unauthorized: Missing bearer or basic authentication in header",
                        _ => "operator draft probe",
                    }),
                    "{status}: {:?}",
                    prompts[0]
                );
                if status == "harness_failed" {
                    let diagnostics =
                        std::fs::read_to_string(output.with_file_name("harness.log")).unwrap();
                    assert!(
                        !prompts[0]
                            .prompt
                            .contains(diagnostics.lines().next().unwrap())
                    );
                }
            }
            if status == "supervisor_call" {
                // cas-d5bb / cas-44eb: an answer is context for the verifier,
                // not a supervisor-authored final approval.
                let question = cas_store::get_verification_for_dispatch(&root, &dispatch.id)
                    .unwrap().unwrap().summary;
                let answer = "Scope reviewed: preserve drafts and use the existing scoped receipt.";
                std::fs::write(&response_path, serde_json::json!({
                    "status":"approved", "summary":"Independent verdict after scope answer",
                    "confidence":0.95, "files_reviewed":"README.md", "issues":"[]",
                }).to_string()).unwrap();
                env.set("CAS_TEST_VERIFIER_STATUS", "approved");
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                let request = serde_json::from_value(serde_json::json!({
                    "task_id":task.id, "dispatch_id":dispatch.id,
                    "status":"supervisor_call", "summary":answer,
                })).unwrap();
                runtime.block_on(core.cas_verification_add(
                    rmcp::handler::server::wrapper::Parameters(request),
                )).unwrap();
                let resumed = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
                assert_eq!(resumed.id, dispatch.id);
                assert_ne!(resumed.verifier_agent_id, claimed.verifier_agent_id);
                for _ in 0..600 {
                    if settled() { break; }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                assert!(settled());
                let final_dispatch = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
                let final_verdict = cas_store::get_verification_for_dispatch(&root, &dispatch.id).unwrap().unwrap();
                assert_eq!(final_dispatch.state, cas_types::VerificationDispatchState::Resolved);
                assert_eq!(final_verdict.status, VerificationStatus::Approved);
                assert_eq!(final_verdict.provenance, cas_types::VerificationProvenance::TaskVerifier);
                assert_eq!(final_verdict.agent_id, resumed.verifier_agent_id);
                let conversation = std::fs::read_dir(root.join("verification-runs").join(&dispatch.id))
                    .unwrap().map(|entry| entry.unwrap().path().join("prompt.txt"))
                    .filter(|path| path != &output.with_file_name("prompt.txt"))
                    .map(|path| std::fs::read_to_string(path).unwrap())
                    .find_map(|body| body.lines().filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                        .find_map(|value| value.get("supervisor_calls").cloned()))
                    .unwrap();
                let summaries = conversation.as_array().unwrap().iter()
                    .map(|call| call["summary"].as_str().unwrap()).collect::<Vec<_>>();
                assert!(summaries.contains(&question.as_str()));
                assert!(summaries.contains(&answer));
                assert!(core.prepare_premerge_verification(&task, repo, "factory/worker", "epic/train").unwrap().is_some());
            }
            if status == "harness_failed" {
                let old_child = dispatch.verifier_agent_id.as_deref().unwrap();
                let old_stderr = std::fs::read(output.with_file_name("harness.log")).unwrap();
                std::fs::write(&response_path, serde_json::json!({
                    "status":"approved", "summary":"replacement reviewed the candidate",
                    "confidence":0.95, "files_reviewed":"README.md", "issues":"[]",
                }).to_string()).unwrap();
                env.set("CAS_TEST_VERIFIER_STATUS", "approved");
                // GH #2135: the first attempt's deadline lapses before the retry,
                // so the replacement must refresh and review the same dispatch.
                rusqlite::Connection::open(root.join("cas.db")).unwrap().execute(
                    "UPDATE verification_dispatches SET deadline_at = ?2 WHERE id = ?1",
                    rusqlite::params![dispatch.id, (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339()],
                ).unwrap();
                // Retry the merge's automatic handoff once, retaining its join
                // handle so completion needs no new timer or polling loop.
                let handle = core.emit_premerge_review_handoff(&task, &claimed).unwrap().unwrap();
                let replacement = cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap().unwrap();
                assert_eq!(replacement.id, dispatch.id);
                assert_ne!(replacement.verifier_agent_id.as_deref(), Some(old_child));
                assert!(core.emit_premerge_review_handoff(&task, &replacement).unwrap().is_none());
                let stale = CasCore::with_daemon(root.clone(), None, core.daemon.clone());
                stale.bind_agent_identity(old_child.into(), crate::mcp::AgentIdentitySource::ServerInternal).unwrap();
                let request = serde_json::from_value(serde_json::json!({
                    "task_id":task.id, "dispatch_id":dispatch.id, "status":"rejected",
                    "summary":"stale child's competing verdict", "issues":"[]",
                })).unwrap();
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                assert!(runtime.block_on(stale.cas_verification_add(rmcp::handler::server::wrapper::Parameters(request))).is_err());
                handle.join().unwrap();
                let verdict = cas_store::get_verification_for_dispatch(&root, &dispatch.id).unwrap().unwrap();
                assert_eq!(verdict.status, VerificationStatus::Approved);
                assert_eq!(verdict.agent_id, replacement.verifier_agent_id);
                assert!(core.prepare_premerge_verification(&task, repo, "factory/worker", "epic/train").unwrap().is_some());
                assert_eq!(std::fs::read(output.with_file_name("harness.log")).unwrap(), old_stderr);
            }
            eprintln!(
                "AUTOMATIC_VERIFIER row={status} hand_launches=0 dispatch={} candidate={}",
                dispatch.id, candidate.proof.head_commit
            );
        }
        // GH #1867: a no-code close dispatch has no candidate proof. The same
        // daemon-owned verifier reviews it and records the exact verdict, and
        // the owner hears about either outcome. GH #2383: an approval of a
        // train delivery whose worker blocked on the wait closes it for the
        // train owner; a real gate stays held and is recorded on the task.
        for case in ["approved", "foreign-train", "rejected"] {
            let status = if case == "rejected" { "rejected" } else { "approved" };
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path();
            super::super::tests::run(repo, &["init", "-q", "-b", "main"]);
            let root = crate::store::init_cas_dir(dir.path()).unwrap();
            std::fs::write(repo.join(".git/info/exclude"), ".cas\n").unwrap();
            super::super::tests::commit_file(repo, "README.md", "base\n", "base");
            std::fs::write(root.join("config.toml"), "[verification]\nenabled = true\n[worktrees]\nenabled = false\n[qa]\nmode = 'batch'\nevidence_gate = false\nindependent_pass = false\n").unwrap();
            env.set("CAS_ROOT", &root);
            env.set("CAS_AGENT_ROLE", "worker");
            env.set("CAS_FACTORY_MODE", "1");
            env.set("CAS_TEST_VERIFIER_STATUS", "no_code");
            let gate = root.join("verifier-gate");
            env.set("CAS_TEST_VERIFIER_GATE", &gate);
            let response = root.join("provider-response.json");
            std::fs::write(&response, serde_json::json!({"status": status,
                "summary": "no-code proof reviewed", "confidence": 0.9,
                "files_reviewed": "", "issues": "[]"}).to_string()).unwrap();
            env.set("CAS_TEST_VERIFIER_RESPONSE", &response);
            let daemon = crate::mcp::daemon::EmbeddedDaemon::new(
                crate::mcp::daemon::EmbeddedDaemonConfig { cas_root: root.clone(), ..Default::default() },
            );
            let core = CasCore::with_daemon(root.clone(), None, Some(std::sync::Arc::new(daemon)));
            let mut owner = cas_types::Agent::new_with_role(
                "owner".into(), "supervisor".into(), cas_types::AgentRole::Supervisor,
            );
            owner.factory_session = Some("no-code-session".into());
            core.open_agent_store().unwrap().register(&owner).unwrap();
            let mut worker = cas_types::Agent::new_with_role(
                "worker".into(), "worker".into(), cas_types::AgentRole::Worker,
            );
            worker.factory_session = owner.factory_session.clone();
            core.open_agent_store().unwrap().register(&worker).unwrap();
            core.bind_agent_identity(worker.id.clone(), crate::mcp::AgentIdentitySource::ServerInternal).unwrap();
            // Factory reviews cross the real local control boundary. A session
            // name alone cannot stand in for the daemon that claims the review.
            use crate::ui::factory::{ClientMessage, DaemonMessage, SessionState,
                FRAME_HEADER_SIZE, PROTOCOL_VERSION, decode_length, encode_message,
                gui_socket_path, local_gui_capabilities};
            use std::io::{Read, Write};
            use std::os::fd::AsRawFd;
            let session = owner.factory_session.clone().unwrap();
            let socket = gui_socket_path(&session);
            std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
            let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
            let broker_root = root.clone();
            let (claimed, claim) = std::sync::mpsc::channel();
            let broker = std::thread::spawn(move || {
                let mut ready = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                assert!(unsafe { libc::poll(&mut ready, 1, 10000) } > 0);
                let (mut client, _) = listener.accept().unwrap();
                client.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                client.set_write_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                client.write_all(&encode_message(&DaemonMessage::Welcome {
                    session_name: session.clone(),
                    state: SessionState { focused_pane: None, panes: vec![], epic_id: None,
                        epic_title: None, cols: 80, rows: 24 },
                    scrollback: None, protocol_version: PROTOCOL_VERSION,
                    capabilities: local_gui_capabilities(), pane_bootstrap: vec![],
                }).unwrap()).unwrap();
                let mut header = [0; FRAME_HEADER_SIZE];
                client.read_exact(&mut header).unwrap();
                let mut body = vec![0; decode_length(&header)];
                client.read_exact(&mut body).unwrap();
                let ClientMessage::LaunchVerifier { dispatch_id } = serde_json::from_slice(&body).unwrap()
                    else { panic!("unexpected factory control"); };
                let review = super::super::factory_launch::launch(&broker_root, &session, &dispatch_id)
                    .unwrap().unwrap();
                client.write_all(&encode_message(&DaemonMessage::VerifierLaunched {
                    dispatch_id: dispatch_id.clone(),
                }).unwrap()).unwrap();
                claimed.send(dispatch_id).unwrap();
                review.join().unwrap();
                std::fs::remove_file(socket).unwrap();
            });
            let mut task = Task::new("cas-e50f-no-code".into(), "No-code close".into());
            task.status = cas_types::TaskStatus::InProgress;
            task.task_type = cas_types::TaskType::Chore;
            task.assignee = Some(worker.name.clone());
            task.risk = vec![cas_types::TaskRisk::None];
            task.execution_note = Some("no-code".into());
            task.external_ref = Some("artifact:cas-e50f/report".into());
            core.open_task_store().unwrap().add(&task).unwrap();
            let mut train = Task::new("cas-2383-train".into(), "Train".into());
            train.task_type = cas_types::TaskType::Epic;
            train.epic_verification_owner = Some(if case == "foreign-train" { "other-owner".into() } else { owner.id.clone() });
            core.open_task_store().unwrap().add(&train).unwrap();
            core.open_task_store().unwrap().add_dependency(&cas_types::Dependency::new(
                task.id.clone(), train.id.clone(), cas_types::DependencyType::ParentChild,
            )).unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let request = serde_json::from_value(serde_json::json!({
                "id": task.id, "reason": "External artifact published; no source changes",
            })).unwrap();
            let close = runtime.block_on(crate::async_runtime::heap_future(|| {
                core.cas_task_close(rmcp::handler::server::wrapper::Parameters(request))
            })).unwrap();
            assert_eq!(close.is_error, Some(true), "initial close must await review: {close:?}");
            let dispatch = cas_store::get_latest_verification_dispatch(&root, &task.id).unwrap()
                .unwrap_or_else(|| panic!("real close did not dispatch: {close:?}"));
            assert_eq!(claim.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), dispatch.id);
            let dispatch = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
            assert_eq!(dispatch.requester_agent_id, worker.id);
            assert_eq!(dispatch.owner_agent_id, owner.id);
            assert!(core.open_task_store().unwrap().get(&task.id).unwrap().pending_verification);
            assert!(owns(&root, &dispatch));
            assert_ne!(dispatch.verifier_agent_id.as_deref(), Some(worker.id.as_str()));
            assert_ne!(dispatch.verifier_agent_id.as_deref(), Some(owner.id.as_str()));
            // The worker records the documented verification wait (GH #2383).
            let wait = crate::mcp::tools::core::task::block_hold::verification_wait_note(&dispatch.id);
            let blocked = runtime.block_on(core.cas_task_update(rmcp::handler::server::wrapper::Parameters(
                serde_json::from_value(serde_json::json!({"id": task.id, "status": "blocked", "notes": wait})).unwrap(),
            ))).unwrap();
            assert_ne!(blocked.is_error, Some(true), "{blocked:?}");
            std::fs::write(&gate, "go").unwrap();
            broker.join().unwrap();
            let prompts = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap();
            let verdict = cas_store::get_verification_for_dispatch(&root, &dispatch.id).unwrap()
                .unwrap_or_else(|| panic!("{status}: no verdict; notices {prompts:?}"));
            assert_eq!(verdict.provenance, cas_types::VerificationProvenance::TaskVerifier);
            assert_eq!(verdict.dispatch_id.as_deref(), Some(dispatch.id.as_str()));
            assert_eq!(verdict.status.to_string(), status);
            assert_eq!(verdict.agent_id,
                cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap().verifier_agent_id);
            let closed = core.open_task_store().unwrap().get(&task.id).unwrap();
            assert_eq!(closed.status == cas_types::TaskStatus::Closed, case == "approved",
                "{case}: {:?} notes {}; notices {prompts:?}", closed.status, closed.notes);
            if case == "foreign-train" {
                assert_eq!(closed.status, cas_types::TaskStatus::Blocked);
                let gate = crate::mcp::tools::core::task::lifecycle::merge_close::no_delivery_close_hold(&closed.notes, &dispatch.id);
                assert!(gate.is_some_and(|gate| gate.contains("G-113")), "{case}: {}", closed.notes);
                continue;
            }
            // Approval closes and tells the worker; a held verdict queues its
            // own notice. Either way one row names the dispatch.
            assert_eq!(
                prompts.iter().filter(|row| row.prompt.contains(&dispatch.id)).count(),
                1,
                "{status}: {prompts:?}"
            );
            eprintln!("NO_CODE_CLOSE case={case} status={status} dispatch={} verifier={:?} verdict={}",
                dispatch.id, verdict.agent_id, verdict.id);
        }
        // With no eligible home left, the refusal names each home's state
        // instead of blaming only the logged-out default home.
        std::fs::write(
            account.join("sessions/usage.jsonl"),
            serde_json::json!({
                "timestamp": chrono::Utc::now(), "payload": {"rate_limits": {
                    "primary": {"used_percent": 100.0, "window_minutes": 10080,
                        "resets_at": (chrono::Utc::now() + chrono::Duration::days(2)).timestamp()}
                }}
            })
            .to_string(),
        )
        .unwrap();
        let refused = command(env.home(), &env.home().join("verdict.json")).unwrap_err();
        for needle in [".codex-review", "100%", " ET", "not logged in"] {
            assert!(refused.contains(needle), "{needle}: {refused}");
        }
        // cas-bdde: with every Codex home ineligible, an eligible Claude home
        // runs the same verifier, and the notice names harness and account.
        let claude_home = env.home().join(".claude-review");
        std::fs::create_dir_all(&claude_home).unwrap();
        let reset = (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
        std::fs::write(
            claude_home.join(".cas-claude-usage.json"),
            serde_json::json!({"identity": null, "observed_at": chrono::Utc::now(), "windows": [
                {"name": "5h", "used_percent": 10.0, "reset_at": reset, "spend_dollars": null},
                {"name": "weekly", "used_percent": 20.0, "reset_at": reset, "spend_dollars": null},
            ]})
            .to_string(),
        )
        .unwrap();
        let (fallback, notice) = command(env.home(), &env.home().join("verdict.json")).unwrap();
        assert!(fallback.get_args().any(|arg| arg == "-p"), "{fallback:?}");
        assert!(
            // HOME is the short socket alias (cas-3f0a); compare the directories.
            fallback.get_envs().any(|(key, value)| key == "CLAUDE_CONFIG_DIR"
                && value.is_some_and(|dir| std::fs::canonicalize(dir).ok() == std::fs::canonicalize(&claude_home).ok())),
            "{fallback:?}"
        );
        let notice = notice.expect("fallback notice");
        assert!(notice.contains("Claude") && notice.contains("review"), "{notice}");
    }
}
