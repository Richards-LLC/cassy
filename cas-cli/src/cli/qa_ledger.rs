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
    task.map_or_else(|| crate::qa_ledger::check(path, 1), |task| crate::qa_ledger::check_for_task(path, task))
}

pub(crate) fn add_release_obligations<'a>(
    store: &dyn crate::store::TaskStore,
    members: impl IntoIterator<Item = (&'a str, &'a str)>,
    journeys: &mut Vec<(String, String)>,
) -> Result<Vec<String>> {
    let mut owed = std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for (task, anchor) in members {
        let rows = store.get_execution_state(task)?
            .and_then(|state| {
                let impact = &state["qa_impact"];
                (impact["head_sha"].as_str() == Some(anchor))
                    .then(|| impact.get("g61_deferred_journeys").cloned()).flatten()
            })
            .with_context(|| format!("G61_DEFERRED_STATE_MISSING: {task} at {anchor}"))?;
        for row in serde_json::from_value::<Vec<crate::qa_registry::DeferredSpec>>(rows)
            .with_context(|| format!("G61_DEFERRED_STATE_INVALID: {task} at {anchor}"))?
        {
            owed.entry(row.id).or_default().insert(format!("{} ({})", row.spec, row.reason.as_str()));
        }
    }
    let ids = owed.keys().cloned().collect();
    for (id, specs) in owed {
        let obligation = format!("Owed to release batch QA: {}", specs.into_iter().collect::<Vec<_>>().join(", "));
        if let Some((_, name)) = journeys.iter_mut().find(|(journey, _)| journey == &id) {
            *name = format!("{name} — {obligation}");
        } else {
            journeys.push((id, obligation));
        }
    }
    Ok(ids)
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
            let caller = std::env::current_dir()?;
            let same_repo = repo_context::git_layout(&caller).ok()
                .zip(repo_context::git_layout(&repo).ok())
                .is_some_and(|((_, caller_common), (_, target_common))| caller_common == target_common);
            let repo = if same_repo {
                PathBuf::from(crate::qa_impact::io::git(&caller, &["rev-parse", "--show-toplevel"])?.trim()).canonicalize()?
            } else { repo };
            let config = crate::config::Config::load(cas_root)?;
            let impact = crate::qa_impact::predicted(&repo, &config.qa(), &task)?;
            let mut journeys = impact
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
            let release = release_scope.as_deref()
                .map(|scope| super::release_adapters::review_blocks_for_scope(cas_root, scope)).transpose()?;
            let members = if let Some((inventory, _)) = &release {
                inventory.deliveries.iter().filter_map(|delivery|
                    Some((delivery.task_id.clone(), delivery.anchor.clone()?))).collect::<Vec<_>>()
            } else {
                let mut members = Vec::new();
                for id in covers {
                    let covered = match store.get(id) {
                        Ok(covered) => covered,
                        Err(crate::store::StoreError::TaskNotFound(_)) => continue,
                        Err(error) => return Err(error.into()),
                    };
                    if crate::qa_batch::is_report_member(&covered) {
                        continue;
                    }
                    let (repo, target) = crate::qa_batch::task_context(cas_root, &covered).map_err(anyhow::Error::msg)?;
                    let anchor = crate::qa_batch::resolved_delivery_anchor(&repo, &covered, &target)
                        .with_context(|| format!("G61_DEFERRED_STATE_MISSING: {id} has no delivery anchor"))?;
                    members.push((id.clone(), anchor));
                }
                members
            };
            let owed = add_release_obligations(store.as_ref(), members.iter().map(|(id, sha)| (id.as_str(), sha.as_str())), &mut journeys)?;
            let revision = match (release.as_ref(), source) {
                (Some((inventory, _)), _) => inventory.scope.final_release_sha.clone(),
                (None, Some(branch)) => {
                    // A local source remains authoritative for ongoing work.
                    // After branch cleanup, use the same immutable delivery
                    // identity as batch receipts, never a movable tag or HEAD.
                    crate::qa_impact::io::resolve_ref(&repo, &format!("refs/heads/{branch}"))
                        .or_else(|error| crate::qa_batch::delivery_anchor(&repo, &task).ok_or(error))
                        .with_context(|| {
                            format!("resolve recorded task source branch {branch:?} for {id}")
                        })?
                }
                (None, None) if task.deliverables.factory_branch_anchor.is_some()
                    || !task.deliverables.historical_factory_branch_anchors.is_empty() =>
                {
                    crate::qa_batch::delivery_anchor(&repo, &task)
                        .with_context(|| format!("resolve recorded delivery anchor for {id}"))?
                }
                (None, None) => crate::qa_impact::io::resolve_ref(&repo, "HEAD")?,
            };
            let mut covers = match &release {
                Some((inventory, _)) => inventory.deliveries.iter().map(|d| d.task_id.clone()).collect(),
                None if covers.is_empty() => vec![id.clone()],
                None => covers.clone(),
            };
            covers.sort_unstable();
            covers.dedup();
            let mut body = crate::qa_ledger::scaffold_with_coverage(&task, &journeys, revision.trim(), &covers);
            if let Some((_, blocks)) = release {
                body.push_str("\n## Release review blocks\n\nRemove any epic whose acceptance this review did not verify.\n\n");
                body.push_str(&blocks);
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
            for id in owed {
                ensure!(report.rows.iter().any(|row| row.id == id),
                    "G61_DEFERRED_ROWS_MISSING: {} needs a row for {id}; nothing was overwritten", path.display());
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn init_uses_callers_worktree_registry_and_head_gh1975() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("primary");
        std::fs::create_dir(&repo).unwrap();
        let git = |at: &Path, args: &[&str]| {
            let output = std::process::Command::new("git").current_dir(at)
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/form.rs"), "pub fn form() {}\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "primary"]);
        let root = crate::store::init_cas_dir(&repo).unwrap();
        let checkout = dir.path().join("worker");
        git(&repo, &["worktree", "add", "-q", "-b", "factory/fixture", checkout.to_str().unwrap()]);
        std::fs::create_dir_all(checkout.join("docs/qa")).unwrap();
        std::fs::write(checkout.join(crate::qa_registry::DEFAULT_PATH),
            "version: 1\nupdated: '2026-11-17'\ninvariants:\n  INV: {statement: Arrived, check: Read state}\njourneys:\n  - id: HUB-J9\n    name: Worktree form\n    status: live\n    entry: {url: /form, trigger: Open}\n    steps: [Arrive]\n    owners: ['src/form.rs']\n    invariants: [INV]\n").unwrap();
        git(&checkout, &["add", "."]);
        git(&checkout, &["commit", "-qm", "worker registry"]);
        let head = git(&checkout, &["rev-parse", "HEAD"]);
        env.set_current_dir(checkout.join("src"));
        let mut config = crate::config::Config::default();
        let artifacts = dir.path().join("artifacts");
        config.set("factory.artifacts_root", artifacts.to_str().unwrap()).unwrap();
        config.save(&root).unwrap();
        let task = cas_types::Task::new("cas-worktree-ledger".into(), "Review src/form.rs".into());
        let store = crate::store::open_task_store(&root).unwrap();
        store.init().unwrap();
        store.create_atomic(&task, &[], None, None).unwrap();
        let args = LedgerArgs { command: LedgerCommand::Init { task: task.id.clone(), covers: Vec::new(), release_scope: None } };
        execute(&args, &Cli::parse_from(["cas"]), Some(&root)).unwrap();
        let path = crate::config::resolved_factory_artifact_paths(&root, config.factory().artifacts_root.as_deref())
            .project_root.join(&task.id).join("LEDGER.md");
        let body = std::fs::read_to_string(path).unwrap();
        assert_eq!(body.lines().find_map(|line| line.strip_prefix("build: ")), Some(head.as_str()));
        assert_eq!(crate::qa_ledger::parse(&body, 1).rows[0].id, "HUB-J9");
        assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&checkout, &["symbolic-ref", "--short", "HEAD"]), "factory/fixture");

        let foreign = dir.path().join("foreign");
        std::fs::create_dir(&foreign).unwrap();
        git(&foreign, &["init", "-q", "-b", "main"]);
        std::fs::write(foreign.join("foreign.txt"), "unrelated caller\n").unwrap();
        git(&foreign, &["add", "."]);
        git(&foreign, &["commit", "-qm", "foreign"]);
        env.set_current_dir(&foreign);
        let primary_head = git(&repo, &["rev-parse", "HEAD"]);
        for (id, source, expected) in [
            ("cas-foreign-caller", None, primary_head.as_str()),
            ("cas-recorded-source", Some("factory/fixture"), head.as_str()),
        ] {
            let mut other = task.clone();
            other.id = id.into();
            other.deliverables.parked_branch = source.map(str::to_string);
            store.create_atomic(&other, &[], None, None).unwrap();
            let args = LedgerArgs { command: LedgerCommand::Init { task: id.into(), covers: Vec::new(), release_scope: None } };
            execute(&args, &Cli::parse_from(["cas"]), Some(&root)).unwrap();
            let path = crate::config::resolved_factory_artifact_paths(&root, config.factory().artifacts_root.as_deref())
                .project_root.join(id).join("LEDGER.md");
            let body = std::fs::read_to_string(path).unwrap();
            assert_eq!(body.lines().find_map(|line| line.strip_prefix("build: ")), Some(expected));
        }
    }
}
