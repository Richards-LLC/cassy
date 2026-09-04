//! Status command

use std::path::Path;

use clap::Parser;

use crate::config::Config;
use crate::store::{open_code_store, open_rule_store, open_store};
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;
use cas_core::Syncer;

use crate::cli::Cli;

#[derive(Parser)]
pub struct StatusArgs {
    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,
}

pub fn execute(args: &StatusArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let store = open_store(cas_root)?;
    let rule_store = open_rule_store(cas_root)?;
    let config = Config::load(cas_root)?;

    let entries = store.list()?;
    let archived = store.list_archived()?;
    let rules = rule_store.list()?;

    // Get code stats (optional - may not have indexed code)
    let (code_files, code_symbols) = if let Ok(code_store) = open_code_store(cas_root) {
        let symbols = code_store
            .search_symbols("%", None, None, 100000)
            .unwrap_or_default();
        let files = code_store.count_files().unwrap_or(0);
        (files, symbols.len())
    } else {
        (0, 0)
    };
    let project_root = cas_root.parent().unwrap_or(std::path::Path::new("."));
    let (repo_root, repository) = crate::daemon::indexing::resolve_repository(project_root);
    let vector_store = cas_store::SqliteCodeVectorStore::open(cas_root).ok();
    // Same coverage source as `cas doctor` (cas-73e7): queue-row counts made
    // `cas status --json` and the doctor line disagree with each other, and both
    // disagree with the symbols that actually lack vectors.
    let code_vectors = vector_store
        .as_ref()
        .and_then(|store| store.coverage().ok())
        .unwrap_or_default();
    let code_scan = vector_store
        .as_ref()
        .and_then(|store| store.index_state(&repository).ok().flatten());
    let current_head = repo_root
        .as_deref()
        .and_then(crate::daemon::indexing::head_commit);
    let head_lag = code_scan.as_ref().and_then(|scan| {
        current_head
            .as_ref()
            .zip(scan.last_head.as_ref())
            .map(|(current, indexed)| current != indexed)
    });
    let eligible_files = code_scan
        .as_ref()
        .map(|scan| scan.eligible_files)
        .unwrap_or(0);
    let indexed_files = code_scan
        .as_ref()
        .map(|scan| scan.indexed_files)
        .unwrap_or(0);
    let failed_files = code_scan
        .as_ref()
        .map(|scan| scan.failed_files)
        .unwrap_or(0);
    let file_lag = eligible_files.saturating_sub(indexed_files);

    let total_entries = entries.len();
    let total_archived = archived.len();
    let total_rules = rules.len();

    // Count high-value entries
    let high_value = entries.iter().filter(|e| e.feedback_score() > 0).count();

    // Count proven rules
    let syncer = Syncer::new(
        project_root.join(&config.sync.target),
        config.sync.min_helpful,
    );
    let proven_rules = rules.iter().filter(|r| syncer.is_proven(r)).count();

    if cli.json {
        let status = serde_json::json!({
            "entries": total_entries,
            "archived": total_archived,
            "high_value": high_value,
            "rules": total_rules,
            "proven_rules": proven_rules,
            "code_files": code_files,
            "code_symbols": code_symbols,
            "code_index": {
                "repository": repository,
                "eligible_files": eligible_files,
                "indexed_files": indexed_files,
                "failed_files": failed_files,
                "file_lag": file_lag,
                "last_head": code_scan.as_ref().and_then(|scan| scan.last_head.clone()),
                "head_lag": head_lag,
                "last_scan_at": code_scan.as_ref().map(|scan| scan.last_scan_at.clone()),
                "last_error": code_scan.as_ref().and_then(|scan| scan.last_error.clone()),
                "vector_eligible": code_vectors.eligible,
                "vectorized": code_vectors.vectorized,
                "pending": code_vectors.pending,
                "failed": code_vectors.failed,
                "unqueued": code_vectors.unqueued,
                "orphaned_queue_rows": code_vectors.orphaned,
            },
            "sync_enabled": config.sync.enabled && !Config::is_sync_disabled()
        });
        println!("{}", serde_json::to_string(&status)?);
    } else if args.verbose || cli.verbose {
        let theme = ActiveTheme::default();
        let mut out = std::io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);

        fmt.subheading("cas status")?;
        fmt.write_muted(&"─".repeat(40))?;
        fmt.newline()?;
        fmt.field(
            "  Entries",
            &format!("{total_entries} ({total_archived} archived)"),
        )?;
        fmt.field("  High-value", &format!("{high_value} (positive feedback)"))?;
        fmt.field("  Rules", &format!("{total_rules} ({proven_rules} proven)"))?;
        if code_files > 0 || code_symbols > 0 {
            fmt.field(
                "  Code",
                &format!("{code_files} files, {code_symbols} symbols"),
            )?;
            fmt.field(
                "  Code coverage",
                &format!(
                    "{indexed_files}/{eligible_files} files, {file_lag} lagging, {failed_files} failed, HEAD {}",
                    match head_lag {
                        Some(true) => "behind",
                        Some(false) => "current",
                        None => "unknown",
                    }
                ),
            )?;
            fmt.field(
                "  Code vectors",
                &format!(
                    "{}/{} vectorized, {} pending, {} failed{}",
                    code_vectors.vectorized,
                    code_vectors.eligible,
                    code_vectors.pending,
                    code_vectors.failed,
                    // Pending that nothing has asked for is a different fact
                    // from pending that is queued and waiting its turn.
                    if code_vectors.unqueued > 0 {
                        format!(" ({} never queued)", code_vectors.unqueued)
                    } else {
                        String::new()
                    }
                ),
            )?;
        }
        fmt.newline()?;
        fmt.subheading("Configuration")?;
        fmt.write_muted(&"─".repeat(40))?;
        fmt.newline()?;
        fmt.field("  Sync enabled", &config.sync.enabled.to_string())?;
        fmt.field("  Sync target", &config.sync.target)?;
        fmt.field("  Min helpful", &config.sync.min_helpful.to_string())?;

        if Config::is_sync_disabled() {
            fmt.newline()?;
            fmt.warning("Sync disabled via environment")?;
        }

        // Show recent entries
        if !entries.is_empty() {
            fmt.newline()?;
            fmt.subheading("Recent entries")?;
            fmt.write_muted(&"─".repeat(40))?;
            fmt.newline()?;
            for entry in entries.iter().take(5) {
                let score = entry.feedback_score();
                let score_str = if score > 0 {
                    format!("+{score}")
                } else {
                    score.to_string()
                };
                let line = format!("{} [{}] {}", entry.id, score_str, entry.preview(40));
                if score > 0 {
                    let color = fmt.theme().palette.status_success;
                    fmt.write_colored(&format!("  {line}"), color)?;
                } else if score < 0 {
                    let color = fmt.theme().palette.status_error;
                    fmt.write_colored(&format!("  {line}"), color)?;
                } else {
                    fmt.write_raw(&format!("  {line}"))?;
                }
                fmt.newline()?;
            }
        }
    } else {
        // One-line summary
        let code_part = if code_symbols > 0 {
            format!(", {code_symbols} code symbols")
        } else {
            String::new()
        };
        println!(
            "cas: {total_entries} entries, {total_rules} rules ({proven_rules} proven), {high_value} high-value{code_part}"
        );
    }

    Ok(())
}
