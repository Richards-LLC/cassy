//! Factory reviews start in the daemon, outside workers' credential-store userns.
use super::*;
use crate::ui::factory::{
    ClientMessage, DaemonMessage, FRAME_HEADER_SIZE, MAX_MESSAGE_SIZE, ProtocolCapability,
    decode_length, encode_message,
};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

/// How long one request waits for the daemon's answer.
const RESPONSE_DEADLINE: Duration = Duration::from_secs(5);

/// The daemon did not answer within [`RESPONSE_DEADLINE`] (cas-7caf). A timed
/// socket read reports this as EAGAIN ("os error 35" on macOS, 11 on Linux);
/// it means a busy daemon, which may still finish the launch, not a host limit.
#[derive(Debug)]
struct ResponseDeadline;

impl std::fmt::Display for ResponseDeadline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "verifier daemon response deadline: no answer within {} s", RESPONSE_DEADLINE.as_secs())
    }
}

impl std::error::Error for ResponseDeadline {}

fn read_frame(stream: &mut UnixStream, deadline: Instant) -> anyhow::Result<DaemonMessage> {
    fn read(
        stream: &mut UnixStream,
        mut bytes: &mut [u8],
        deadline: Instant,
    ) -> anyhow::Result<()> {
        while !bytes.is_empty() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero())
                .ok_or(ResponseDeadline)?;
            stream.set_read_timeout(Some(remaining))?;
            match stream.read(bytes) {
                Ok(0) => anyhow::bail!("verifier daemon disconnected"),
                Ok(n) => bytes = &mut bytes[n..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    return Err(ResponseDeadline.into());
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    let mut header = [0; FRAME_HEADER_SIZE];
    read(stream, &mut header, deadline)?;
    let size = decode_length(&header);
    anyhow::ensure!(
        size <= MAX_MESSAGE_SIZE,
        "verifier daemon frame exceeds protocol limit"
    );
    let mut body = vec![0; size];
    read(stream, &mut body, deadline)?;
    Ok(serde_json::from_slice(&body)?)
}

use super::automatic::{LaunchError, Retry, retry_for_code, store_refusal};

/// The daemon's answer, carried through the socket read as its own error type.
#[derive(Debug)]
struct Refused(String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// The daemon predates verifier launches: only an update changes that.
#[derive(Debug)]
struct Unsupported;

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("verifier_factory_unsupported: factory daemon lacks verifier launch support; update/relaunch the factory daemon")
    }
}

impl std::error::Error for Unsupported {}

pub(super) fn request(session: &str, dispatch: &str) -> Result<(), LaunchError> {
    let request = || -> anyhow::Result<()> {
        let deadline = Instant::now() + RESPONSE_DEADLINE;
        let mut stream = UnixStream::connect(crate::ui::factory::gui_socket_path(session))?;
        match read_frame(&mut stream, deadline)? {
            DaemonMessage::Welcome {
                session_name,
                capabilities,
                ..
            } if session_name == session
                && capabilities.contains(&ProtocolCapability::VerifierLaunch) => {}
            _ => return Err(Unsupported.into()),
        }
        stream.set_write_timeout(Some(deadline.saturating_duration_since(Instant::now())))?;
        stream.write_all(&encode_message(&ClientMessage::LaunchVerifier {
            dispatch_id: dispatch.into(),
        })?)?;
        loop {
            match read_frame(&mut stream, deadline)? {
                DaemonMessage::VerifierLaunched { dispatch_id } if dispatch_id == dispatch => {
                    return Ok(());
                }
                DaemonMessage::Error {
                    message,
                    client_ref,
                } if client_ref.as_deref() == Some(dispatch) => {
                    return Err(Refused(message).into());
                }
                _ => {}
            }
        }
    };
    request().map_err(|error| {
        let retry = if let Some(Refused(message)) = error.downcast_ref::<Refused>() {
            retry_for_code(message)
        } else if error.is::<Unsupported>() {
            Retry::Never
        } else if error.is::<ResponseDeadline>() {
            // A busy daemon: wait behind it without spending an attempt.
            Retry::Contended
        } else {
            Retry::Backoff
        };
        LaunchError { retry, message: format!("verifier_factory_launch: {error:#}") }
    })
}

/// The dispatch owner must be a live registered supervisor. cas-5d2e: a lookup
/// that failed, such as on a busy store connection, keeps its own cause and
/// code; only a caller who is not a live supervisor is reported as one.
/// cas-4fd6: a row still marked active whose process is gone is not live, and
/// a refusal names the agent and its status, never the whole row.
// gate[G-158 verifier-launch]: Verifier launch owner must be a live registered supervisor with a running process
fn owner_authority(core: &CasCore) -> Result<(), String> {
    use crate::mcp::daemon::{LivenessOutcome, evaluate_liveness, pid_alive, pid_matches_fingerprint};
    use crate::mcp::tools::core::task::lifecycle::close_ops::SupervisorAuthorityError;
    let owner = core.resolve_live_supervisor_authority().map_err(|error| match error {
        SupervisorAuthorityError::NotLive(agent) => format!(
            "verifier_factory_owner: live registered supervisor required (agent {} is {} with status {:?})",
            agent.id, agent.role, agent.status
        ),
        SupervisorAuthorityError::NotRegistered { caller_id, error } => format!(
            "verifier_factory_owner: live registered supervisor required (agent {caller_id} is not registered: {error})"
        ),
        SupervisorAuthorityError::StoreBusy(error) => format!("verifier_factory_store_busy: {error}"),
        SupervisorAuthorityError::Identity(error) | SupervisorAuthorityError::Store(error) => {
            format!("verifier_factory_owner_lookup: {error}")
        }
    })?;
    let local = owner
        .machine_id
        .as_deref()
        .is_none_or(|machine| machine == cas_types::Agent::get_or_generate_machine_id());
    if local
        && let LivenessOutcome::Dead { cc_pid, .. } =
            evaluate_liveness(&owner, pid_alive, pid_matches_fingerprint)
    {
        return Err(crate::gates::tag(
            "G-158",
            format!(
                "verifier_factory_owner: live registered supervisor required (agent {} process {cc_pid} is not running)",
                owner.id
            ),
        ));
    }
    Ok(())
}

/// This local control carries identity only, never executable arguments or a root.
pub(crate) fn launch(
    root: &Path,
    session: &str,
    dispatch_id: &str,
) -> Result<Option<std::thread::JoinHandle<()>>, String> {
    launch_with(root, session, dispatch_id, super::automatic::launch_local)
}

pub(super) fn launch_with(
    root: &Path,
    session: &str,
    dispatch_id: &str,
    start: impl FnOnce(&CasCore, &Task, &VerificationDispatch) -> Result<Option<std::thread::JoinHandle<()>>, String>,
) -> Result<Option<std::thread::JoinHandle<()>>, String> {
    let dispatch = super::automatic::launch_dispatch(root, dispatch_id, Some(session))?;
    let owner = crate::store::open_agent_store(root)
        .map_err(|e| e.to_string())?
        .get(&dispatch.owner_agent_id)
        .map_err(|e| store_refusal(&e))?;
    if owner.factory_session.as_deref() != Some(session)
        || cas_store::get_latest_verification_dispatch(root, &dispatch.task_id)
            .map_err(|e| e.to_string())?
            .as_ref()
            .map(|latest| latest.id.as_str())
            != Some(dispatch_id)
    {
        return Err(
            "verifier_factory_scope: dispatch does not belong to this session's current review"
                .into(),
        );
    }
    let daemon =
        crate::mcp::daemon::EmbeddedDaemon::new(crate::mcp::daemon::EmbeddedDaemonConfig {
            cas_root: root.to_path_buf(),
            ..Default::default()
        });
    let core = CasCore::with_daemon(root.to_path_buf(), None, Some(std::sync::Arc::new(daemon)));
    core.bind_agent_identity(owner.id, crate::mcp::AgentIdentitySource::ServerInternal)
        .map_err(|e| e.to_string())?;
    owner_authority(&core)?;
    let task = core
        .open_task_store()
        .map_err(|e| e.to_string())?
        .get(&dispatch.task_id)
        .map_err(|e| e.to_string())?;
    if task.is_terminal() {
        return Err("verifier_factory_task_terminal".into());
    }
    // Claim stays synchronous and exact; provider probes and the review run off-loop.
    start(&core, &task, &dispatch)
}

#[cfg(test)]
mod authority_tests {
    use super::*;

    /// cas-5d2e: verifier launches for cas-c389 and cas-5a04 said "live
    /// registered supervisor required" while the owner was live, because the
    /// authority lookup hit a busy shared store connection.
    #[test]
    fn busy_owner_lookup_is_a_retryable_store_refusal_cas_5d2e() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let mut owner = cas_types::Agent::new_with_role(
            "owner".into(),
            "supervisor".into(),
            cas_types::AgentRole::Supervisor,
        );
        owner.pid = Some(std::process::id());
        core.open_agent_store().unwrap().register(&owner).unwrap();
        core.bind_agent_identity(owner.id.clone(), crate::mcp::AgentIdentitySource::ServerInternal)
            .unwrap();
        owner_authority(&core).expect("a live registered supervisor owns the dispatch");

        let shared = cas_store::shared_db::shared_connection(&root.join("cas.db")).unwrap();
        let (held, holding) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _operation = cas_store::shared_db::connection_operation("slow_close_fixture");
            let site = line!() + 1;
            let _guard = cas_store::shared_db::begin_immediate_pooled(&shared).unwrap();
            held.send(site).unwrap();
            released.recv().unwrap();
        });
        let site = holding.recv_timeout(Duration::from_secs(5)).unwrap();
        let refused = {
            let _budget = cas_store::wait_budget::bound_waits_for(Duration::from_millis(20));
            owner_authority(&core).unwrap_err()
        };
        release.send(()).unwrap();
        holder.join().unwrap();
        assert!(refused.starts_with("verifier_factory_store_busy:"), "{refused}");
        assert!(refused.contains("holder_operation=slow_close_fixture"), "{refused}");
        let holder_site = refused.split_whitespace().find_map(|field| field.strip_prefix("holder_site="));
        assert_eq!(holder_site, Some(format!("{}:{}", file!(), site).as_str()));
    }

    /// cas-4fd6: a supervisor row still marked active whose process is gone
    /// is not a live owner; the status alone said it was.
    #[test]
    fn owner_whose_process_is_gone_cannot_launch_cas_4fd6() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let mut harness = Command::new("sleep").arg("30").spawn().unwrap();
        let mut owner = cas_types::Agent::new_with_role(
            "owner".into(),
            "supervisor".into(),
            cas_types::AgentRole::Supervisor,
        );
        owner.pid = Some(harness.id());
        crate::mcp::daemon::stamp_pid_fingerprint(&mut owner, harness.id());
        core.open_agent_store().unwrap().register(&owner).unwrap();
        core.bind_agent_identity(owner.id.clone(), crate::mcp::AgentIdentitySource::ServerInternal)
            .unwrap();
        owner_authority(&core).expect("an owner whose process runs is live");
        harness.kill().unwrap();
        harness.wait().unwrap();
        let refused = owner_authority(&core).unwrap_err();
        assert!(refused.starts_with("verifier_factory_owner:"), "{refused}");
    }

    /// cas-4fd6: the refusal named the whole agent row with Debug, putting its
    /// metadata and account paths in the attention notice.
    #[test]
    fn refused_owner_is_named_by_id_not_by_its_whole_row_cas_4fd6() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let mut owner = cas_types::Agent::new_with_role(
            "owner".into(),
            "supervisor".into(),
            cas_types::AgentRole::Supervisor,
        );
        owner.metadata.insert("account_dir".into(), "/accounts/unique-4fd6-secret".into());
        let agents = core.open_agent_store().unwrap();
        agents.register(&owner).unwrap();
        agents.graceful_shutdown(&owner.id).unwrap();
        core.bind_agent_identity(owner.id.clone(), crate::mcp::AgentIdentitySource::ServerInternal)
            .unwrap();
        let refused = owner_authority(&core).unwrap_err();
        assert!(refused.starts_with("verifier_factory_owner:"), "{refused}");
        assert!(refused.contains(&owner.id), "{refused}");
        assert!(!refused.contains("unique-4fd6-secret"), "{refused}");
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;

    #[test]
    fn retired_supervisor_dispatch_launches_for_claimed_successor_cas_32c2() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = env.home().join("owner-handoff");
        std::fs::create_dir_all(&project).unwrap();
        let root = crate::store::init_cas_dir(&project).unwrap();
        env.set("CAS_ROOT", &root);
        // No real provider/account is needed to prove the synchronous launch
        // and owner binding; provider preparation is asynchronous and isolated.
        env.set("CAS_TEST_HUB_PATH", env.home().join("no-provider"));
        let agents = cas_store::SqliteAgentStore::open(&root).unwrap();
        use cas_store::AgentStore;
        let mut previous = cas_types::Agent::new_with_role(
            "supervisor-a".into(), "previous".into(), cas_types::AgentRole::Supervisor,
        );
        previous.machine_id = Some(cas_types::Agent::get_or_generate_machine_id());
        previous.factory_session = Some("retired-factory".into());
        agents.register(&previous).unwrap();
        let mut task = Task::new("cas-32c2-launch".into(), "Owner handoff".into());
        task.status = cas_types::TaskStatus::AwaitingMerge;
        task.pending_verification = true;
        crate::store::open_task_store(&root).unwrap().add(&task).unwrap();
        let dispatch = cas_store::create_verification_dispatch_bound(
            &root, &task.id, &previous.id, &previous.id,
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() - chrono::Duration::minutes(1), false,
        ).unwrap();
        agents.unregister(&previous.id).unwrap();
        let mut successor = cas_types::Agent::new_with_role(
            "supervisor-b".into(), "successor".into(), cas_types::AgentRole::Supervisor,
        );
        successor.machine_id = previous.machine_id.clone();
        successor.factory_session = Some("successor-factory".into());
        agents.register(&successor).unwrap();
        let handoffs = agents.claim_supervisor_handoffs(&successor).unwrap();
        assert!(handoffs.iter().any(|handoff| handoff.agent_id == previous.id));
        assert!(agents.get(&previous.id).is_err());
        assert!(super::super::automatic::recovery_retry(&root, &dispatch).is_none());
        assert!(launch(&root, "unrelated-factory", &dispatch.id).is_err());
        assert_eq!(cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap().owner_agent_id, previous.id);
        let launched = launch(&root, "successor-factory", &dispatch.id)
            .expect("retired owner must resolve through its claimed handoff")
            .expect("an unresolved dispatch must launch once");
        launched.join().unwrap();
        let current = cas_store::get_verification_dispatch(&root, &dispatch.id).unwrap();
        assert_eq!(current.owner_agent_id, successor.id);
        assert_eq!(current.requester_agent_id, previous.id);
        assert_eq!(current.repository, dispatch.repository);
        assert!(current.deadline_at > chrono::Utc::now());
        assert!(super::super::automatic::recovery_retry(&root, &dispatch).is_some());
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        let metadata: String = conn.query_row(
            "SELECT metadata FROM events WHERE json_extract(metadata,'$.code') = 'verification_owner_adopted' AND json_extract(metadata,'$.dispatch_id') = ?1",
            [&dispatch.id], |row| row.get(0),
        ).unwrap();
        let audit: serde_json::Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!(audit["previous_owner"], previous.id);
        assert_eq!(audit["owner"], successor.id);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    fn user_namespaces_available() -> bool {
        Command::new("bwrap")
            .args(["--dev-bind", "/", "/", "--unshare-user", "--", "true"])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[test]
    fn factory_review_starts_outside_worker_userns_cas_343d() {
        const CANDIDATE_DATA: &[u8] = &[0, 1, 2, 127, 255];
        let role = std::env::var("CAS_TEST_BROKER_ROLE").unwrap_or_default();
        let exact = format!(
            "{}::factory_review_starts_outside_worker_userns_cas_343d",
            module_path!().split_once("::").unwrap().1
        );
        if role == "worker" {
            let root = PathBuf::from(std::env::var_os("CAS_TEST_BROKER_ROOT").unwrap());
            let daemon =
                crate::mcp::daemon::EmbeddedDaemon::new(crate::mcp::daemon::EmbeddedDaemonConfig {
                    cas_root: root.clone(),
                    ..Default::default()
                });
            let core = CasCore::with_daemon(root.clone(), None, Some(std::sync::Arc::new(daemon)));
            let task = core
                .open_task_store()
                .unwrap()
                .get("cas-broker343d")
                .unwrap();
            let dispatch = cas_store::get_latest_verification_dispatch(&root, &task.id)
                .unwrap()
                .unwrap();
            super::super::automatic::launch(&core, &task, &dispatch)
                .unwrap()
                .unwrap()
                .join()
                .unwrap();
            return;
        }
        if role == "host" {
            let root = PathBuf::from(std::env::var_os("CAS_TEST_BROKER_ROOT").unwrap());
            let session = std::env::var("CAS_TEST_BROKER_SESSION").unwrap();
            let socket = crate::ui::factory::gui_socket_path(&session);
            std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
            let listener = UnixListener::bind(socket).unwrap();
            // A completion pipe distinguishes a failed worker from a broker request;
            // no sleeps or assumed startup delay stand in for the actual event.
            let mut command = Command::new("bwrap");
            command
                .args(["--dev-bind", "/", "/", "--unshare-user", "--"])
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", &exact, "--nocapture"])
                .env("CAS_TEST_BROKER_ROLE", "worker");
            let (done, mut signal) = UnixStream::pair().unwrap();
            let output = std::sync::Arc::new(std::sync::Mutex::new(None));
            let captured = output.clone();
            let process = std::thread::spawn(move || {
                *captured.lock().unwrap() = Some(command.output().unwrap());
                signal.write_all(&[1]).unwrap();
            });
            let mut fds = [
                libc::pollfd {
                    fd: listener.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: done.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            assert!(unsafe { libc::poll(fds.as_mut_ptr(), 2, 10000) } > 0);
            assert_ne!(
                fds[0].revents & libc::POLLIN,
                0,
                "worker finished without requesting daemon launch: {:?}",
                output.lock().unwrap()
            );
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(
                    &encode_message(&DaemonMessage::Welcome {
                        session_name: session.clone(),
                        state: crate::ui::factory::SessionState {
                            focused_pane: None,
                            panes: vec![],
                            epic_id: None,
                            epic_title: None,
                            cols: 80,
                            rows: 24,
                        },
                        scrollback: None,
                        protocol_version: crate::ui::factory::PROTOCOL_VERSION,
                        capabilities: crate::ui::factory::local_gui_capabilities(),
                        pane_bootstrap: vec![],
                    })
                    .unwrap(),
                )
                .unwrap();
            let mut header = [0; FRAME_HEADER_SIZE];
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream.read_exact(&mut header).unwrap();
            let mut body = vec![0; decode_length(&header)];
            stream.read_exact(&mut body).unwrap();
            let ClientMessage::LaunchVerifier { dispatch_id } =
                serde_json::from_slice(&body).unwrap()
            else {
                panic!("wrong control")
            };
            assert!(launch(&root, "different-session", &dispatch_id).is_err());
            assert_eq!(
                cas_store::get_verification_dispatch(&root, &dispatch_id)
                    .unwrap()
                    .state,
                cas_types::VerificationDispatchState::Pending
            );
            let review = launch(&root, &session, &dispatch_id).unwrap().unwrap();
            stream
                .write_all(
                    &encode_message(&DaemonMessage::VerifierLaunched {
                        dispatch_id: dispatch_id.clone(),
                    })
                    .unwrap(),
                )
                .unwrap();
            review.join().unwrap();
            process.join().unwrap();
            let result = output.lock().unwrap().take().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let verdict = cas_store::get_verification_for_dispatch(&root, &dispatch_id)
                .unwrap()
                .unwrap();
            assert_eq!(verdict.status, VerificationStatus::Approved);
            assert_eq!(
                verdict.provenance,
                cas_types::VerificationProvenance::TaskVerifier
            );
            let marker = PathBuf::from(std::env::var_os("CAS_TEST_BROKER_MARKER").unwrap());
            assert_eq!(std::fs::read(&marker).unwrap(), CANDIDATE_DATA);
            assert_eq!(
                std::fs::read_to_string(marker.with_extension("namespace"))
                    .unwrap()
                    .trim(),
                std::fs::read_link("/proc/self/ns/user")
                    .unwrap()
                    .to_str()
                    .unwrap()
            );
            return;
        }
        // The worker role runs under bwrap's user namespace. Hosts that refuse
        // unprivileged namespaces (a factory worker session on soundwave) cannot
        // exercise it; lane CI and the merge queue still do.
        if !user_namespaces_available() {
            eprintln!("skipping {exact}: bwrap cannot create a user namespace on this host");
            return;
        }
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let home = env.home().to_path_buf();
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let root = crate::store::init_cas_dir(&project).unwrap();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let codex = bin.join("codex");
        std::fs::write(&codex, r#"#!/bin/sh
set -eu
if [ "$1" = --version ]; then echo 'codex-cli 9.9.9'; exit 0; fi
if [ "$1" = login ]; then [ -f "$CODEX_HOME/auth.json" ] || exit 1; echo 'Logged in using ChatGPT'; exit 0; fi
out=''
while [ "$#" -gt 0 ]; do
    if [ "$1" = --output-last-message ]; then shift; out=$1; fi
    shift
done
readlink /proc/self/ns/user > "${CAS_TEST_BROKER_MARKER%.*}.namespace"
bwrap --ro-bind / / --proc /proc --dev /dev -- /bin/sh -c 'cat "$1"; ! printf mutation >> "$1"' sh "$CAS_TEST_BROKER_INPUT" > "$CAS_TEST_BROKER_MARKER"
printf '%s' '{"status":"approved","summary":"Candidate read; write denied","confidence":0.95,"files_reviewed":"proof.data","issues":"[]"}' > "$out"
"#).unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(home.join(".codex-review")).unwrap();
        std::fs::write(home.join(".codex-review/auth.json"), "{}").unwrap();
        let session = format!("broker343d-{}", std::process::id());
        let mut owner = cas_types::Agent::new_with_role(
            "broker-owner".into(),
            "supervisor".into(),
            cas_types::AgentRole::Supervisor,
        );
        owner.factory_session = Some(session.clone());
        crate::store::open_agent_store(&root)
            .unwrap()
            .register(&owner)
            .unwrap();
        let mut task = Task::new("cas-broker343d".into(), "Read protected candidate".into());
        task.status = cas_types::TaskStatus::InProgress;
        task.pending_verification = true;
        crate::store::open_task_store(&root)
            .unwrap()
            .add(&task)
            .unwrap();
        cas_store::create_verification_dispatch_bound(
            &root,
            &task.id,
            &owner.id,
            &owner.id,
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(5),
            false,
        )
        .unwrap();
        let input = project.join("proof.data");
        std::fs::write(&input, CANDIDATE_DATA).unwrap();
        let marker = home.join("read.marker");
        let path = std::env::join_paths([bin.clone(), "/usr/bin".into(), "/bin".into()]).unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &exact, "--nocapture"])
            .env("PATH", path)
            .env_remove("CODEX_HOME")
            .env("CAS_TEST_HUB_PATH", bin)
            .env("CAS_TEST_BROKER_ROLE", "host")
            .env("CAS_TEST_BROKER_ROOT", &root)
            .env("CAS_TEST_BROKER_SESSION", session)
            .env("CAS_TEST_BROKER_INPUT", &input)
            .env("CAS_TEST_BROKER_MARKER", marker)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read(input).unwrap(), CANDIDATE_DATA);
        env.remove("CAS_TEST_BROKER_ROLE");
    }
}
