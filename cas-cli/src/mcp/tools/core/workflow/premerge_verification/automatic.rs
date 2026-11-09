//! Daemon-owned transport for the existing task verifier. Only the final
//! verdict arguments cross the harness boundary; authority stays server-side.
use super::*;
use std::process::Stdio;

const OWNER_KEY: &str = "automatic_premerge_dispatch";

pub(super) fn owns(root: &Path, dispatch: &VerificationDispatch) -> bool {
    dispatch.verifier_agent_id.as_deref().is_some_and(|id| {
        crate::store::open_agent_store(root)
            .ok()
            .and_then(|store| store.get(id).ok())
            .is_some_and(|agent| agent.metadata.get(OWNER_KEY) == Some(&dispatch.id))
    })
}

fn prompt(core: &CasCore, task: &Task, dispatch: &VerificationDispatch) -> Result<String, String> {
    let store = core.open_task_store().map_err(|e| e.to_string())?;
    let parent = store.get_parent_epic(&task.id).map_err(|e| e.to_string())?;
    Ok(format!(
        "{}\n\n{}\n\nDaemon-owned task-verifier invocation. The task and parent below include acceptance criteria, receipts and supervisor adversarial notes. Review the bound candidate using read-only filesystem commands. Do not edit, register, close, spawn agents, or call MCP. Return exactly one JSON object containing the arguments to verification action=add: status (approved, rejected, supervisor_call, or error), summary, confidence, files_reviewed (comma-separated), issues (JSON-encoded array). Omit task_id, dispatch_id and verifier_capability: the daemon supplies the immutable dispatch identity and records your verdict through the existing sealed handoff. This transport replaces only the MCP recording step above. Missing evidence requires supervisor_call.\nTask evidence directory (LEDGER.md, logs and captures; read this exact path instead of deriving <project-key>): {}. If evidence is absent there, name this absolute path in the verdict.\nTask: {}\nParent: {}\nDispatch: {}\n",
        include_str!("../../../../../builtins/agents/task-verifier.body.md"),
        include_str!(
            "../../../../../builtins/skills/cas-qa-craft/references/verifier-evidence-gate.md"
        ),
        crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(
            &core.cas_root,
            task,
        )?
        .display(),
        serde_json::to_string(task).map_err(|e| e.to_string())?,
        serde_json::to_string(&parent).map_err(|e| e.to_string())?,
        serde_json::to_string(dispatch).map_err(|e| e.to_string())?,
    ))
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
                        "{codex}; Claude accounts could not be read; launch the task-verifier yourself"
                    ));
                }
                Err(claude) => {
                    return Err(format!("{codex}; {claude}; launch the task-verifier yourself"));
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
            command
                .args(["exec", "--sandbox", "read-only", "--skip-git-repo-check"])
                .arg("--model")
                .arg(model)
                .arg("-c")
                .arg(format!("model_reasoning_effort=\"{}\"", effort.as_str()))
                .args(["-c", "project_doc_max_bytes=0", "--output-last-message"])
                .arg(output)
                .arg("-");
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

/// Claim synchronously, then do provider work off the MCP/control loop. The
/// existing dispatch claim prevents a second launch, including manual races.
pub(super) fn launch(
    core: &CasCore,
    task: &Task,
    dispatch: &VerificationDispatch,
) -> Result<Option<std::thread::JoinHandle<()>>, String> {
    let proof = dispatch
        .repository
        .as_ref()
        .ok_or("candidate proof missing")?;
    let directory = core.cas_root.join("verification-runs").join(&dispatch.id);
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let output = directory.join("verdict.json");
    let (mut command, notice) = command(&core.cas_root, &output)?;
    command.current_dir(&proof.worktree_root);
    launch_command(core, task, dispatch, command, output, notice)
}

fn launch_command(
    core: &CasCore,
    task: &Task,
    dispatch: &VerificationDispatch,
    mut command: Command,
    output: PathBuf,
    notice: Option<String>,
) -> Result<Option<std::thread::JoinHandle<()>>, String> {
    let live = cas_store::get_verification_dispatch(&core.cas_root, &dispatch.id)
        .map_err(|e| e.to_string())?;
    if live.state != cas_types::VerificationDispatchState::Pending {
        return Ok(None);
    }
    let body = prompt(core, task, &live)?;
    let input = output.with_file_name("prompt.txt");
    std::fs::write(&input, body).map_err(|e| e.to_string())?;
    let mut child = cas_types::Agent::new_sub_agent(
        format!("task-verifier-{}", live.id),
        "task-verifier".into(),
        live.owner_agent_id.clone(),
    );
    // The daemon owns this identity and submits the harness's structured
    // verdict. The subprocess never receives a credential or this identity.
    child.pid = Some(std::process::id());
    crate::mcp::daemon::stamp_pid_fingerprint(&mut child, std::process::id());
    child.metadata.insert(OWNER_KEY.into(), live.id.clone());
    child.factory_session = core
        .open_agent_store()
        .map_err(|e| e.to_string())?
        .get(&live.owner_agent_id)
        .map_err(|e| e.to_string())?
        .factory_session;
    cas_store::issue_server_verifier_handoff(
        &core.cas_root,
        &task.id,
        &live.id,
        &live.owner_agent_id,
        &format!("daemon:{}", live.id),
    )
    .map_err(|e| e.to_string())?;
    cas_store::bind_server_verifier_handoff_and_register_child(
        &core.cas_root,
        &live.owner_agent_id,
        &child,
    )
    .map_err(|e| e.to_string())?;
    let worker = core.clone();
    let handle = std::thread::Builder::new().name(format!("verifier-{}", task.id)).spawn(move || {
        let result = review(&worker, &live, &child.id, &mut command, &input, &output);
        // The existing daemon expiry sweep owns timeout escalation and its
        // durable dedupe key; do not emit a second timeout wake here.
        if let Err(error) = result && chrono::Utc::now() < live.deadline_at && !resolved(&worker, &live) {
            let _ = attention(&worker, &live, &format!("Automatic verifier stopped: {error}. Review or recover this exact dispatch."));
        }
        if let Ok(agents) = worker.open_agent_store() {
            let _ = agents.graceful_shutdown(&child.id);
        }
    }).map_err(|e| e.to_string())?;
    if let Some(notice) = notice {
        attention(core, dispatch, &notice)?;
    }
    Ok(Some(handle))
}

/// A dispatch that already holds a verdict, or whose task is terminal, needs no
/// supervisor notice when a second verifier finds it settled.
fn resolved(core: &CasCore, dispatch: &VerificationDispatch) -> bool {
    cas_store::get_verification_for_dispatch(&core.cas_root, &dispatch.id).is_ok_and(|v| v.is_some()) || core
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
    std::fs::write(output.with_file_name("harness.log"), &result.stderr)
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        // Keep the full log on disk; notices carry only the final eight lines
        // within a 2 KiB window, so an echoed prompt cannot flood the owner.
        let tail =
            String::from_utf8_lossy(&result.stderr[result.stderr.len().saturating_sub(2048)..]);
        let mut lines = tail.lines().rev().take(8).collect::<Vec<_>>();
        lines.reverse();
        let tail =
            crate::mcp::tools::service::agent_search_system::system::redact_known_credentials(
                &lines.join("\n"),
            );
        return Err(format!(
            "harness exited {}; stderr tail ({}):\n{}",
            result.status,
            output.with_file_name("harness.log").display(),
            if tail.trim().is_empty() {
                "(empty)"
            } else {
                &tail
            },
        ));
    }
    let bytes = if output.exists() {
        std::fs::read(output).map_err(|e| e.to_string())?
    } else {
        result.stdout
    };
    let mut arguments: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid verifier response: {e}"))?;
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
    Ok(())
}

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
    // A readable receipt without a prompt: approval must not start another
    // supervisor turn merely to acknowledge an automatic continuation.
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
        let result = launch_command(
            &core, &task, &dispatch, command, directory.join("verdict.json"),
            Some("fallback".into()),
        );
        let handle = result.expect("a notice failure must not report a failed launch")
            .expect("the verifier was launched");
        handle.join().unwrap();
        let claimed = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
        assert!(owns(&root, &claimed));
        assert_eq!(claimed.state, cas_types::VerificationDispatchState::Claimed);
    }

    #[test]
    fn automatic_verifier_records_exact_dispatch_without_hand_launch_cas_381f() {
        let child = std::env::var_os("CAS_TEST_PREMERGE_ACCOUNT_CHILD").is_some();
        let mut env = crate::test_support::TestEnvGuard::temp_home();
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
if [ "$CAS_TEST_VERIFIER_STATUS" = late_error ]; then
    while [ ! -f "$CAS_TEST_VERIFIER_GATE" ]; do sleep 0.05; done
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
            super::super::tests::commit_file(repo, "README.md", "base\n", "base");
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
                super::super::tests::commit_file(&lane, "README.md", "delivery\n", "delivery");
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
                serde_json::json!({"status": status, "summary": format!("{status}: operator draft probe"),
                    "confidence":0.95,"files_reviewed":"README.md","issues":"[]",
                    "task_id":"cas-other","dispatch_id":"vdispatch-other", "verifier_capability":"forged",
                    "verification_type":"epic", "duration_ms":0}).to_string()
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
            let directory = root.join("verification-runs").join(&dispatch.id);
            std::fs::create_dir_all(&directory).unwrap();
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
            std::fs::write(&gate, "go").unwrap();
            let child_done = || {
                !core.open_agent_store().unwrap().list(None).unwrap().iter().any(|agent| {
                    agent.metadata.get(OWNER_KEY) == Some(&dispatch.id)
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
                            .is_ok_and(|s| s.success()),
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
            assert!(owns(&root, &claimed));
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
                assert!(
                    prompts.is_empty(),
                    "approval must not wake the supervisor: {prompts:?}"
                );
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
                        "malformed" => "invalid verifier response",
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
            eprintln!(
                "AUTOMATIC_VERIFIER row={status} hand_launches=0 dispatch={} candidate={}",
                dispatch.id, candidate.proof.head_commit
            );
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
        for needle in [".codex-review", "100%", " ET", "not logged in", "launch the task-verifier yourself"] {
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
            fallback.get_envs().any(|(key, value)| key == "CLAUDE_CONFIG_DIR"
                && value.is_some_and(|dir| std::path::Path::new(dir) == claude_home)),
            "{fallback:?}"
        );
        let notice = notice.expect("fallback notice");
        assert!(notice.contains("Claude") && notice.contains("review"), "{notice}");
    }
}
