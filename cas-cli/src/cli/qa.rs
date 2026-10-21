//! Project QA commands; registry checking uses the shared in-process validator.

use std::path::Path;

use anyhow::{Context, Result};
use clap::{ArgGroup, Subcommand};

use super::Cli;

#[derive(Subcommand)]
pub enum QaCommand {
    /// Map a diff to journeys and unowned CODEMAP areas
    Impact(super::qa_impact::ImpactArgs),
    /// Observe a release before announcing it or closing its epic
    PostRelease(super::qa_post_release::PostReleaseArgs),
    /// Seed, inspect or validate the journey registry
    #[command(group(ArgGroup::new("journey_action").required(true).multiple(false).args(["check", "seed", "status"])))]
    Journeys {
        /// Check schema, references and globs without executing specs
        #[arg(long)]
        check: bool,
        /// Bootstrap from tracked routes and project context
        #[arg(long)]
        seed: bool,
        /// Add new journeys while preserving curated entries
        #[arg(long, requires = "seed")]
        merge: bool,
        /// Show registry coverage and curation debt
        #[arg(long)]
        status: bool,
    },
}

pub fn execute(command: &QaCommand, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    match command {
        QaCommand::Impact(args) => super::qa_impact::execute(args, cli, cas_root),
        QaCommand::PostRelease(args) => super::qa_post_release::execute(args, cli, cas_root),
        QaCommand::Journeys {
            check: _,
            seed,
            merge,
            status,
        } => {
            let git = std::process::Command::new("git")
                .args(["rev-parse", "--show-toplevel"])
                .output()
                .context("locate project for journey registry")?;
            anyhow::ensure!(
                git.status.success(),
                "run cas qa journeys inside a Git checkout"
            );
            let project_root = std::path::PathBuf::from(String::from_utf8(git.stdout)?.trim_end());
            let local_cas = project_root.join(".cas");
            let config = crate::config::Config::load(cas_root.unwrap_or(&local_cas))?;
            let qa = config.qa();
            if *seed || *status {
                let result = if *seed {
                    crate::qa_seed::seed(&project_root, &qa, *merge)?
                } else {
                    crate::qa_seed::status(&project_root, &qa)?
                };
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                } else {
                    let verdict = if !result.present {
                        "MISSING"
                    } else if !result.valid {
                        "INVALID"
                    } else if result.debt_count > 0 || !result.unowned_surfaces.is_empty() {
                        "DEBT"
                    } else {
                        "READY"
                    };
                    println!(
                        "{verdict}: {} journey(s), {} added",
                        result.journeys, result.added
                    );
                    println!(
                        "Next: cas qa journeys --{}",
                        if result.present { "check" } else { "seed" }
                    );
                    println!(
                        "Seeded: {}  Unowned surfaces: {}",
                        result.seeded,
                        result.unowned_surfaces.len()
                    );
                    println!("Registry: {}", result.path);
                }
                return Ok(());
            }
            let registry_path = qa.registry.path;
            let report = crate::qa_registry::check(&project_root, Path::new(&registry_path))?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                let verdict = if report.problems.is_empty() {
                    "PASS"
                } else {
                    "FAIL"
                };
                println!(
                    "{verdict}: {} problem(s), {} debt item(s)",
                    report.problems.len(),
                    report.debt.len()
                );
                println!("Registry: {registry_path}");
                for (label, rows) in [("ERROR", &report.problems), ("DEBT", &report.debt)] {
                    for row in rows {
                        println!(
                            "{label} {} [{}]: {}",
                            row.subject.replace('\n', "\\n").replace('\r', "\\r"),
                            row.code,
                            row.message.replace('\n', "\\n").replace('\r', "\\r")
                        );
                    }
                }
            }
            anyhow::ensure!(
                report.problems.is_empty(),
                "fix the registry problems, then rerun: cas qa journeys --check"
            );
            Ok(())
        }
    }
}
