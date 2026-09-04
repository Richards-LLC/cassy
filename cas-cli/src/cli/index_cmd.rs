//! `cas index` — build the local search indexes on demand.
//!
//! cas-499c: `cas index code` was advertised by the `code_search` MCP tool
//! (`agent_search_system/code.rs`) long before it existed, so every user who followed that
//! message hit "unrecognized subcommand". This registers it for real.
//!
//! The daemon indexes the same symbols in the background, but only while it is idle
//! (`mcp/daemon.rs`, gate deliberately retained), so this command is the manual catch-up lever
//! when the lag reported by `cas doctor` matters right now.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Args, Subcommand};

use crate::cli::Cli;
use crate::config::Config;
use crate::daemon::indexing::{
    collect_source_files, index_code_files_with, reconcile_code_tree, reconcile_code_vector_queue,
};

#[derive(Subcommand, Debug, Clone)]
pub enum IndexCommands {
    /// Index source files into the code symbol index (tree-sitter symbols + BM25)
    Code(IndexCodeArgs),
}

#[derive(Args, Debug, Clone)]
pub struct IndexCodeArgs {
    /// Directories or files to index (default: the project root)
    pub paths: Vec<PathBuf>,

    /// Re-parse files whose content hash is unchanged (use after losing the BM25 index)
    #[arg(long)]
    pub force: bool,

    /// Maximum number of files to index in this run (0 = no limit)
    #[arg(long, default_value_t = 0)]
    pub max_files: usize,
}

pub fn execute(cmd: &IndexCommands, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    match cmd {
        IndexCommands::Code(args) => execute_code(args, cli, cas_root),
    }
}

fn execute_code(args: &IndexCodeArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let project_root = cas_root
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let config = Config::load(cas_root).unwrap_or_default();
    let code_config = config.code();

    let roots: Vec<PathBuf> = if args.paths.is_empty() {
        vec![project_root.clone()]
    } else {
        args.paths.clone()
    };

    let mut files = collect_source_files(
        &roots,
        &code_config.extensions,
        &code_config.exclude_patterns,
    );
    files.sort();
    if args.max_files > 0 && files.len() > args.max_files {
        files.truncate(args.max_files);
    }

    if files.is_empty() && !args.paths.is_empty() {
        if cli.json {
            println!(
                "{}",
                serde_json::json!({
                    "files_scanned": 0,
                    "files_indexed": 0,
                    "symbols_indexed": 0,
                    "message": "no indexable source files found"
                })
            );
        } else {
            println!("No indexable source files found under the requested paths.");
        }
        return Ok(());
    }

    let started = Instant::now();
    let mut result = if args.paths.is_empty() {
        reconcile_code_tree(&files, &roots, cas_root, args.force)?
    } else {
        index_code_files_with(&files, cas_root, args.force)?
    };
    // Every run ends reconciled, including a path-scoped one: the queue rows
    // doctor complains about are not owned by any particular file, so scoping
    // the reconcile to the requested paths would leave the operator with the
    // same warning and the same command (cas-8a03).
    if result.vector_reconcile.is_none() {
        reconcile_code_vector_queue(cas_root, args.force, &mut result);
    }
    let elapsed = started.elapsed();

    // Post-run totals come from the store, not the run, so a no-op incremental pass still
    // reports the truth about what is searchable.
    let (total_files, total_symbols) = match crate::store::open_code_store(cas_root) {
        Ok(store) => (
            store.count_files().unwrap_or(0),
            store.count_symbols().unwrap_or(0),
        ),
        Err(_) => (0, 0),
    };

    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "files_scanned": files.len(),
                "files_indexed": result.files_indexed,
                "symbols_indexed": result.symbols_indexed,
                "total_files": total_files,
                "total_symbols": total_symbols,
                "elapsed_ms": elapsed.as_millis(),
                "errors": result.errors,
                "vector_reconcile": result.vector_reconcile.as_ref().map(|reconcile| {
                    serde_json::json!({
                        "orphaned_dropped": reconcile.orphaned_dropped,
                        "failed_rearmed": reconcile.failed_rearmed,
                        "failed_retained": reconcile.failed_retained,
                        "stale_rearmed": reconcile.stale_rearmed,
                        "requeued": reconcile.requeued,
                    })
                }),
            })
        );
    } else {
        println!(
            "Indexed {} file(s), {} symbol(s) in {:.1}s",
            result.files_indexed,
            result.symbols_indexed,
            elapsed.as_secs_f64()
        );
        println!("Index now holds {total_files} file(s) and {total_symbols} symbol(s).");
        if let Some(reconcile) = &result.vector_reconcile {
            println!("{}", reconcile_summary(reconcile));
        }
        if !result.errors.is_empty() {
            println!("{} file(s) could not be indexed:", result.errors.len());
            for error in result.errors.iter().take(3) {
                println!("  - {error}");
            }
        }
    }

    Ok(())
}

