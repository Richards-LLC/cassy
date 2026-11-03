//! Resolve the native Claude installation before a legacy npm executable on PATH.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// The factory's Claude conformance baseline also meets the current model's
// minimum. Reject older clients before opening a pane that cannot take a turn.
const MIN_VERSION: [u64; 3] = [2, 1, 280];
pub const MIN_CLAUDE_VERSION: &str = "2.1.280";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, thiserror::Error)]
pub enum ClaudeBinaryError {
    #[error("Cannot launch Claude Code: no executable found. Install Claude Code and retry.")]
    Missing,
    #[error("{detail}")]
    Incompatible { detail: String },
    #[error("{detail}")]
    Unprobeable { detail: String },
}

/// Prefer the native installer's active link, then its newest installed version,
/// then PATH. Probe the executable itself, never trust a version filename.
/// Explicit inputs keep service and child environments consistent and testable.
pub fn resolve_claude_binary(home: &Path, paths: &[PathBuf]) -> Result<PathBuf, ClaudeBinaryError> {
    let mut candidates = vec![home.join(".local/bin/claude")];
    let mut versions: Vec<_> = std::fs::read_dir(home.join(".local/share/claude/versions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| Some((version(entry.file_name().to_str()?)?, entry.path())))
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.extend(versions.into_iter().map(|(_, path)| path));
    candidates.extend(
        paths
            .iter()
            .filter(|path| path.is_absolute())
            .map(|path| path.join("claude")),
    );
    let mut tried = std::collections::HashSet::new();
    let mut failures = Vec::new();
    let mut incompatible = false;
    for candidate in candidates {
        if !candidate.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if !candidate
                .metadata()
                .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
            {
                continue;
            }
        }
        let identity = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        if !tried.insert(identity) {
            continue;
        }
        match probe_version(&candidate, home, paths) {
            Ok(found) if found >= MIN_VERSION => return Ok(candidate),
            Ok(found) => {
                incompatible = true;
                failures.push(format!(
                    "{} is {}.{}.{}",
                    candidate.display(),
                    found[0],
                    found[1],
                    found[2]
                ));
            }
            Err(reason) => failures.push(format!("{}: {reason}", candidate.display())),
        }
    }
    if failures.is_empty() {
        return Err(ClaudeBinaryError::Missing);
    }
    let failures = failures.join("; ");
    let detail = format!(
        "Cannot launch Claude Code: version {MIN_CLAUDE_VERSION} or newer is required; {failures}. Update the native Claude installation and retry."
    );
    Err(if incompatible {
        ClaudeBinaryError::Incompatible { detail }
    } else {
        ClaudeBinaryError::Unprobeable { detail }
    })
}

pub(crate) fn current_claude_binary() -> Result<PathBuf, ClaudeBinaryError> {
    let home = dirs::home_dir().ok_or_else(|| ClaudeBinaryError::Unprobeable {
        detail: "Cannot launch Claude Code: HOME is unavailable".into(),
    })?;
    let paths: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    resolve_claude_binary(&home, &paths)
}

fn version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split_whitespace().next()?.split('.');
    let parsed = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(parsed)
}

fn probe_version(path: &Path, home: &Path, paths: &[PathBuf]) -> Result<[u64; 3], String> {
    // A file avoids pipe backpressure and a descendant retaining stdout. Read
    // only the small version response; kill and reap our child on a deadline.
    let mut output = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut child = Command::new(path)
        .arg("--version")
        .env("HOME", home)
        .env(
            "PATH",
            std::env::join_paths(paths).map_err(|error| error.to_string())?,
        )
        .stdin(Stdio::null())
        .stdout(output.try_clone().map_err(|error| error.to_string())?)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("--version failed: {error}"))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => error.to_string(),
                    _ => "--version timed out after two seconds".into(),
                });
            }
        }
    };
    if !status.success() {
        return Err(format!("--version exited with {status}"));
    }
    output
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut text = String::new();
    output
        .take(4096)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    version(&text).ok_or_else(|| "--version returned an unrecognized version".into())
}
