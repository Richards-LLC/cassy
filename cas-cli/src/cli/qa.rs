//! Project QA commands; registry checking uses the shared in-process validator.

use std::path::Path;

use anyhow::{Context, Result};
use clap::Subcommand;

use super::Cli;

#[derive(Subcommand)]
pub enum QaCommand {
    /// Map a diff to journeys and unowned CODEMAP areas
    Impact(super::qa_impact::ImpactArgs),
    /// Validate the journey registry against tracked files
    Journeys {
        /// Check schema, references and globs without executing specs
        #[arg(long, required = true)]
        check: bool,
    },
}

pub fn execute(command: &QaCommand, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    match command {
        QaCommand::Impact(args) => super::qa_impact::execute(args, cli, cas_root),
        QaCommand::Journeys { check: _ } => {
            let git = std::process::Command::new("git")
                .args(["rev-parse", "--show-toplevel"])
                .output()
                .context("locate project for journey registry")?;
            anyhow::ensure!(
                git.status.success(),
                "run cas qa journeys --check inside a Git checkout"
            );
            let project_root = std::path::PathBuf::from(String::from_utf8(git.stdout)?.trim_end());
            let local_cas = project_root.join(".cas");
            let config = crate::config::Config::load(cas_root.unwrap_or(&local_cas))?;
            let registry_path = config.qa().registry.path;
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
