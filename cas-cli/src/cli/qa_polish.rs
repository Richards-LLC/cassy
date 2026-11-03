//! The release-prep polish sweep: non-blocking verifier findings for one epic.
//!
//! Findings stay in their verdicts rather than becoming tasks one at a time. At
//! release prep a supervisor reads them here, grouped by file, and may batch them
//! into one polish task or drop them.

use super::Cli;
use cas_types::{DependencyType, IssueSeverity, TaskStatus, Verification, VerificationStatus};
use anyhow::Result;
use clap::Args;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Args)]
pub struct PolishArgs {
    /// The epic whose closed tasks to read
    #[arg(long)]
    epic: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct Finding {
    task: String,
    line: Option<u32>,
    problem: String,
    suggestion: Option<String>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
struct Sweep {
    epic: String,
    verdicts: usize,
    findings: usize,
    /// Approved verdicts that mention improvements only in their summary text.
    unstructured_verdicts: Vec<String>,
    files: BTreeMap<String, Vec<Finding>>,
}

/// Each task's newest approved verdict, because an earlier cycle's findings may
/// already be fixed.
fn sweep(epic: &str, verdicts_by_task: Vec<Vec<Verification>>) -> Sweep {
    let mut sweep = Sweep { epic: epic.to_string(), ..Sweep::default() };
    for verdicts in verdicts_by_task {
        let Some(verdict) = verdicts
            .into_iter()
            .filter(|verdict| verdict.status == VerificationStatus::Approved)
            .max_by_key(|verdict| verdict.created_at)
        else {
            continue;
        };
        sweep.verdicts += 1;
        let warnings: Vec<_> = verdict
            .issues
            .iter()
            .filter(|issue| issue.severity == IssueSeverity::Warning)
            .collect();
        if warnings.is_empty() && verdict.summary.contains("Improvements") {
            sweep.unstructured_verdicts.push(verdict.task_id.clone());
        }
        for issue in warnings {
            sweep.findings += 1;
            sweep.files.entry(issue.file.clone()).or_default().push(Finding {
                task: verdict.task_id.clone(),
                line: issue.line,
                problem: issue.problem.clone(),
                suggestion: issue.suggestion.clone(),
            });
        }
    }
    sweep
}

fn render(sweep: &Sweep) -> String {
    let mut output = format!(
        "Polish sweep for {}: {} warnings in {} files, from {} approved verdicts\n",
        sweep.epic,
        sweep.findings,
        sweep.files.len(),
        sweep.verdicts
    );
    for (file, findings) in &sweep.files {
        output.push_str(&format!("\n{file} ({})\n", findings.len()));
        for finding in findings {
            let line = finding.line.map(|line| format!(":{line}")).unwrap_or_default();
            output.push_str(&format!("  {}{line}  {}\n", finding.task, finding.problem));
        }
    }
    if !sweep.unstructured_verdicts.is_empty() {
        output.push_str(&format!(
            "\nImprovements in summary text only, not listed above: {}\n",
            sweep.unstructured_verdicts.join(", ")
        ));
    }
    output
}

pub fn execute(args: &PolishArgs, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    let root = crate::store::detect::find_git_toplevel(&std::env::current_dir()?)?;
    let local_cas = root.join(".cas");
    let cas_root = cas_root.unwrap_or(&local_cas);
    let tasks = crate::store::open_task_store(cas_root)?;
    let verifications = crate::store::open_verification_store(cas_root)?;
    let mut verdicts = Vec::new();
    for dependency in tasks.get_dependents(&args.epic)? {
        if dependency.dep_type != DependencyType::ParentChild {
            continue;
        }
        if tasks.get(&dependency.from_id)?.status == TaskStatus::Closed {
            verdicts.push(verifications.get_for_task(&dependency.from_id)?);
        }
    }
    let sweep = sweep(&args.epic, verdicts);
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&sweep)?);
    } else {
        print!("{}", render(&sweep));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::VerificationIssue;

    fn verdict(task: &str, status: VerificationStatus, age: i64, issues: &[(&str, IssueSeverity)]) -> Verification {
        let mut verdict = Verification::new(format!("ver-{task}-{age}"), task.to_string());
        verdict.status = status;
        verdict.created_at = chrono::Utc::now() - chrono::Duration::hours(age);
        verdict.issues = issues
            .iter()
            .map(|(file, severity)| {
                let mut issue = VerificationIssue::new(file.to_string(), "polish".to_string(), format!("{task} {file}"));
                issue.severity = *severity;
                issue
            })
            .collect();
        verdict
    }

    /// Only warnings from each task's newest approved verdict, grouped by file.
    #[test]
    fn sweep_groups_warnings_from_the_newest_approved_verdicts() {
        use IssueSeverity::{Blocking, Warning};
        use VerificationStatus::{Approved, Rejected};
        let result = sweep(
            "cas-epic",
            vec![
                vec![
                    verdict("cas-a", Approved, 9, &[("old.rs", Warning)]),
                    verdict("cas-a", Approved, 1, &[("a.rs", Warning), ("a.rs", Warning), ("a.rs", Blocking)]),
                ],
                vec![verdict("cas-b", Approved, 1, &[("a.rs", Warning), ("b.rs", Warning)])],
                vec![verdict("cas-c", Rejected, 1, &[("rejected.rs", Warning)])],
            ],
        );
        assert_eq!((result.verdicts, result.findings), (2, 4));
        assert_eq!(result.files.keys().map(String::as_str).collect::<Vec<_>>(), ["a.rs", "b.rs"]);
        assert_eq!(result.files["a.rs"].len(), 3);
    }
}
