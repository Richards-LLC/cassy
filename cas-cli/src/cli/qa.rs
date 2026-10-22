//! Project QA commands; registry checking uses the shared in-process validator.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, ensure};
use clap::{ArgGroup, Args, Subcommand};
use rusqlite::OptionalExtension;
use serde::Serialize;

use super::Cli;
use crate::hooks::handlers::handlers_events::docs_freshness::{Measurement, UpkeepReport};

pub(crate) fn measured_ref_line(reference: &str) -> String {
    let prefix = "Measured ref: ";
    format!(
        "{prefix}{}",
        crate::ui::widgets::truncate(reference, 80 - prefix.len())
    )
}

#[derive(Serialize)]
struct DebtSummary {
    total: usize,
    by_kind: BTreeMap<String, usize>,
    previous_total: Option<usize>,
    delta: Option<i64>,
    grew: Option<bool>,
    history_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    history_error: Option<String>,
}

impl DebtSummary {
    fn record(
        report: &crate::qa_registry::ValidationReport,
        project: &Path,
        registry: &Path,
        cas_root: &Path,
    ) -> Self {
        let mut summary = Self {
            total: report.debt.len(),
            by_kind: BTreeMap::new(),
            previous_total: None,
            delta: None,
            grew: None,
            history_status: "unavailable",
            history_error: None,
        };
        for debt in &report.debt {
            *summary.by_kind.entry(debt.code.clone()).or_default() += 1;
        }
        match summary.record_previous(project, registry, cas_root) {
            Ok(()) => summary.history_status = "recorded",
            Err(error) => summary.history_error = Some(format!("{error:#}")),
        }
        summary
    }

    /// Reuse the project's metadata receipt, without initializing a store or schema.
    /// Serialize concurrent checks so each compares with the preceding recorded run.
    fn record_previous(&mut self, project: &Path, registry: &Path, cas_root: &Path) -> Result<()> {
        let key = format!(
            "qa_journeys_check:{}",
            serde_json::to_string(&(
                project.canonicalize()?,
                project.join(registry).canonicalize()?
            ))?
        );
        let mut connection = rusqlite::Connection::open_with_flags(
            cas_root.join("cas.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .context("open existing project database for journey check history")?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let previous: Option<String> = transaction
            .query_row("SELECT value FROM metadata WHERE key = ?1", [&key], |row| {
                row.get(0)
            })
            .optional()?;
        let previous_total = previous
            .map(|raw| -> Result<usize> {
                let receipt: serde_json::Value = serde_json::from_str(&raw)?;
                anyhow::ensure!(
                    receipt["version"] == 1,
                    "unsupported journey check receipt version"
                );
                let total = receipt["total"]
                    .as_u64()
                    .context("journey check receipt has no debt total")?;
                Ok(usize::try_from(total)?)
            })
            .transpose()?;
        let delta = previous_total
            .map(|previous| -> Result<i64> {
                Ok(i64::try_from(self.total)? - i64::try_from(previous)?)
            })
            .transpose()?;
        let receipt = serde_json::json!({
            "version": 1,
            "checked_at": chrono::Utc::now(),
            "total": self.total,
            "by_kind": self.by_kind,
            "previous_total": previous_total,
        });
        transaction.execute(
            "INSERT INTO metadata (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, serde_json::to_string(&receipt)?],
        )?;
        transaction.commit()?;
        self.previous_total = previous_total;
        self.delta = delta;
        self.grew = delta.map(|change| change > 0);
        Ok(())
    }
}

#[derive(Serialize)]
struct CheckReport<'a> {
    #[serde(flatten)]
    validation: &'a crate::qa_registry::ValidationReport,
    debt_summary: DebtSummary,
}

#[derive(Subcommand)]
pub enum QaCommand {
    /// Build an exact-revision binary for a task's real-build QA evidence
    Candidate(CandidateArgs),
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
        /// Show registry coverage and integration upkeep drift
        #[arg(long)]
        status: bool,
    },
}

