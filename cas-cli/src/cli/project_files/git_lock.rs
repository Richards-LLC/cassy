//! Diagnostic only: never remove a Git lock or infer ownership from age alone.
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::bounded_process::{Deadline, run_command};

// A recent lock may belong to a writer between closing and renaming the index.
const STALE_AFTER: Duration = Duration::from_secs(3600);
const PROBE_BUDGET: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum Writers {
    Found,
    NotObserved,
    Unknown,
}

pub(super) fn diagnose(root: &Path, stderr: &str) -> Option<String> {
    if !stderr.contains("index.lock") {
        return None;
    }
    let deadline = Deadline::after(PROBE_BUDGET);
    let mut git = Command::new("git");
    git.arg("-C").arg(root).args([
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "index.lock",
    ]);
    let output = run_command(&mut git, deadline, PROBE_BUDGET).ok()?;
    if !output.status.success() {
        return None;
    }
    let resolved = String::from_utf8(output.stdout).ok()?;
    let path = Path::new(resolved.trim());
    let age = std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok());
    let writers = if age.is_some_and(|age| age < STALE_AFTER) {
        // Recent is busy even if a process snapshot would miss its writer.
        Writers::Found
    } else {
        writers(path, deadline)
    };
    let display = path.strip_prefix(root).unwrap_or(path).display();
    Some(finding(&display.to_string(), age, writers))
}

fn writers(path: &Path, deadline: Deadline) -> Writers {
    // Conservative across repositories: even an unrelated Git process prevents
    // a stale verdict. `comm` avoids collecting arguments or credentials.
    let mut ps = Command::new("ps");
    ps.args(["-A", "-o", "comm="]);
    let processes = run_command(&mut ps, deadline, PROBE_BUDGET).ok();
    let quiet = processes.as_ref().is_some_and(|output| {
        output.status.success() && output.stderr.is_empty() && !output.stdout.is_empty()
    });
    if processes.as_ref().is_some_and(|output| {
        String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            Path::new(line.trim())
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "git" || name.starts_with("git-"))
        })
    }) {
        return Writers::Found;
    }
    let mut lsof = Command::new("lsof");
    lsof.args(["-nP", "-F", "p", "--"]).arg(path);
    let Ok(holders) = run_command(&mut lsof, deadline, PROBE_BUDGET) else {
        return Writers::Unknown;
    };
    if String::from_utf8_lossy(&holders.stdout)
        .lines()
        .any(|line| {
            line.strip_prefix('p')
                .is_some_and(|pid| pid.parse::<u32>().is_ok())
        })
    {
        return Writers::Found;
    }
    // lsof exits 1 for no matches. Warnings, permission errors, missing tools
    // and timeouts cannot support a claim that no writer was observed.
    if quiet
        && holders.status.code() == Some(1)
        && holders.stdout.is_empty()
        && holders.stderr.is_empty()
    {
        Writers::NotObserved
    } else {
        Writers::Unknown
    }
}

fn finding(path: &str, age: Option<Duration>, writers: Writers) -> String {
    let (state, remedy) = match (writers, age) {
        (Writers::Found, _) => ("busy", "wait for Git, then retry"),
        (Writers::NotObserved, Some(age)) if age >= STALE_AFTER => (
            "stale (no writer observed)",
            "recheck writers, remove only this lock, then retry",
        ),
        (Writers::NotObserved, Some(_)) => ("busy", "wait for Git, then retry"),
        _ => ("unknown ownership", "inspect Git writers before retrying"),
    };
    let age = age.map_or_else(
        || "unknown".into(),
        |age| {
            let seconds = age.as_secs();
            let (value, unit) = if seconds >= 86400 {
                (seconds / 86400, "d")
            } else if seconds >= 3600 {
                (seconds / 3600, "h")
            } else if seconds >= 60 {
                (seconds / 60, "m")
            } else {
                (seconds, "s")
            };
            format!("{value}{unit}")
        },
    );
    format!("git index lock {state}: {path} (age {age}); {remedy}")
}
