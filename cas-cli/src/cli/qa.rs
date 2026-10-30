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
    let room = 80 - prefix.len();
    if reference.len() <= room {
        return format!("{prefix}{reference}");
    }
    let Some((branch, sha)) = reference.rsplit_once('@') else {
        return format!("{prefix}{}", crate::ui::widgets::truncate(reference, room));
    };
    let sha = if sha == "unavailable" { sha } else { sha.get(..7).unwrap_or(sha) };
    let branch_room = room.saturating_sub(sha.len() + 1);
    let branch = if branch.len() <= branch_room {
        branch.to_owned()
    } else {
        // Keep both branch ends and the measured commit; ASCII dots also work
        // in C locale. Byte budgets conservatively fit multibyte ref names.
        let tail = branch
            .char_indices()
            .find(|(index, _)| branch.len() - index <= branch_room / 2)
            .map(|(index, _)| &branch[index..])
            .unwrap_or_default();
        format!("{}{tail}", crate::ui::widgets::truncate(branch, branch_room - tail.len()))
    };
    format!("{prefix}{branch}@{sha}")
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
    #[serde(skip_serializing_if = "Option::is_none")]
    history_notice: Option<&'static str>,
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
            history_notice: None,
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
        let prior = previous
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
            .transpose();
        // A damaged receipt is not debt growth. Replace only this registry's
        // baseline, while genuine database/transaction failures still surface.
        let (previous_total, reset_history) = match prior {
            Ok(previous) => (previous, false),
            Err(_) => (None, true),
        };
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
        self.history_notice = reset_history
            .then_some("invalid prior record replaced; new baseline recorded.");
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
    /// Validate or scaffold a task evidence ledger
    Ledger(super::qa_ledger::LedgerArgs),
    /// Build an exact-revision binary for a task's real-build QA evidence
    Candidate(CandidateArgs),
    /// Map a diff to journeys and unowned CODEMAP areas
    Impact(super::qa_impact::ImpactArgs),
    /// List an epic's warning findings from approved verdicts, by file
    Polish(super::qa_polish::PolishArgs),
    /// Observe a release before announcing it or closing its epic
    PostRelease(super::qa_post_release::PostReleaseArgs),
    /// Operator-only, time-boxed rapid-response window for one epic
    RapidResponse(super::qa_rapid_response::RapidResponseArgs),
    /// Seed, inspect or validate the journey registry
    #[command(group(ArgGroup::new("journey_action").required(true).multiple(false).args(["check", "seed", "status", "traffic"])))]
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
        /// Per-journey traffic shares from Commander's local usage counts
        #[arg(long)]
        traffic: bool,
        /// Days of usage to count, ending today
        #[arg(long, requires = "traffic", default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=400))]
        days: u32,
        /// Write the shares and a traffic_snapshot header into the registry
        #[arg(long, requires = "traffic")]
        write: bool,
    },
}

/// Compare the checkout with the pinned upkeep commit, including a newly
/// created registry or family file that Git has not tracked yet. Unavailable is not equal.
fn working_tree_differs(root: &Path, registry: &str, upkeep: &UpkeepReport) -> Option<bool> {
    let (_, sha) = upkeep.measured_ref.rsplit_once('@')?;
    let diff = Command::new("git")
        .args(["diff", "--quiet", sha, "--"])
        .current_dir(root)
        .output()
        .ok()?;
    match diff.status.code()? {
        1 => Some(true),
        0 => {
            let families = Path::new(registry).parent()?.join("bug-families");
            // Include ignored files too: the working registry loader reads them.
            let untracked = Command::new("git")
                .args(["--literal-pathspecs", "ls-files", "--others", "-z", "--"])
                .arg(families)
                .current_dir(root)
                .output()
                .ok()?;
            if !untracked.status.success() {
                return None;
            }
            if std::str::from_utf8(&untracked.stdout)
                .ok()?
                .split('\0')
                .any(|path| crate::qa_registry::is_registry_input(registry, path))
            {
                return Some(true);
            }
            let committed = Command::new("git")
                .args(["show", &format!("{sha}:{registry}")])
                .current_dir(root)
                .output()
                .ok()?;
            Some(
                committed.status.success().then_some(committed.stdout)
                    != std::fs::read(root.join(registry)).ok(),
            )
        }
        _ => None,
    }
}