/// One line stating what the closing queue reconcile changed.
///
/// The counts are printed even when they are all zero (as "already
/// consistent"), because the failure this command is fixing was a run that
/// silently changed nothing while doctor kept naming it as the remedy
/// (cas-8a03). A retained residual names its own remediation rather than
/// leaving the operator to re-run the same command forever.
fn reconcile_summary(reconcile: &cas_store::CodeVectorReconcile) -> String {
    if reconcile.is_noop() && reconcile.failed_retained == 0 {
        return "Code-vector queue already consistent: nothing to reconcile.".to_string();
    }
    let mut line = format!(
        "Reconciled code-vector queue: {} orphaned row(s) dropped, {} failed re-armed, \
         {} stale row(s) re-armed, {} symbol(s) re-queued.",
        reconcile.orphaned_dropped,
        reconcile.failed_rearmed,
        reconcile.stale_rearmed,
        reconcile.requeued,
    );
    if reconcile.failed_retained > 0 {
        line.push_str(&format!(
            " {} failed row(s) left alone — their recorded error names input the provider \
             rejects every time; `cas index code --force` re-arms them anyway.",
            reconcile.failed_retained
        ));
    }
    line
}

/// Walk `roots` and collect files whose extension is indexable.
///
/// `.gitignore` is honoured (via the `ignore` walker) so `target/`, `node_modules/` and friends
/// cost nothing, and the configured exclude globs are applied on top for trees that ignore
/// nothing.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_source_files_filters_by_extension_and_excludes() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("src/notes.md"), "# not code").unwrap();
        std::fs::write(root.join("target/debug/build.rs"), "fn main() {}").unwrap();

        let files = collect_source_files(
            &[root.to_path_buf()],
            &["rs".to_string()],
            &["target/**".to_string()],
        );

        assert_eq!(files.len(), 1, "unexpected files: {files:?}");
        assert!(files[0].ends_with("src/main.rs"));
    }

    #[test]
    fn collect_source_files_accepts_explicit_file_paths() {
        let temp = tempfile::TempDir::new().unwrap();
        let file = temp.path().join("lib.rs");
        std::fs::write(&file, "pub fn hello() {}").unwrap();

        let files = collect_source_files(&[file.clone()], &["rs".to_string()], &[]);
        assert_eq!(files, vec![file]);
    }

    #[test]
    fn collect_source_files_excludes_configured_but_unsupported_extensions() {
        let temp = tempfile::TempDir::new().unwrap();
        let supported = temp.path().join("lib.rs");
        let unsupported = temp.path().join("App.swift");
        std::fs::write(&supported, "pub fn supported() {}").unwrap();
        std::fs::write(&unsupported, "func unsupported() {}").unwrap();

        let files = collect_source_files(
            &[temp.path().to_path_buf()],
            &["rs".to_string(), "swift".to_string()],
            &[],
        );

        assert_eq!(files, vec![supported]);
    }
}