pub fn execute(command: &QaCommand, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    match command {
        QaCommand::Candidate(args) => candidate(args, cli, cas_root),
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
                    let result = crate::qa_seed::seed(&project_root, &qa, *merge)?;
                    crate::config::refresh_upkeep(
                        cas_root.unwrap_or(&local_cas),
                        env!("CARGO_PKG_VERSION"),
                        true,
                        false,
                    )?;
                    result
                } else {
                    crate::qa_seed::status(&project_root, &qa)?
                };
                let upkeep = status.then(|| {
                    UpkeepReport::at(
                        &Measurement::integration_at(&project_root, cas_root.unwrap_or(&local_cas)),
                        &qa,
                    )
                });
                if cli.json {
                    let mut value = serde_json::to_value(&result)?;
                    if let Some(upkeep) = &upkeep {
                        value["upkeep"] = serde_json::to_value(upkeep)?;
                    }
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else {
                    let verdict = if !result.present {
                        "MISSING"
                    } else if !result.valid {
                        "INVALID"
                    } else if upkeep.as_ref().is_some_and(|report| {
                        report
                            .artifacts()
                            .iter()
                            .any(|artifact| artifact.drift_count.is_some_and(|count| count > 0))
                    }) {
                        "DRIFT"
                    } else if upkeep.as_ref().is_some_and(|report| {
                        report
                            .artifacts()
                            .iter()
                            .any(|artifact| artifact.drift_count.is_none())
                    }) {
                        "UNKNOWN"
                    } else if result.debt_count > 0 || !result.unowned_surfaces.is_empty() {
                        "DEBT"
                    } else {
                        "READY"
                    };
                    println!(
                        "{verdict}: {} journey(s), {} added",
                        result.journeys, result.added
                    );
                    if let Some(upkeep) = &upkeep {
                        let line = measured_ref_line(&upkeep.measured_ref);
                        println!("{line}");
                        for artifact in upkeep.artifacts() {
                            match artifact.drift_count {
                                Some(count) => {
                                    println!("{}: {count} {}", artifact.name, artifact.unit)
                                }
                                None => println!("{}: {}", artifact.name, artifact.state),
                            }
                            if let Some(debt) = artifact.debt_count {
                                println!("  Curation debt: {debt} (separate from drift)");
                            }
                            if cli.verbose
                                && let Some(detail) = &artifact.detail
                            {
                                println!("  {detail}");
                            }
                        }
                        if line != format!("Measured ref: {}", upkeep.measured_ref) {
                            println!("Full ref: cas qa journeys --status --json");
                        }
                    }
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
            let debt_summary = DebtSummary::record(
                &report,
                &project_root,
                Path::new(&registry_path),
                cas_root.unwrap_or(&local_cas),
            );
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&CheckReport {
                        validation: &report,
                        debt_summary,
                    })?
                );
            } else {
                let verdict = if !report.problems.is_empty() {
                    "FAIL"
                } else if !report.debt.is_empty() {
                    "DEBT"
                } else {
                    "PASS"
                };
                println!(
                    "{verdict}: {} problem(s), {} debt item(s)",
                    report.problems.len(),
                    report.debt.len()
                );
                if !debt_summary.by_kind.is_empty() {
                    println!("Debt by kind:");
                    for (kind, count) in &debt_summary.by_kind {
                        println!("  {kind}: {count}");
                    }
                }
                match (debt_summary.previous_total, debt_summary.delta) {
                    (Some(previous), Some(delta)) if delta > 0 => println!(
                        "Debt grew: +{delta} since previous check ({previous} -> {})",
                        debt_summary.total
                    ),
                    (Some(previous), Some(delta)) => println!(
                        "Debt change: {delta:+} since previous check ({previous} -> {})",
                        debt_summary.total
                    ),
                    _ if debt_summary.history_status == "recorded" => {
                        println!("Debt change: first recorded check");
                    }
                    _ => println!("Debt history unavailable; current check was not recorded."),
                }
                if !report.debt.is_empty() || debt_summary.history_error.is_some() {
                    println!("Details: cas qa journeys --check --verbose");
                }
                println!("Registry: {registry_path}");
                if cli.verbose
                    && let Some(error) = &debt_summary.history_error
                {
                    println!("History: {error}");
                }
                for (label, rows) in [("ERROR", &report.problems), ("DEBT", &report.debt)] {
                    if label == "DEBT" && !cli.verbose {
                        continue;
                    }
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

#[derive(Debug, Args)]
pub struct CandidateArgs {
    #[command(subcommand)]
    command: CandidateCommand,
}

#[derive(Debug, Subcommand)]
enum CandidateCommand {
    /// Build and place a binary at the task's canonical candidate path
    Build {
        #[arg(long)]
        task: String,
        /// Commit or ref to build (defaults to HEAD)
        #[arg(long, default_value = "HEAD")]
        rev: String,
        /// Print the output layout without building or creating files
        #[arg(long)]
        plan: bool,
    },
    /// Print a task's built candidate binary path (workers may run this)
    Path {
        #[arg(long)]
        task: String,
    },
}

#[derive(Serialize)]
struct CandidatePlan {
    task: String,
    revision: String,
    candidate_dir: PathBuf,
    binary: PathBuf,
    version_file: PathBuf,
    checksums_file: PathBuf,
    receipt_file: PathBuf,
    /// cas-d0e6: the short form that resolves `binary` from the task id.
    path_command: String,
}

#[derive(serde::Deserialize)]
struct CandidateReceipt {
    candidate_dir: PathBuf,
    revision: String,
    version: String,
    sha256: String,
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// cas-d0e6: a task's canonical candidate directory, from its id alone.
fn candidate_dir_for_task(cas_root: Option<&Path>, task: &str) -> Result<(PathBuf, PathBuf)> {
    ensure!(
        task.starts_with("cas-")
            && task.len() > 4
            && task
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid task id {task:?}"
    );
    let cas_root = cas_root.context("No Cassy project; run cas init first")?;
    let project = PathBuf::from(git_text(Path::new("."), &["rev-parse", "--show-toplevel"])?);
    let config = crate::config::Config::load(cas_root)?;
    let artifacts =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let artifacts = if artifacts.is_absolute() {
        artifacts
    } else {
        project.join(artifacts)
    };
    let candidate_dir = crate::config::project_factory_artifacts_root(cas_root, &artifacts)
        .join(task)
        .join("candidate");
    Ok((project, candidate_dir))
}

fn candidate_path_command(task: &str) -> String {
    format!("cas qa candidate path --task {task}")
}

fn candidate(args: &CandidateArgs, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    let (task, rev, plan) = match &args.command {
        CandidateCommand::Build { task, rev, plan } => (task, rev, plan),
        CandidateCommand::Path { task } => {
            let (_, candidate_dir) = candidate_dir_for_task(cas_root, task)?;
            let binary = candidate_dir.join("cas");
            ensure!(
                binary.is_file(),
                "no QA candidate built for {task} yet (expected {}); ask the supervisor to run \
                 `cas qa candidate build --task {task} --rev <sha>`",
                binary.display()
            );
            println!("{}", binary.display());
            return Ok(());
        }
    };
    let (project, candidate_dir) = candidate_dir_for_task(cas_root, task)?;
    let revision = git_text(
        &project,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ],
    )?;
    ensure!(
        revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "resolved revision is not a full commit SHA"
    );
    let layout = CandidatePlan {
        task: task.clone(),
        revision: revision.clone(),
        binary: candidate_dir.join("cas"),
        version_file: candidate_dir.join("VERSION.txt"),
        checksums_file: candidate_dir.join("SHA256SUMS"),
        receipt_file: candidate_dir.join("RECEIPT.json"),
        path_command: candidate_path_command(task),
        candidate_dir,
    };
    if *plan {
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&layout)?);
        } else {
            println!(
                "Candidate: {}\nRevision: {}\nFiles: cas, VERSION.txt, SHA256SUMS, RECEIPT.json\nFind it: {}",
                layout.candidate_dir.display(),
                layout.revision,
                layout.path_command
            );
        }
        return Ok(());
    }
    ensure!(
        std::env::var("CAS_AGENT_ROLE").ok().as_deref() != Some("worker"),
        "candidate builds are supervisor/operator-owned; ask the supervisor for the exact-tip binary"
    );
    let script = project.join("scripts/qa-candidate-build.py");
    ensure!(
        script.is_file(),
        "QA candidate builder missing: {}",
        script.display()
    );
    let output = Command::new("python3")
        .arg(script)
        .arg("--repo")
        .arg(&project)
        .arg("--rev")
        .arg(&revision)
        .arg("--candidate-dir")
        .arg(&layout.candidate_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .context("run QA candidate build")?;
    ensure!(
        output.status.success(),
        "QA candidate build exited {}",
        output.status
    );
    let receipt: CandidateReceipt =
        serde_json::from_slice(&output.stdout).context("read QA candidate build receipt")?;
    ensure!(
        receipt.revision == revision && receipt.candidate_dir == layout.candidate_dir,
        "candidate receipt does not match requested revision and path"
    );
    if cli.json {
        print!("{}", String::from_utf8(output.stdout)?);
    } else {
        println!(
            "Candidate: {}\nVersion: {}\nRevision: {}\nSHA-256: {}\nFind it: {}",
            receipt.candidate_dir.display(),
            receipt.version,
            receipt.revision,
            receipt.sha256,
            layout.path_command
        );
    }
    Ok(())
}
