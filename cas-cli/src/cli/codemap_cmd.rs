use std::path::Path;

use clap::Subcommand;

use crate::hooks::handlers::handlers_events::codemap::{
    CodemapPending, evaluate_codemap_freshness,
};

use super::Cli;

#[derive(Subcommand)]
pub enum CodemapCommands {
    /// Show codemap staleness info (last updated, pending changes)
    Status,
    /// List specific pending structural changes
    Pending,
    /// Clear codemap-pending.json after manual update
    Clear,
}

const CODEMAP_PENDING_FILE: &str = "codemap-pending.json";

pub fn execute(cmd: &CodemapCommands, _cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    match cmd {
        CodemapCommands::Status => execute_status(cas_root),
        CodemapCommands::Pending => execute_pending(cas_root),
        CodemapCommands::Clear => execute_clear(cas_root),
    }
}

fn execute_status(cas_root: &Path) -> anyhow::Result<()> {
    let project_root = cas_root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from Cassy directory"))?;

    let codemap_path = project_root.join(".claude/CODEMAP.md");
    let pending_path = cas_root.join(CODEMAP_PENDING_FILE);

    // Check CODEMAP.md existence and last modified time
    if !codemap_path.exists() {
        println!("CODEMAP.md: not found");
        println!("  Run the /codemap skill to generate one.");
        return Ok(());
    }

    // Get last modified time from git or filesystem (display only)
    let last_updated = get_codemap_last_updated(&codemap_path);
    println!("CODEMAP.md: {}", codemap_path.display());
    println!("  Last updated: {last_updated}");

    // Authoritative freshness signal — same function used by the SessionStart hook.
    // This ensures `cas codemap status` and the hook can never disagree.
    if let Some(evaluation) = evaluate_codemap_freshness(cas_root) {
        println!("  Evaluated ref: {}", evaluation.evaluated_ref);
        match evaluation.staleness {
            None => println!("  Status: up to date"),
            Some(staleness) => {
                // Strip XML wrapper tags (with any attributes) for CLI display
                let injection = staleness.format_injection(false);
                let clean = injection
                    .lines()
                    .filter(|l| {
                        !l.starts_with("<codemap-freshness")
                            && !l.starts_with("</codemap-freshness")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                println!("  Status: stale");
                println!("  {clean}");
            }
        }
    }

    // Informational: show pending-ledger entries (not used for staleness decisions,
    // but useful for understanding what changed within the session).
    let pending_count = count_pending_changes(&pending_path);
    if pending_count > 0 {
        println!("  Pending ledger: {pending_count} in-session structural change(s) recorded");
        println!("  (ledger is informational — commit CODEMAP.md to reset the staleness signal)");
    }

    Ok(())
}

fn execute_pending(cas_root: &Path) -> anyhow::Result<()> {
    let project_root = cas_root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from Cassy directory"))?;

    let codemap_path = project_root.join(".claude/CODEMAP.md");
    let pending_path = cas_root.join(CODEMAP_PENDING_FILE);

    let mut has_changes = false;

    // Show pending file changes
    if pending_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&pending_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(pending) = serde_json::from_str::<CodemapPending>(line) {
                    for change in &pending.changes {
                        let prefix = match change.change_type.as_str() {
                            "A" => "  + ",
                            "D" => "  - ",
                            "R" => "  ~ ",
                            _ => "  ? ",
                        };
                        if let Some(old) = &change.old_path {
                            println!("{prefix}{old} → {}", change.path);
                        } else {
                            println!("{prefix}{}", change.path);
                        }
                        has_changes = true;
                    }
                }
            }
        }
    }

    // Show git-based structural changes
    if codemap_path.exists() {
        let git_changes = get_git_structural_change_list(project_root, &codemap_path);
        for line in &git_changes {
            let (status, path) = line.split_at(1);
            let path = path.trim();
            let prefix = match status {
                "A" => "  + ",
                "D" => "  - ",
                "R" => "  ~ ",
                _ => "  ? ",
            };
            println!("{prefix}{path} (git)");
            has_changes = true;
        }
    }

    if !has_changes {
        println!("No pending structural changes.");
    }

    Ok(())
}

fn execute_clear(cas_root: &Path) -> anyhow::Result<()> {
    let pending_path = cas_root.join(CODEMAP_PENDING_FILE);

    if pending_path.exists() {
        std::fs::remove_file(&pending_path)?;
        println!("Cleared {}", pending_path.display());
    } else {
        println!("No pending file to clear.");
    }

    Ok(())
}

/// Count total structural changes in the pending file.
fn count_pending_changes(pending_path: &Path) -> usize {
    if !pending_path.exists() {
        return 0;
    }
    let content = match std::fs::read_to_string(pending_path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<CodemapPending>(line.trim()).ok())
        .map(|p| p.changes.len())
        .sum()
}

/// Get last updated time for CODEMAP.md (from git log or file mtime).
fn get_codemap_last_updated(codemap_path: &Path) -> String {
    // Try git log first
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%ci", "--"])
        .arg(codemap_path)
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let date = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !date.is_empty() {
                return date;
            }
        }
    }

    // Fallback to file mtime
    if let Ok(metadata) = std::fs::metadata(codemap_path) {
        if let Ok(modified) = metadata.modified() {
            let dt: chrono::DateTime<chrono::Utc> = modified.into();
            return dt.format("%Y-%m-%d %H:%M:%S %z").to_string();
        }
    }

    "unknown".to_string()
}

// Note: get_git_structural_changes_since (the usize wrapper) was removed by cas-2de1 —
// execute_status() now calls evaluate_codemap_freshness() directly so both surfaces agree.
// get_git_structural_change_list is kept because execute_pending() uses it for display.

/// Get structural change lines from git since CODEMAP.md was last updated.
/// Used by execute_pending() to display what has changed — NOT used for
/// freshness decisions (evaluate_codemap_freshness() in codemap.rs owns that).
fn get_git_structural_change_list(project_root: &Path, codemap_path: &Path) -> Vec<String> {
    // Get CODEMAP.md's last commit timestamp
    let since = match get_codemap_git_timestamp(codemap_path) {
        Some(s) => s,
        None => return Vec::new(),
    };

    // Find structural changes (A/D/R) since that timestamp
    let output = std::process::Command::new("git")
        .current_dir(project_root)
        .args([
            "log",
            "--diff-filter=ADR",
            "--name-status",
            "--no-renames",
            "--format=",
            &format!("--since={since}"),
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|l| {
            let l = l.trim();
            !l.is_empty() && (l.starts_with('A') || l.starts_with('D') || l.starts_with('R'))
        })
        .map(|l| l.trim().to_string())
        .collect()
}

/// Get the ISO timestamp of CODEMAP.md's last commit (for display in execute_pending).
/// Returns None when CODEMAP.md has no git history (gitignored or never committed).
fn get_codemap_git_timestamp(codemap_path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%cI", "--"])
        .arg(codemap_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let ts = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if ts.is_empty() { None } else { Some(ts) }
}
