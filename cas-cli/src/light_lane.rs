//! Bounded one-shot work routed through the factory light lane.

use std::io;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cas_mux::SupervisorCli;

pub(crate) fn default_model() -> String {
    cas_factory::resolve_lane("light", &cas_factory::CapabilitySnapshot::default())
        .ok()
        .and_then(|route| route.spec.model)
        .unwrap_or_else(|| "gpt-6-luna".to_string())
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
    let output = std::fs::File::create(log)?;
    let errors = output.try_clone()?;
    let inner = command(prompt, cwd)?;
    let lock = log
        .parent()
        .ok_or_else(|| io::Error::other("maintenance log has no directory"))?
        .join("lane.lock");
    let mut bounded = Command::new("flock");
    bounded.args(["-w", "900"]);
    bounded.arg(lock);
    bounded.args(["timeout", "-k", "10s", "300s"]);
    bounded.arg(inner.get_program()).args(inner.get_args());
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
