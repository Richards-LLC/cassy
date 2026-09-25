//! Bounded one-shot work routed through the factory light lane.

use std::fs::OpenOptions;
use std::io;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cas_mux::SupervisorCli;
use fs2::FileExt;

pub const DETACHED_WORKER_ARG: &str = "--internal-light-lane-worker";
const LOCK_WAIT: Duration = Duration::from_secs(900);
const JOB_TIMEOUT: Duration = Duration::from_secs(300);
const KILL_GRACE: Duration = Duration::from_secs(10);

pub(crate) fn default_model() -> String {
    cas_factory::resolve_lane("light", &cas_factory::CapabilitySnapshot::default())
        .ok()
        .and_then(|route| route.spec.model)
        .unwrap_or_else(|| "gpt-6-luna".to_string())
}

/// MCP tool prefix of the harness the light lane runs on, so a job body can
/// name tools the way that harness sees them (audit D12).
pub(crate) fn tool_prefix() -> io::Result<&'static str> {
    let route = cas_factory::resolve_lane("light", &cas_factory::CapabilitySnapshot::default())
        .map_err(|error| io::Error::other(format!("light lane unavailable: {error}")))?;
    Ok(route.spec.cli.backend().capabilities().tool_prefix)
}

fn command(prompt: &str, cwd: &Path) -> io::Result<Command> {
    let route = cas_factory::resolve_lane("light", &cas_factory::CapabilitySnapshot::default())
        .map_err(|error| io::Error::other(format!("light lane unavailable: {error}")))?;
    let model = route
        .spec
        .model
        .ok_or_else(|| io::Error::other("light lane has no model"))?;
    let effort = route
        .spec
        .effort
        .ok_or_else(|| io::Error::other("light lane has no effort"))?;
    let mut command = match route.spec.cli {
        SupervisorCli::Codex => {
            let mut command = Command::new("codex");
            // Maintenance jobs need unattended MCP writes. Codex's regular
            // `approval_policy=never` rejects those calls even in workspace-write.
            command.args([
                "exec",
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
            ]);
            command.arg("--model").arg(model);
            command
                .arg("-c")
                .arg(format!("model_reasoning_effort=\"{}\"", effort.as_str()));
            // Job bodies carry their own instructions; repository AGENTS.md
            // can otherwise turn a one-shot extraction into a factory worker.
            command.arg("-c").arg("project_doc_max_bytes=0");
            command.arg("-C").arg(cwd).arg("--").arg(prompt);
            command
        }
        SupervisorCli::Claude => {
            let mut command = Command::new("claude");
            command.args(["-p", "--model"]).arg(model);
            command.args([
                "--effort",
                effort.as_str(),
                "--dangerously-skip-permissions",
            ]);
            command.arg("--").arg(prompt);
            command.current_dir(cwd);
            command
        }
        other => {
            return Err(io::Error::other(format!(
                "unsupported light lane harness: {other:?}"
            )));
        }
    };
    command.stdin(Stdio::null());
    for key in [
        "CAS_AGENT_ROLE",
        "CAS_AGENT_NAME",
        "CAS_AGENT_ID",
        "CAS_SESSION_ID",
        "CAS_FACTORY_MODE",
        "CAS_FACTORY_SESSION",
        "CAS_FACTORY_NICE_WORKER",
        "CAS_FACTORY_WORKER_MODEL",
        "CAS_FACTORY_WORKER_EFFORT",
        "CAS_FACTORY_SUPERVISOR_CLI",
        "CAS_SUPERVISOR_NAME",
        "CAS_CLONE_PATH",
    ] {
        command.env_remove(key);
    }
    command.env("CAS_MAINTENANCE_JOB", "1");
    Ok(command)
}

pub(crate) fn spawn(prompt: &str, cwd: &Path, log: &Path) -> io::Result<Child> {
    let _ = command(prompt, cwd)?;
    let output = std::fs::File::create(log)?;
    let errors = output.try_clone()?;
    let lock = log
        .parent()
        .ok_or_else(|| io::Error::other("maintenance log has no directory"))?
        .join("lane.lock");
    let mut bounded = Command::new(std::env::current_exe()?);
    bounded
        .arg(DETACHED_WORKER_ARG)
        .arg(lock)
        .arg(cwd)
        .arg(prompt);
    bounded.current_dir(cwd);
    for key in [
        "CAS_AGENT_ROLE",
        "CAS_AGENT_NAME",
        "CAS_AGENT_ID",
        "CAS_SESSION_ID",
        "CAS_FACTORY_MODE",
        "CAS_FACTORY_SESSION",
        "CAS_FACTORY_NICE_WORKER",
        "CAS_FACTORY_WORKER_MODEL",
        "CAS_FACTORY_WORKER_EFFORT",
        "CAS_FACTORY_SUPERVISOR_CLI",
        "CAS_SUPERVISOR_NAME",
        "CAS_CLONE_PATH",
    ] {
        bounded.env_remove(key);
    }
    bounded.env("CAS_MAINTENANCE_JOB", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The Stop hook exits immediately; detach the bounded job from the
        // hook process group so harness cleanup cannot cancel it.
        unsafe {
            bounded.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    bounded
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(errors))
        .spawn()
}

/// Run the detached maintenance process without relying on GNU `flock` or
/// `timeout`, neither of which ships with macOS. The lock belongs to this
/// process and remains held until the harness exits or its deadline expires.
pub fn run_detached_worker(lock: &Path, cwd: &Path, prompt: &str) -> io::Result<()> {
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock)?;
    let lock_deadline = Instant::now() + LOCK_WAIT;
    loop {
        match lock_file.try_lock_exclusive() {
            Ok(()) => break,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= lock_deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "light-lane lock timed out",
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    }

    let mut harness = command(prompt, cwd)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            harness.pre_exec(|| {
                if libc::setpgid(0, 0) < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    let mut child = harness.spawn()?;
    let deadline = Instant::now() + JOB_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "light-lane harness exited with {status}"
                )))
            };
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            unsafe {
                libc::killpg(child.id() as i32, libc::SIGTERM);
            }
            #[cfg(not(unix))]
            let _ = child.kill();
            let grace_deadline = Instant::now() + KILL_GRACE;
            while Instant::now() < grace_deadline {
                if child.try_wait()?.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            #[cfg(unix)]
            unsafe {
                libc::killpg(child.id() as i32, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "light-lane harness timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn run(prompt: &str, cwd: &Path, timeout: Duration) -> io::Result<String> {
    let mut child = command(prompt, cwd)?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "light lane run timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let status = child.wait()?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| io::Error::other("light lane stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| io::Error::other("light lane stderr reader panicked"))??;
    if !status.success() {
        return Err(io::Error::other(format!(
            "light lane exited {}: {}",
            status,
            String::from_utf8_lossy(&stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}
