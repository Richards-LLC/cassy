//! Factory reviews start in the daemon, outside workers' credential-store userns.
use super::*;
use crate::ui::factory::{
    ClientMessage, DaemonMessage, FRAME_HEADER_SIZE, MAX_MESSAGE_SIZE, ProtocolCapability,
    decode_length, encode_message,
};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

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
                .ok_or_else(|| anyhow::anyhow!("verifier daemon response deadline"))?;
            stream.set_read_timeout(Some(remaining))?;
            match stream.read(bytes) {
                Ok(0) => anyhow::bail!("verifier daemon disconnected"),
                Ok(n) => bytes = &mut bytes[n..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
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

pub(super) fn request(session: &str, dispatch: &str) -> Result<(), String> {
    let request = || -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = UnixStream::connect(crate::ui::factory::gui_socket_path(session))?;
        match read_frame(&mut stream, deadline)? {
            DaemonMessage::Welcome {
                session_name,
                capabilities,
                ..
            } if session_name == session
                && capabilities.contains(&ProtocolCapability::VerifierLaunch) => {}
            _ => anyhow::bail!(
                "factory daemon lacks verifier launch support; update/relaunch the factory daemon"
            ),
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
                } if client_ref.as_deref() == Some(dispatch) => anyhow::bail!(message),
                _ => {}
            }
        }
    };
    request().map_err(|error| format!("verifier_factory_launch: {error:#}"))
}

/// This local control carries identity only, never executable arguments or a root.
pub(crate) fn launch(
    root: &Path,
    session: &str,
    dispatch_id: &str,
) -> Result<Option<std::thread::JoinHandle<()>>, String> {
    let dispatch =
        cas_store::get_verification_dispatch(root, dispatch_id).map_err(|e| e.to_string())?;
    let owner = crate::store::open_agent_store(root)
        .map_err(|e| e.to_string())?
        .get(&dispatch.owner_agent_id)
        .map_err(|e| e.to_string())?;
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
    core.resolve_live_supervisor_authority()
        .map_err(|_| "verifier_factory_owner: live registered supervisor required".to_string())?;
    let task = core
        .open_task_store()
        .map_err(|e| e.to_string())?
        .get(&dispatch.task_id)
        .map_err(|e| e.to_string())?;
    if task.is_terminal() {
        return Err("verifier_factory_task_terminal".into());
    }
    // Claim stays synchronous and exact; provider probes and the review run off-loop.
    super::automatic::launch_local(&core, &task, &dispatch)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

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