pub fn execute(command: &QaCommand, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    match command {
        QaCommand::Ledger(args) => super::qa_ledger::execute(args, cli, cas_root),
        QaCommand::Candidate(args) => candidate(args, cli, cas_root),
        QaCommand::Impact(args) => super::qa_impact::execute(args, cli, cas_root),
        QaCommand::Polish(args) => super::qa_polish::execute(args, cli, cas_root),
        QaCommand::PostRelease(args) => super::qa_post_release::execute(args, cli, cas_root),
        QaCommand::RapidResponse(args) => super::qa_rapid_response::execute(args, cas_root),
        QaCommand::Journeys {
            check: _,
            seed,
            merge,
            status,
            traffic,
            days,
            write,
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
            if *traffic {
                return journey_traffic(&project_root, &qa.registry.path, *days, *write, cli.json);
            }
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
                    let mut report = UpkeepReport::at(
                        &Measurement::integration_at(&project_root, cas_root.unwrap_or(&local_cas)),
                        &qa,
                    );
                    report.registry.unit = "validation/ownership issue(s)";
                    report
                });
                let differs = upkeep.as_ref().and_then(|report| {
                    working_tree_differs(&project_root, &qa.registry.path, report)
                });
                if cli.json {
                    let mut value = serde_json::to_value(&result)?;
                    if let Some(upkeep) = &upkeep {
                        value["source"] = serde_json::json!("working_tree");
                        value["differs_from_measured_ref"] = serde_json::to_value(differs)?;
                        value["upkeep"] = serde_json::to_value(upkeep)?;
                    }
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else {
                    let verdict = if !result.present {
                        "MISSING"
                    } else if !result.valid {
                        "INVALID"
                    } else if result.debt_count > 0 {
                        "DEBT"
                    } else if *status {
                        "PASS"
                    } else if !result.unowned_surfaces.is_empty() {
                        "DEBT"
                    } else {
                        "READY"
                    };
                    if *status {
                        println!(
                            "{verdict}: {} problem(s), {} debt item(s) (working tree)",
                            result.problems.len(), result.debt_count
                        );
                    } else {
                        println!(
                            "{verdict}: {} journey(s), {} added",
                            result.journeys, result.added
                        );
                    }
                    println!(
                        "Next: cas qa journeys --{}",
                        if result.present { "check" } else { "seed" }
                    );
                    if *status {
                        println!("Working tree:");
                        println!("  Journeys: {}  Added: {}", result.journeys, result.added);
                        println!(
                            "  Seeded: {}  Unowned surfaces: {}",
                            result.seeded, result.unowned_surfaces.len()
                        );
                        if let Some(line) = result.planned_specs_line() {
                            println!("{line}");
                        }
                        if differs == Some(true) {
                            println!("Working tree differs from measured ref.");
                        }
                    }
                    if let Some(upkeep) = &upkeep {
                        println!("Integration upkeep:");
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
                            if (cli.verbose || artifact.name == "journey registry")
                                && let Some(detail) = &artifact.detail
                            {
                                println!("  {detail}");
                            }
                        }
                        if line != format!("Measured ref: {}", upkeep.measured_ref) {
                            println!("Full ref: cas qa journeys --status --json");
                        }
                    }
                    if !*status {
                        println!(
                            "Seeded: {}  Unowned surfaces: {}",
                            result.seeded, result.unowned_surfaces.len()
                        );
                    }
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
                if let Some(notice) = debt_summary.history_notice {
                    println!("History reset: {notice}");
                }
                for (group, journey) in &report.single_reference_owner_groups {
                    let group = group.escape_debug().to_string();
                    let journey = journey.escape_debug().to_string();
                    let warning = format!("WARN owner group {group}: only journey {journey}");
                    if warning.len() <= 80 {
                        println!("{warning}");
                    } else {
                        let suffix = " (full: --json)";
                        let room = 80 - "WARN owner group ".len() - ": only journey ".len() - suffix.len();
                        let group_room = (room / 2).max(room.saturating_sub(journey.len()));
                        let group = crate::ui::widgets::truncate(&group, group_room);
                        let journey = crate::ui::widgets::truncate(&journey, room - group.len());
                        println!("WARN owner group {group}: only journey {journey}{suffix}");
                    }
                }
                if !cli.verbose && (!report.debt.is_empty() || debt_summary.history_error.is_some()) {
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

/// `cas qa journeys --traffic` (cas-0b3d): shares from this machine's hub ledger.
fn journey_traffic(project_root: &Path, registry_path: &str, days: u32, write: bool, json: bool) -> Result<()> {
    let path = project_root.join(registry_path);
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let registry = crate::qa_registry::parse(&text)?;
    let hub = crate::hub::HubRuntimePaths::default_for_user()?;
    let ledger_path = hub.root().join(crate::hub::usage::USAGE_FILE);
    let ledger = crate::hub::usage::read_ledger(hub.root())?.unwrap_or_default();
    let report = crate::qa_traffic::report(&registry, &ledger, chrono::Utc::now().date_naive(), days);
    if write {
        std::fs::write(&path, crate::qa_traffic::apply(&text, &report)?)
            .with_context(|| format!("write {}", path.display()))?;
    }
    if json {
        let mut value = serde_json::to_value(&report)?;
        value["ledger"] = serde_json::json!(ledger_path);
        value["written"] = serde_json::json!(write);
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if report.actions == 0 {
        println!(
            "NO DATA: no counted Commander actions in the {} days to {}.",
            report.window_days, report.as_of
        );
        println!("Use Commander with a build that counts usage, then rerun.");
    } else {
        println!(
            "{} counted Commander actions in the {} days to {}",
            report.actions, report.window_days, report.as_of
        );
        for share in &report.journeys {
            println!("  {:>5.1}%  {:>6}  {}  {}", share.share_pct, share.count, share.id, share.name);
        }
    }
    for (id, count) in &report.unregistered {
        println!("  not in registry: {id} ({count})");
    }
    println!("{}", report.snapshot_line());
    if write {
        println!("Wrote shares to {registry_path}");
    } else if report.actions > 0 {
        println!("Next: cas qa journeys --traffic --days {days} --write");
    }
    println!("Ledger: {}", ledger_path.display());
    Ok(())
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

#[derive(Serialize, serde::Deserialize)]
struct CandidateReceipt {
    // The caller owns these paths; the builder's disk manifest contains
    // identity fields, not its stdout-only candidate_dir summary field.
    #[serde(skip_deserializing)]
    candidate_dir: PathBuf,
    #[serde(skip_deserializing)]
    receipt_file: PathBuf,
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
        "QA candidate builder missing: scripts/qa-candidate-build.py\n\
         Use an up-to-date Cassy source checkout, then rerun:\n\
           cas qa candidate build --task {task} \\\n    --rev {revision}"
    );
    let status = Command::new("python3")
        .arg(script)
        .arg("--repo")
        .arg(&project)
        .arg("--rev")
        .arg(&revision)
        .arg("--candidate-dir")
        .arg(&layout.candidate_dir)
        // Git and cleanup helpers may write progress to stdout. The
        // published file, not that stream, is the receipt contract.
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .context("run QA candidate build")?;
    ensure!(
        status.success(),
        "QA candidate build exited {}",
        status
    );
    let mut receipt: CandidateReceipt = serde_json::from_slice(
        &std::fs::read(&layout.receipt_file)
            .with_context(|| format!("read QA candidate build receipt {}", layout.receipt_file.display()))?,
    ).context("read QA candidate build receipt")?;
    ensure!(
        receipt.revision == revision,
        "candidate receipt does not match requested revision"
    );
    receipt.candidate_dir = layout.candidate_dir;
    receipt.receipt_file = layout.receipt_file;
    if cli.json {
        println!("{}", serde_json::to_string(&receipt)?);
    } else {
        println!(
            "Candidate: {}\nVersion: {}\nRevision: {}\nSHA-256: {}\nReceipt: {}\nFind it: {}",
            receipt.candidate_dir.display(),
            receipt.version,
            receipt.revision,
            receipt.sha256,
            receipt.receipt_file.display(),
            layout.path_command
        );
    }
    Ok(())
}
