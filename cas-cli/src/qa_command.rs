//! The sandbox for project-owned QA executables (post-release probes and the
//! release-gate adapter).
//!
//! A command is a project-relative executable plus whitespace-separated
//! arguments, run without a shell from the project root. Its environment is a
//! minimal PATH and LANG plus only the allow-listed names. It has a timeout and
//! at most 1 MiB of stdout. Raw stdout and stderr never appear in an error,
//! because a broken probe may print credentials.
use anyhow::{Context, Result, ensure};
use std::io::{Seek, Write};
use std::path::Path;
use std::time::Duration;

/// Largest stdout a QA command may return.
pub const MAX_STDOUT: usize = 1024 * 1024;

/// One sandboxed run's limits.
#[derive(Debug, Clone, Copy)]
pub struct Sandbox<'a> {
    /// The project root: the working directory and the bound on the executable path.
    pub root: &'a Path,
    /// Environment variable names passed through from Cassy's environment.
    pub env: &'a [String],
    pub timeout: Duration,
}

/// Why a sandboxed run produced no stdout. Messages never include raw output.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("Command timed out after {0:?}")]
    TimedOut(Duration),
    #[error("{0}")]
    Failed(anyhow::Error),
}

/// Runs `command` (with `extra_args` appended) in the sandbox, writing `stdin`
/// to it when given, and returns its stdout after a zero exit.
pub fn run(
    sandbox: Sandbox<'_>,
    command: &str,
    extra_args: &[&str],
    stdin: Option<&[u8]>,
) -> Result<Vec<u8>, CommandError> {
    let mut process = prepare(sandbox, command, extra_args).map_err(CommandError::Failed)?;
    let input = match stdin {
        Some(bytes) => std::process::Stdio::from(
            input_file(bytes)
                .context("Could not stage command input")
                .map_err(CommandError::Failed)?,
        ),
        None => std::process::Stdio::null(),
    };
    let output = crate::bounded_process::run_command_with_stdin(
        &mut process,
        crate::bounded_process::Deadline::after(sandbox.timeout),
        sandbox.timeout,
        input,
    )
    .map_err(|error| match error {
        crate::bounded_process::BoundedCommandError::TimedOut => {
            CommandError::TimedOut(sandbox.timeout)
        }
        crate::bounded_process::BoundedCommandError::Io => {
            CommandError::Failed(anyhow::anyhow!("Command could not run"))
        }
    })?;
    if !output.status.success() {
        return Err(CommandError::Failed(anyhow::anyhow!(
            "Command exited {}",
            output.status
        )));
    }
    if output.stdout.len() > MAX_STDOUT {
        return Err(CommandError::Failed(anyhow::anyhow!(
            "Check output exceeds 1 MiB"
        )));
    }
    Ok(output.stdout)
}

fn prepare(
    sandbox: Sandbox<'_>,
    command: &str,
    extra_args: &[&str],
) -> Result<std::process::Command> {
    let mut words = command.split_whitespace();
    let program = words.next().context("Empty check command")?;
    ensure!(
        !Path::new(program).is_absolute(),
        "Check executable must be project-relative"
    );
    let path = sandbox
        .root
        .join(program)
        .canonicalize()
        .context("Check executable missing")?;
    ensure!(
        path.starts_with(sandbox.root.canonicalize()?),
        "Check executable must remain inside the project"
    );
    let mut process = std::process::Command::new(path);
    process
        .args(words)
        .args(extra_args)
        .current_dir(sandbox.root)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8");
    for name in sandbox.env {
        ensure!(
            !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'),
            "Invalid environment allow-list name"
        );
        if let Some(value) = std::env::var_os(name) {
            process.env(name, value);
        }
    }
    Ok(process)
}

/// Input goes through an unlinked file, not a pipe, so writing it cannot block
/// the bounded cleanup of a command that never reads stdin.
fn input_file(bytes: &[u8]) -> std::io::Result<std::fs::File> {
    let mut file = tempfile::tempfile()?;
    file.write_all(bytes)?;
    file.rewind()?;
    Ok(file)
}
