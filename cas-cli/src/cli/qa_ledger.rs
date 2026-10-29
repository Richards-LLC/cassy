//! Thin CLI adapter; grammar and diagnostics live in qa_ledger.
use super::Cli;
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Args)]
pub struct LedgerArgs {
    #[command(subcommand)]
    command: LedgerCommand,
}
#[derive(Subcommand)]
enum LedgerCommand {
    /// Validate row grammar, counts and readable PASS captures
    Check {
        path: PathBuf,
        /// Apply this task's demo breadth policy
        #[arg(long)]
        task: Option<String>,
    },
    /// Create an honest ledger scaffold without overwriting evidence
    Init {
        #[arg(long)]
        task: String,
    },
}

pub(crate) fn check(path: &Path, task: Option<&cas_types::Task>) -> crate::qa_ledger::Report {
    crate::qa_ledger::check(path, task.map(crate::qa_ledger::minimum_rows).unwrap_or(1))
}

pub fn execute(args: &LedgerArgs, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    match &args.command {
        LedgerCommand::Check { path, task } => {
            let task = task
                .as_ref()
                .map(|id| -> Result<_> {
                    Ok(crate::store::open_task_store(
                        cas_root.context("--task requires a Cassy project")?,
                    )?
                    .get(id)?)
                })
                .transpose()?;
            let report = check(path, task.as_ref());
            if cli.json {
                let mut json = serde_json::to_value(&report)?;
                json["path"] = serde_json::json!(path);
                println!("{}", serde_json::to_string_pretty(&json)?);
            } else if report.ok {
                println!("Valid ledger: {} rows", report.rows.len());
            } else {
                println!("Invalid ledger: {} diagnostics", report.diagnostics.len());
                println!("Repair rows; rerun: cas qa ledger check <path> --task <id>");
                for d in &report.diagnostics {
                    let location = format!(
                        "line {}{} [{}]",
                        d.line,
                        d.row_id
                            .as_ref()
                            .map(|id| format!(" row {id}"))
                            .unwrap_or_default(),
                        d.field
                    );
                    println!("{}", crate::ui::widgets::truncate(&location, 80));
                    for value in [&d.message, &d.remedy] {
                        println!(
                            "  {}",
                            if cli.verbose {
                                value.to_owned()
                            } else {
                                crate::ui::widgets::truncate(value, 78)
                            }
                        );
                    }
                }
            }
            if !report.ok {
                let error = anyhow::anyhow!("ledger validation failed");
                return Err(if cli.json {
                    super::JsonReportedFailure(error).into()
                } else {
                    error
                });
            }
        }
        LedgerCommand::Init { task: id } => {
            let cas_root = cas_root.context("ledger init requires a Cassy project")?;
            let store = crate::store::open_task_store(cas_root)?;
            let task = store.get(id)?;
            let parent = store.get_parent_epic(id)?;
            use crate::mcp::tools::core::task::{lifecycle::close_ops, repo_context};
            let target = close_ops::effective_close_work_target(&task, parent.as_ref());
            let repo = match target {
                Some(target) => {
                    repo_context::resolve_repo_context(cas_root, &target)
                        .map_err(|e| anyhow::anyhow!(e))?
                        .repo_root
                }
                None => close_ops::resolve_close_gate_repo_root(cas_root)
                    .map_err(|e| anyhow::anyhow!(e))?,
            };
            let config = crate::config::Config::load(cas_root)?;
            let impact = crate::qa_impact::predicted(&repo, &config.qa(), &task)?;
            let journeys = impact
                .journeys
                .into_iter()
                .filter(|j| j.status == "live")
                .map(|j| (j.id, j.name))
                .collect::<Vec<_>>();
            let revision = crate::qa_impact::io::git(&repo, &["rev-parse", "HEAD"])?;
            let body = crate::qa_ledger::scaffold(&task, &journeys, revision.trim());
            let paths = crate::config::resolved_factory_artifact_paths(
                cas_root,
                config.factory().artifacts_root.as_deref(),
            );
            let dir = crate::config::project_factory_artifacts_root(cas_root, &paths.base).join(id);
            std::fs::create_dir_all(&dir)?;
            ensure!(
                crate::config::canonical_factory_task_artifact_dir(&paths.base, &dir).is_some(),
                "task artifact directory escapes its project namespace"
            );
            let path = dir.join("LEDGER.md");
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .with_context(|| {
                    format!(
                        "cannot create {}; existing evidence is preserved",
                        path.display()
                    )
                })?
                .write_all(body.as_bytes())?;
            if cli.json {
                let mut json = serde_json::to_value(check(&path, Some(&task)))?;
                json["path"] = serde_json::json!(path);
                json["task"] = serde_json::json!(id);
                println!("{}", serde_json::to_string_pretty(&json)?);
            } else {
                println!(
                    "Created ledger: {} rows",
                    crate::qa_ledger::parse(&body, 1).rows.len()
                );
                println!("Check: cas qa ledger check <path> --task {id}");
                println!(
                    "Path: {}",
                    crate::ui::widgets::truncate(&path.display().to_string(), 74)
                );
                println!("Use --json for the full path.");
            }
        }
    }
    Ok(())
}
