//! Small, credential-agnostic process deadline used by read-only probes.
//!
//! Output is captured in mode-0600 files so a descendant cannot keep a pipe
//! open after its parent exits. On Unix each probe owns a process group, which
//! lets timeout cleanup reap descendants without touching the caller.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static CAPTURE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadline {
    expires_at: Instant,
}

impl Deadline {
    pub(crate) fn after(duration: Duration) -> Self {
        Self {
            expires_at: Instant::now() + duration,
        }
    }

    pub(crate) fn remaining(self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }
}

#[derive(Debug)]
pub(crate) enum BoundedCommandError {
    TimedOut,
    Io,
}

pub(crate) fn run_command(
    command: &mut Command,
    deadline: Deadline,
    per_command_cap: Duration,
) -> Result<Output, BoundedCommandError> {
    let remaining = deadline.remaining();
    if remaining.is_zero() {
        return Err(BoundedCommandError::TimedOut);
    }
    let timeout = remaining.min(per_command_cap);
    let stdout = Capture::new().ok_or(BoundedCommandError::Io)?;
    let stderr = Capture::new().ok_or(BoundedCommandError::Io)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            stdout.writer().map_err(|_| BoundedCommandError::Io)?,
        ))
        .stderr(Stdio::from(
            stderr.writer().map_err(|_| BoundedCommandError::Io)?,
        ));
    configure_process_group(command);
    let mut child = command.spawn().map_err(|_| BoundedCommandError::Io)?;
    let probe_deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < probe_deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(None) => {
                terminate_process_group(&mut child);
                return Err(BoundedCommandError::TimedOut);
            }
            Err(_) => {
                terminate_process_group(&mut child);
                return Err(BoundedCommandError::Io);
            }
        }
    };
    Ok(Output {
        status,
        stdout: stdout.read().map_err(|_| BoundedCommandError::Io)?,
        stderr: stderr.read().map_err(|_| BoundedCommandError::Io)?,
    })
}

#[cfg(unix)]
pub(crate) fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setpgid is async-signal-safe and the closure touches no shared
    // Rust state between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
}

#[cfg(not(unix))]
pub(crate) fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
pub(crate) fn terminate_process_group(child: &mut std::process::Child) {
    // A negative pid targets only the process group created above.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
}

#[cfg(not(unix))]
pub(crate) fn terminate_process_group(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

struct Capture {
    path: PathBuf,
}

impl Capture {
    fn new() -> Option<Self> {
        for _ in 0..8 {
            let sequence = CAPTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                ".cas-bounded-process-{}-{sequence}",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            if options.open(&path).is_ok() {
                return Some(Self { path });
            }
        }
        None
    }

    fn writer(&self) -> io::Result<File> {
        OpenOptions::new().append(true).open(&self.path)
    }

    fn read(&self) -> io::Result<Vec<u8>> {
        std::fs::read(&self.path)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deadline long enough that runner load cannot decide the outcome.
    ///
    /// Tests that assert *what* a bounded command returns must not also race
    /// the clock: on a loaded CI runner a one-second budget is a coin toss, not
    /// a correctness statement. Only the tests that assert timeout behaviour
    /// itself keep a short deadline, and there the load pushes them toward the
    /// expected result rather than away from it.
    const GENEROUS: Duration = Duration::from_secs(30);

    /// Echo a token through the platform shell, with no toolchain dependency.
    #[cfg(unix)]
    fn echo_command() -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", "echo bounded-process-ok"]);
        command
    }

    #[cfg(not(unix))]
    fn echo_command() -> Command {
        let mut command = Command::new("cmd");
        command.args(["/C", "echo bounded-process-ok"]);
        command
    }

    #[cfg(unix)]
    #[test]
    fn timeout_reaps_descendants_without_waiting_for_inherited_output() {
        let started = Instant::now();
        let result = run_command(
            Command::new("sh").args(["-c", "(sleep 30)& wait"]),
            Deadline::after(Duration::from_millis(75)),
            Duration::from_millis(75),
        );
        assert!(matches!(result, Err(BoundedCommandError::TimedOut)));
        // The point is that we did not wait for the orphaned child, so the
        // bound only has to sit clearly below its sleep. Five seconds proves
        // that while leaving ~66x the deadline as headroom for a loaded runner;
        // the previous one-second bound was the same tight-margin shape that
        // made this file flaky (cas-2b7e, commit 2c6a4bc6).
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "returned after {:?}, which no longer proves the inherited sleep was skipped",
            started.elapsed()
        );
    }

    #[test]
    fn captures_success_output() {
        // Deliberately not `rustc --version`: an external toolchain command
        // under a one-second deadline lost the race on a loaded ubuntu-latest
        // runner (CI 32140529939). Nothing here is about the toolchain, so the
        // child is now a cheap deterministic echo with a generous budget.
        let output = run_command(&mut echo_command(), Deadline::after(GENEROUS), GENEROUS).unwrap();

        assert!(output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("bounded-process-ok"),
            "stdout was {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn captures_failure_status_and_stderr_without_racing_the_clock() {
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "echo bounded-process-bad >&2; exit 3"]);
            command
        };
        #[cfg(not(unix))]
        let mut command = {
            let mut command = Command::new("cmd");
            command.args(["/C", "echo bounded-process-bad 1>&2 & exit 3"]);
            command
        };

        let output = run_command(&mut command, Deadline::after(GENEROUS), GENEROUS).unwrap();

        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(3));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("bounded-process-bad"),
            "stderr was {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
