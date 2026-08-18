use std::path::Path;

use clap::Subcommand;

use crate::hooks::handlers::handlers_events::project_overview::{
    check_freshness, clear_pending, ProjectOverviewPending, ProjectOverviewStaleness, DOC_PATH,
};

use super::Cli;

#[derive(Subcommand)]
pub enum ProjectOverviewCommands {
    /// Show PRODUCT_OVERVIEW.md staleness info (last updated, pending changes)
    Status,
    /// List specific pending relevant changes
    Pending,
    /// Clear project-overview-pending.json after running /project-overview
    Clear,
}

const PENDING_FILE: &str = "project-overview-pending.json";

pub fn execute(
    cmd: &ProjectOverviewCommands,
    _cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    match cmd {
        ProjectOverviewCommands::Status => execute_status(cas_root),
        ProjectOverviewCommands::Pending => execute_pending(cas_root),
        ProjectOverviewCommands::Clear => execute_clear(cas_root),
    }
}

fn project_root_from(cas_root: &Path) -> anyhow::Result<&Path> {
    cas_root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from Cassy directory"))
}

fn execute_status(cas_root: &Path) -> anyhow::Result<()> {
    let project_root = project_root_from(cas_root)?;
    let doc_path = project_root.join(DOC_PATH);

    if !doc_path.exists() {
        println!("PRODUCT_OVERVIEW.md: not found");
        println!("  Run the /project-overview skill to generate one.");
        return Ok(());
    }

    let last_updated = get_doc_last_updated(project_root, &doc_path);
    println!("PRODUCT_OVERVIEW.md: {}", doc_path.display());
    println!("  Last updated: {last_updated}");

    match check_freshness(project_root, None)? {
        None | Some(ProjectOverviewStaleness::Missing) => {
            // Missing is unreachable here — we already returned above for the
            // !doc_path.exists() case, and check_freshness only produces
            // Missing via the same predicate. Treat as up-to-date if we get
            // here (doc existed between both probes).
            println!("  Status: up to date");
        }
        Some(ref s @ ProjectOverviewStaleness::Stale { total_changes, .. })
        | Some(ref s @ ProjectOverviewStaleness::SignificantlyStale { total_changes, .. }) => {
            let label = match s {
                ProjectOverviewStaleness::SignificantlyStale { .. } => "significantly stale",
                _ => "stale",
            };
            println!("  Status: {label} ({total_changes} relevant change(s))");
            let injection = s.format_injection(false);
            let clean = injection
                .lines()
                .filter(|l| {
                    !l.starts_with("<project-overview-freshness")
                        && !l.starts_with("</project-overview-freshness")
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !clean.trim().is_empty() {
                println!("\n  Hook message: {}", clean.trim());
            }
        }
    }

    Ok(())
}

fn execute_pending(cas_root: &Path) -> anyhow::Result<()> {
    let project_root = project_root_from(cas_root)?;
    let pending_path = project_root.join(".cas").join(PENDING_FILE);

    let mut has_changes = false;
    if pending_path.exists() {
        let content = std::fs::read_to_string(&pending_path).unwrap_or_default();
        // Tolerant parse: skip malformed lines (the appender can crash
        // mid-write, and hook-side append races are a known class — we want
        // this CLI to print what it can rather than erroring on a single
        // bad entry).
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<ProjectOverviewPending>(line) else {
                continue;
            };
            for change in &entry.changes {
                let prefix = match change.change_type.as_str() {
                    "A" => "  + ",
                    "D" => "  - ",
                    "R" => "  ~ ",
                    "M" => "  * ",
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

    if !has_changes {
        println!("No pending relevant changes.");
    }

    Ok(())
}

fn execute_clear(cas_root: &Path) -> anyhow::Result<()> {
    let project_root = project_root_from(cas_root)?;
    let pending_path = project_root.join(".cas").join(PENDING_FILE);

    // clear_pending is idempotent (removes only if present); call it
    // unconditionally to avoid a TOCTOU window where a concurrent
    // PostToolUse hook appends between our exists() probe and the
    // remove_file call.
    let existed = pending_path.exists();
    clear_pending(project_root)?;
    if existed {
        println!("Cleared {}", pending_path.display());
    } else {
        println!("No pending file to clear.");
    }

    Ok(())
}

/// Last-updated string for PRODUCT_OVERVIEW.md (git commit time, else file mtime).
fn get_doc_last_updated(project_root: &Path, doc_path: &Path) -> String {
    // Pass DOC_PATH (relative) rather than the absolute doc_path — git log
    // matches pathspecs relative to the working tree, and an absolute path
    // outside the repo boundary can silently return empty output, forcing a
    // permanent mtime fallback.
    let output = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["log", "-1", "--format=%ci", "--", DOC_PATH])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let date = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !date.is_empty() {
                return date;
            }
        }
    }

    if let Ok(metadata) = std::fs::metadata(doc_path) {
        if let Ok(modified) = metadata.modified() {
            let dt: chrono::DateTime<chrono::Utc> = modified.into();
            return dt.format("%Y-%m-%d %H:%M:%S %z").to_string();
        }
    }

    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "po_cmd_{tag}_{}_{id}",
            std::process::id()
        ))
    }

    fn make_cas_root(project_root: &Path) -> std::path::PathBuf {
        let cas = project_root.join(".cas");
        std::fs::create_dir_all(&cas).unwrap();
        cas
    }

    #[test]
    fn status_prints_not_found_when_doc_missing() {
        let project = unique_tmp("status_missing");
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        let cas_root = make_cas_root(&project);

        // Must not error when doc is absent.
        execute_status(&cas_root).unwrap();

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn pending_prints_message_when_file_absent() {
        let project = unique_tmp("pending_empty");
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        let cas_root = make_cas_root(&project);

        execute_pending(&cas_root).unwrap();

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn pending_reads_jsonl_entries() {
        let project = unique_tmp("pending_jsonl");
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        let cas_root = make_cas_root(&project);

        let line = r#"{"changes":[{"type":"A","path":"watched/new.rs"},{"type":"M","path":"schema.prisma"}],"commit":"abc1234","recorded_at":"2026-04-14T00:00:00Z"}"#;
        std::fs::write(
            project.join(".cas/project-overview-pending.json"),
            format!("{line}\n"),
        )
        .unwrap();

        // Must not error and must parse the JSONL.
        execute_pending(&cas_root).unwrap();

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn clear_removes_pending_file() {
        let project = unique_tmp("clear_removes");
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        let cas_root = make_cas_root(&project);

        let pending = project.join(".cas/project-overview-pending.json");
        std::fs::write(&pending, "{}").unwrap();
        assert!(pending.exists());

        execute_clear(&cas_root).unwrap();
        assert!(!pending.exists());

        // Idempotent: second call on missing file is fine.
        execute_clear(&cas_root).unwrap();

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn status_prints_stale_when_pending_has_entries() {
        let project = unique_tmp("status_stale");
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(project.join("docs")).unwrap();
        let cas_root = make_cas_root(&project);
        // Create the doc so we get past the early "not found" return.
        std::fs::write(
            project.join("docs/PRODUCT_OVERVIEW.md"),
            "# overview\n",
        )
        .unwrap();
        // Seed 6 pending entries → SignificantlyStale (threshold is 5).
        let mut jsonl = String::new();
        for i in 0..6 {
            jsonl.push_str(&format!(
                r#"{{"changes":[{{"type":"A","path":"f{i}"}}],"commit":"c{i}","recorded_at":"2026-04-14T00:00:00Z"}}
"#
            ));
        }
        std::fs::write(project.join(".cas").join(PENDING_FILE), jsonl).unwrap();

        // Must not error and must flow through the stale branch.
        execute_status(&cas_root).unwrap();

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn pending_tolerates_malformed_lines() {
        let project = unique_tmp("pending_malformed");
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();
        let cas_root = make_cas_root(&project);

        // Valid entry followed by a partial/corrupt line (simulates the
        // hook-side append race leaving a truncated trailing write).
        let valid = r#"{"changes":[{"type":"A","path":"a.rs"}],"commit":"c","recorded_at":"2026-04-14T00:00:00Z"}"#;
        let corrupt = r#"{"changes":[{"type":"A","pat"#;
        std::fs::write(
            project.join(".cas").join(PENDING_FILE),
            format!("{valid}\n{corrupt}\n"),
        )
        .unwrap();

        // Must not error despite the malformed trailing line.
        execute_pending(&cas_root).unwrap();

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn project_root_from_rejects_root_cas() {
        // cas_root with no parent (shouldn't happen in practice, but the guard
        // exists — confirm it surfaces cleanly).
        let root = Path::new("/");
        // "/" has no parent — error path.
        assert!(project_root_from(root).is_err());
    }
}
