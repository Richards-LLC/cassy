//! Embedded assets, materialized into a private directory for one run.
//!
//! The binary holds the only copy a deliverable verb executes: installed
//! skill copies lag behind the binary, so they are never searched.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use anyhow::Context;

use super::UsageError;
use crate::bounded_process::{BoundedCommandError, Deadline, run_command};
use crate::builtins::BUILTIN_SKILLS;

/// One file to place at `path` inside a [`Workspace`].
#[derive(Clone, Copy, Debug)]
pub struct Asset {
    pub path: &'static str,
    pub content: &'static str,
}

impl Asset {
    /// A file from the builtin skill catalog (for example a skill's script),
    /// placed at `path`.
    pub fn builtin(catalog_path: &str, path: &'static str) -> anyhow::Result<Self> {
        BUILTIN_SKILLS
            .iter()
            .find(|file| file.path == catalog_path)
            .map(|file| Self {
                path,
                content: file.content,
            })
            .ok_or_else(|| anyhow::anyhow!("builtin deliverable asset missing: {catalog_path}"))
    }
}

/// A temporary directory holding materialized assets; removed on drop.
pub struct Workspace {
    dir: tempfile::TempDir,
}

impl Workspace {
    pub fn materialize(assets: &[Asset]) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir().context("could not create deliverable asset workspace")?;
        for asset in assets {
            let target = dir.path().join(asset.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, asset.content)
                .with_context(|| format!("could not write deliverable asset {}", asset.path))?;
        }
        Ok(Self { dir })
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    /// Run an embedded Python asset in isolated mode and parse its one JSON
    /// document. A missing interpreter, a timeout or a nonzero exit is a
    /// [`UsageError`] naming the script's last stderr line.
    pub fn python<I, S>(
        &self,
        script: &str,
        args: I,
        cwd: &Path,
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("python3");
        command
            .current_dir(cwd)
            .arg("-I")
            .arg(self.path(script))
            .args(args);
        let output = run(&mut command, timeout, "python3")?;
        let label = script.trim_end_matches(".py");
        if !output.status.success() {
            let line = last_line(&output);
            let line = line.strip_prefix(&format!("{label}: ")).unwrap_or(&line);
            return Err(UsageError(format!("{label}: {line}")).into());
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|error| UsageError(format!("{label}: unreadable output: {error}")).into())
    }
}

/// Run a command under a deadline; a missing program or a timeout is a usage error.
pub(crate) fn run(
    command: &mut Command,
    timeout: Duration,
    program: &str,
) -> Result<Output, UsageError> {
    run_command(command, Deadline::after(timeout), timeout).map_err(|error| {
        UsageError(match error {
            BoundedCommandError::TimedOut => format!("{program} timed out"),
            BoundedCommandError::Io => format!("{program} is unavailable or could not be executed"),
        })
    })
}

/// The last nonempty stderr line, else stdout's, without a `name: ` echo.
pub(crate) fn last_line(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    stderr
        .lines()
        .chain(stdout.lines())
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(500).collect())
        .unwrap_or_else(|| output.status.to_string())
}
