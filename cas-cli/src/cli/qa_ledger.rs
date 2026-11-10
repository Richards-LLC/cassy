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
        /// Require every intended receipt delivery in the covers header
        #[arg(long, value_delimiter = ',', value_parser = crate::qa_ledger::covered_task_id)]
        covers: Vec<String>,
    },
    /// Create an honest ledger scaffold without overwriting evidence
    Init {
        #[arg(long)]
        task: String,
        /// Delivery IDs covered by the batch receipt (defaults to --task)
        #[arg(long, value_delimiter = ',', value_parser = crate::qa_ledger::covered_task_id)]
        covers: Vec<String>,
        /// Release review: ReleaseScope JSON whose transformation and epic
        /// blocks this RELEASE_VERDICT ledger publishes
        #[arg(long)]
        release_scope: Option<PathBuf>,
    },
}

pub(crate) fn check(path: &Path, task: Option<&cas_types::Task>) -> crate::qa_ledger::Report {
    crate::qa_ledger::check(path, task.map(crate::qa_ledger::minimum_rows).unwrap_or(1))
}

pub fn execute(args: &LedgerArgs, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    match &args.command {
        LedgerCommand::Check { path, task, covers } => {
            let task = task
                .as_ref()
                .map(|id| -> Result<_> {
                    Ok(crate::store::open_task_store(
                        cas_root.context("--task requires a Cassy project")?,
                    )?
                    .get(id)?)
                })
                .transpose()?;
            let mut report = check(path, task.as_ref());
            report.require_coverage(covers);
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
        LedgerCommand::Init { task: id, covers, release_scope } => {
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
            let source = task
                .deliverables
                .factory_branch_start
                .as_ref()
                .map(|start| start.branch.as_str())
                .or(task.deliverables.parked_branch.as_deref());
            let revision = match source {
                Some(branch) => {
                    // A local source remains authoritative for ongoing work.
                    // After branch cleanup, use the same immutable delivery
                    // identity as batch receipts, never a movable tag or HEAD.
                    crate::qa_impact::io::resolve_ref(&repo, &format!("refs/heads/{branch}"))
                        .or_else(|error| crate::qa_batch::delivery_anchor(&repo, &task).ok_or(error))
                        .with_context(|| {
                            format!("resolve recorded task source branch {branch:?} for {id}")
                        })?
                }
                None if task.deliverables.factory_branch_anchor.is_some()
                    || !task.deliverables.historical_factory_branch_anchors.is_empty() =>
                {
                    crate::qa_batch::delivery_anchor(&repo, &task)
                        .with_context(|| format!("resolve recorded delivery anchor for {id}"))?
                }
                None => crate::qa_impact::io::resolve_ref(&repo, "HEAD")?,
            };
            let mut covers = if covers.is_empty() { vec![id.clone()] } else { covers.clone() };
            covers.sort_unstable();
            covers.dedup();
            let mut body = crate::qa_ledger::scaffold_with_coverage(&task, &journeys, revision.trim(), &covers);
            if let Some(scope) = release_scope {
                body.push_str("\n## Release review blocks\n\nRemove any epic whose acceptance this review did not verify.\n\n");
                body.push_str(&super::release_adapters::review_blocks_for_scope(cas_root, scope)?);
            }
            let paths = crate::config::resolved_factory_artifact_paths(
                cas_root,
                config.factory().artifacts_root.as_deref(),
            );
            let dir = paths.project_root.join(id);
            let mut components = Path::new(id).components();
            ensure!(
                matches!(components.next(), Some(std::path::Component::Normal(_)))
                    && components.next().is_none(),
                "task artifact directory escapes its project namespace"
            );
            // Validate existing ancestors before mkdir can follow a namespace alias.
            // Missing directories are safe to create; dangling links fail closed.
            for ancestor in [&paths.project_root, &dir] {
                match std::fs::symlink_metadata(ancestor) {
                    Ok(_) => ensure!(
                        crate::config::canonical_factory_task_artifact_dir(&paths.base, ancestor)
                            .is_some(),
                        "task artifact directory escapes its project namespace"
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            std::fs::create_dir_all(&dir)?;
            ensure!(
                crate::config::canonical_factory_task_artifact_dir(&paths.base, &dir).is_some(),
                "task artifact directory escapes its project namespace"
            );
            let path = dir.join("LEDGER.md");
            // GH #1758: a resumed task keeps its ledger. Init never rewrites
            // existing evidence; it reports the path, or refuses with the
            // diagnostics when that ledger does not validate for this task.
            let resumed = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(body.as_bytes())?;
                    false
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => true,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("cannot create {}; existing evidence is preserved", path.display())
                    });
                }
            };
            let mut report = check(&path, Some(&task));
            report.require_coverage(&covers);
            let (ok, rows, diagnostics) = (report.ok, report.rows.len(), report.diagnostics.len());
            if cli.json {
                let mut json = serde_json::to_value(report)?;
                json["path"] = serde_json::json!(path);
                json["task"] = serde_json::json!(id);
                json["resumed"] = serde_json::json!(resumed);
                println!("{}", serde_json::to_string_pretty(&json)?);
            } else {
                println!(
                    "{}",
                    if !resumed {
                        format!("Created ledger: {rows} rows")
                    } else if ok {
                        format!("Kept existing ledger: {rows} rows")
                    } else {
                        format!("Existing ledger kept but invalid: {diagnostics} diagnostics")
                    }
                );
                println!("Check: cas qa ledger check <path> --task {id} --covers <ids>");
                println!(
                    "Path: {}",
                    crate::ui::widgets::truncate(&path.display().to_string(), 74)
                );
                println!("Use --json for the full path.");
            }
            if resumed && !ok {
                let error = anyhow::anyhow!(
                    "existing ledger {} does not validate for {id}; nothing was overwritten. Repair it, then run cas qa ledger check",
                    path.display()
                );
                return Err(if cli.json { super::JsonReportedFailure(error).into() } else { error });
            }
        }
    }
    Ok(())
}
