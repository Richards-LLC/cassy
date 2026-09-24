//! Rolling union of open epics. All mutations happen in a detached checkout
//! under the repository's delivery-target lock; source epics are never reset.

use super::*;
use cas_types::{Task, TaskStatus, TaskType};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EpicTip {
    id: String,
    branch: String,
    tip: String,
    owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct BaseFailure {
    pub(super) base: String,
    pub(super) failing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IntegrationReceipt {
    base: String,
    /// GH #954: the trunk branch `base` was read from (`origin/<trunk>`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    trunk: String,
    epics: Vec<EpicTip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    already_integrated: Vec<EpicTip>,
    tip: Option<String>,
    status: String,
    detail: String,
    affected: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) test_process_env_scrubbed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_failure: Option<BaseFailure>,
    /// GH #1006: consecutive deferrals not yet followed by a completed run.
    /// Kept across a RUNNING receipt, so an interrupted run still reports.
    #[serde(default, skip_serializing_if = "is_zero")]
    deferrals: u32,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// GH #1006: deferrals outstanding before this run, read from the previous
/// receipt. A legacy `DEFERRED` receipt without a count is one deferral.
fn outstanding_deferrals(receipt_path: &Path) -> u32 {
    let Some(previous) = fs::read_to_string(receipt_path)
        .ok()
        .and_then(|contents| serde_json::from_str::<IntegrationReceipt>(&contents).ok())
    else {
        return 0;
    };
    match previous.status.as_str() {
        "DEFERRED" => previous.deferrals.max(1),
        "RUNNING" => previous.deferrals,
        _ => 0,
    }
}

const ROW_CACHE_FORMAT: &str = "row-cache-v2";
const ROW_CACHE_DIR: &str = "row-cache";
const NEXTTEST_ROW: &str = "nextest";
const GATE_INIT_TIMEOUT_SECS: &str = "900";

#[derive(Debug)]
enum Assembly {
    Clean {
        tip: String,
        prefixes: Vec<String>,
    },
    Conflict {
        detail: String,
        affected: Vec<String>,
    },
}

/// Build the union in creation order. Conflict probes use Git's merge engine,
/// rather than assuming every epic that touched a conflicted file conflicts.
fn assemble(
    worktree: &Path,
    base: &str,
    base_label: &str,
    epics: &[EpicTip],
) -> Result<Assembly, String> {
    git_output(worktree, &["reset", "--hard", base])?;
    let mut prefixes = vec![base.to_owned()];
    for (index, epic) in epics.iter().enumerate() {
        let merge = Command::new("git")
            .current_dir(worktree)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "merge",
                "--no-edit",
                "--no-ff",
                &epic.tip,
            ])
            .output()
            .map_err(|error| error.to_string())?;
        if !merge.status.success() {
            let files =
                git_output_allow_empty(worktree, &["diff", "--name-only", "--diff-filter=U"])?;
            let _ = git_output(worktree, &["merge", "--abort"]);
            if files.is_empty() {
                return Err(format!(
                    "merge {} failed: {}",
                    epic.id,
                    first_output_line(&merge.stderr)
                ));
            }
            let mut pairs = Vec::new();
            // Probe the actual base first. If it conflicts, the content came
            // from main rather than any prior epic in this rolling union.
            let base_conflict = merge_tree_conflicts(worktree, base, &epic.tip)?;
            if !base_conflict {
                // Probe each prior epic against the incoming epic without
                // touching the assembly checkout or manufacturing a commit.
                for prior in &epics[..index] {
                    if merge_tree_conflicts(worktree, &prior.tip, &epic.tip)? {
                        pairs.push(prior.id.clone());
                    }
                }
            }
            if pairs.is_empty() && !base_conflict {
                // Main or a higher-order interaction; name it honestly instead
                // of inventing a pair unsupported by a merge probe.
                pairs.extend(epics[..index].iter().map(|prior| prior.id.clone()));
            }
            let against = if base_conflict || index == 0 {
                base_label.to_owned()
            } else {
                pairs.join(", ")
            };
            pairs.push(epic.id.clone());
            return Ok(Assembly::Conflict {
                detail: format!(
                    "Conflict adding {} against {}. Files: {}",
                    epic.id,
                    against,
                    files.lines().collect::<Vec<_>>().join(", ")
                ),
                affected: pairs,
            });
        }
        prefixes.push(git_output(worktree, &["rev-parse", "HEAD"])?);
    }
    Ok(Assembly::Clean {
        tip: git_output(worktree, &["rev-parse", "HEAD"])?,
        prefixes,
    })
}

fn open_epics(mut tasks: Vec<Task>) -> Vec<Task> {
    tasks.retain(|task| {
        task.task_type == TaskType::Epic
            && !matches!(task.status, TaskStatus::Closed | TaskStatus::Cancelled)
    });
    tasks.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    tasks
}

/// An epic's legacy `branch` is its coordination branch. A WorkTarget on an
/// epic names the branch it delivers into (usually `main`), so it must not
/// replace the coordination branch when assembling the rolling union.
fn epic_branch(task: &Task) -> Option<&str> {
    task.branch
        .as_deref()
        .filter(|branch| branch.starts_with("epic/"))
        .or_else(|| {
            task.deliverables
                .work_target
                .as_ref()
                .map(|target| target.target_branch.as_str())
                .filter(|branch| branch.starts_with("epic/"))
        })
}

fn ref_tip(root: &Path, reference: &str) -> Option<String> {
    git_output(
        root,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )
    .ok()
}

/// GH #954: the trunk the rolling union is built on, and its fetched tip.
///
/// The sweep used to read `origin/main` unconditionally, so a repository whose
/// trunk is `master` (or anything else) failed before it assembled anything.
/// Resolution order, first match wins:
/// 1. the configured trunk, `[factory] epic_base_branch` (an explicit setting
///    that does not resolve on origin is an error, never silently skipped);
/// 2. `origin/HEAD`, the remote's own default;
/// 3. `origin/main`, then `origin/master`.
///
/// Every candidate must resolve under `refs/remotes/origin/`: the union is
/// built on what origin holds, not on a local branch that may be ahead of it.
fn resolve_integration_trunk(
    root: &Path,
    configured: Option<&str>,
) -> Result<(String, String), String> {
    let remote_tip = |branch: &str| ref_tip(root, &format!("refs/remotes/origin/{branch}"));
    if let Some(configured) = configured
        .map(str::trim)
        .map(|branch| branch.strip_prefix("origin/").unwrap_or(branch))
        .filter(|branch| !branch.is_empty())
    {
        return remote_tip(configured)
            .map(|tip| (configured.to_owned(), tip))
            .ok_or_else(|| {
                format!(
                    "configured trunk `{configured}` ([factory] epic_base_branch) does not \
                     resolve as origin/{configured}"
                )
            });
    }
    if let Ok(reference) = git_output(
        root,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) && let Some(branch) = reference.trim().strip_prefix("refs/remotes/origin/")
        && !branch.is_empty()
        && let Some(tip) = remote_tip(branch)
    {
        return Ok((branch.to_owned(), tip));
    }
    for candidate in ["main", "master"] {
        if let Some(tip) = remote_tip(candidate) {
            return Ok((candidate.to_owned(), tip));
        }
    }
    Err(
        "cannot resolve the trunk to integrate on: origin/HEAD is unset and neither \
         origin/main nor origin/master exists; set [factory] epic_base_branch or run \
         `git remote set-head origin --auto`"
            .to_owned(),
    )
}

/// The configured trunk for the repository at `main_root`, if any.
fn configured_trunk(main_root: &Path) -> Option<String> {
    crate::config::Config::configured_epic_base_branch(main_root)
}

fn merge_tree_conflicts(root: &Path, left: &str, right: &str) -> Result<bool, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["merge-tree", "--write-tree", left, right])
        .output()
        .map_err(|error| error.to_string())?;
    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(format!(
            "conflict attribution probe {left} vs {right}: {}",
            first_output_line(&output.stderr)
        )),
    }
}

/// Build the set from all open task rows, independent of ownership. Rows with
/// no live local or origin ref are ignored; a divergent local/origin pair is
/// still an actionable integration error and is preserved for the caller.
/// Tips already contained in `base` are returned separately for the receipt.
fn live_open_epics(
    root: &Path,
    base: &str,
    tasks: Vec<Task>,
) -> Result<(Vec<EpicTip>, Vec<EpicTip>), String> {
    let mut epics = Vec::new();
    let mut already_integrated = Vec::new();
    for task in open_epics(tasks) {
        let Some(branch) = epic_branch(&task).map(str::to_owned) else {
            continue;
        };
        let local = ref_tip(root, &format!("refs/heads/{branch}"));
        let remote = ref_tip(root, &format!("refs/remotes/origin/{branch}"));
        if local.is_none() && remote.is_none() {
            continue;
        }
        let tip = match resolve_epic_tip(root, &branch) {
            Ok(tip) => tip,
            Err(error) => return Err(error),
        };
        let epic = EpicTip {
            id: task.id,
            branch,
            tip,
            owner: task.epic_verification_owner.or(task.assignee),
        };
        if git_output(root, &["merge-base", "--is-ancestor", &epic.tip, base]).is_ok() {
            already_integrated.push(epic);
        } else {
            epics.push(epic);
        }
    }
    Ok((epics, already_integrated))
}

fn focused_epic_id_for_project(root: &Path) -> Option<String> {
    let session = std::env::var("CAS_FACTORY_SESSION")
        .ok()
        .filter(|session| !session.trim().is_empty())?;
    let path = crate::ui::factory::session::metadata_path(&session);
    let metadata = serde_json::from_slice::<crate::ui::factory::protocol::SessionMetadata>(
        &fs::read(path).ok()?,
    )
    .ok()?;
    let metadata_project = metadata
        .project_dir
        .as_deref()
        .filter(|project| !project.trim().is_empty())
        .and_then(|project| fs::canonicalize(project).ok())?;
    if fs::canonicalize(root).ok()? != metadata_project {
        return None;
    }
    metadata
        .pinned_epic_id
        .or(metadata.epic_id)
        .filter(|id| !id.trim().is_empty())
}

pub(super) fn execute(
    project_root: &Path,
    cas_dir: &Path,
    request: SweepRequest,
    settings: SweepSettings,
    cancel: Arc<AtomicBool>,
    strict_target: bool,
) -> SweepResult {
    match integrate(
        project_root,
        cas_dir,
        &request,
        &settings,
        &cancel,
        strict_target,
    ) {
        Ok(result) => result,
        Err(error) => SweepResult {
            integration_epics: vec![request.epic_id.clone()],
            request,
            status: SweepStatus::SetupFailed,
            log_path: cas_dir.join(LOG_DIR).join("integration.json"),
            summary: format!("Rolling integration setup failed: {error}"),
            failures: Vec::new(),
            base_failure: None,
            after_deferrals: 0,
        },
    }
}

fn integrate(
    project_root: &Path,
    cas_dir: &Path,
    request: &SweepRequest,
    settings: &SweepSettings,
    cancel: &Arc<AtomicBool>,
    strict_target: bool,
) -> Result<SweepResult, String> {
    let common = PathBuf::from(git_output(
        project_root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?);
    let main_root = common
        .parent()
        .ok_or("Git common directory has no parent")?;
    // Use the shared root, not the caller's linked worktree, for lock identity
    // and receipt placement. Every factory session contends on the same key.
    let shared_cas = main_root.join(".cas");
    let project = main_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("project name missing")?;
    let branch = format!("integration/{}", sanitize_component(project));
    let waiting = Instant::now();
    let _lock = loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(SweepResult {
                request: request.clone(),
                status: SweepStatus::Superseded,
                log_path: shared_cas.join(LOG_DIR).join("integration.json"),
                summary: "Integration superseded while waiting for lock".into(),
                failures: Vec::new(),
                integration_epics: vec![request.epic_id.clone()],
                base_failure: None,
                after_deferrals: 0,
            });
        }
        if let Some(lock) =
            crate::worktree::target_lock::try_lock_delivery_target(&shared_cas, &common, &branch)
                .map_err(|error| error.to_string())?
        {
            break lock;
        }
        if waiting.elapsed() >= settings.timeout {
            return Err("integration lock wait timed out".into());
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    // Recovery is deliberately stricter than the ordinary daemon path: after
    // taking the shared integration lock, confirm the event still names the
    // exact current local epic ref before touching receipts or integration refs.
    if strict_target {
        validate_recovery_target_under_lock(project_root, request)?;
    }
    let receipt_path = shared_cas.join(LOG_DIR).join("integration.json");
    let prior_deferrals = outstanding_deferrals(&receipt_path);
    // Invalidate an old green receipt before any fallible setup, so a failed
    // fetch or missing epic can never leave release assembly looking green.
    let mut receipt = IntegrationReceipt {
        base: String::new(),
        trunk: String::new(),
        epics: Vec::new(),
        already_integrated: Vec::new(),
        tip: None,
        status: "RUNNING".to_owned(),
        detail: format!(
            "Triggered by {} at {}; {}",
            request.epic_id,
            request.commit,
            super::test_process_identity_receipt_note()
        ),
        affected: vec![request.epic_id.clone()],
        test_process_env_scrubbed: super::scrubbed_test_process_identity_names(),
        base_failure: None,
        deferrals: prior_deferrals,
    };
    write_receipt(&receipt_path, &receipt)?;
    // Invalidate an old failed sweep report at the same boundary. A fetch,
    // assembly, or build-guard error must never leave yesterday's classes
    // available for a later `sweep_tasks accept=true` call.
    crate::factory_sweep_tasks::write_report(
        &shared_cas,
        &crate::factory_sweep_tasks::SweepTaskReport {
            schema_version: 1,
            status: "RUNNING".to_owned(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            integration_branch: branch.clone(),
            integration_tip: String::new(),
            source_epic: request.epic_id.clone(),
            affected_epics: vec![request.epic_id.clone()],
            log_path: receipt_path.display().to_string(),
            failure_count: 0,
            classes: Vec::new(),
            accepted_at: None,
        },
    )?;
    git_output(project_root, &["fetch", "--prune", "origin"])?;
    // GH #954: the trunk is resolved, not assumed to be main.
    let (trunk, base) =
        resolve_integration_trunk(project_root, configured_trunk(main_root).as_deref())?;
    let base_label = format!("origin/{trunk}");
    receipt.base = base.clone();
    receipt.trunk = trunk.clone();
    let store = crate::store::open_task_store(&shared_cas).map_err(|error| error.to_string())?;
    let mut tasks = store.list(None).map_err(|error| error.to_string())?;
    if let Some(focused_id) = focused_epic_id_for_project(main_root) {
        if !tasks.iter().any(|task| task.id == focused_id) {
            if let Ok(task) = store.get(&focused_id) {
                tasks.push(task);
            }
        }
    }
    let (epics, already_integrated) = live_open_epics(project_root, &base, tasks)?;
    receipt.epics = epics;
    receipt.already_integrated = already_integrated;
    let synthetic = SweepRequest {
        epic_id: format!("integration-{}", sanitize_component(project)),
        target_branch: branch.clone(),
        commit: base.clone(),
    };
    let worktree = prepare_merge_worktree(project_root, &synthetic)?;
    let previous = git_output(
        project_root,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )
    .ok();
    let (tip, prefixes) = match assemble(&worktree, &base, &base_label, &receipt.epics)? {
        Assembly::Conflict { detail, affected } => {
            receipt.status = "CONFLICT".to_owned();
            receipt.detail = detail.clone();
            receipt.affected = affected.clone();
            receipt.deferrals = 0;
            write_receipt(&receipt_path, &receipt)?;
            return Ok(SweepResult {
                request: request.clone(),
                status: SweepStatus::Failed,
                log_path: receipt_path,
                summary: detail,
                failures: Vec::new(),
                integration_epics: affected,
                base_failure: None,
                after_deferrals: prior_deferrals,
            });
        }
        Assembly::Clean { tip, prefixes } => (tip, prefixes),
    };
    if cancel.load(Ordering::Relaxed) {
        return Ok(SweepResult {
            request: request.clone(),
            status: SweepStatus::Superseded,
            log_path: receipt_path,
            summary: "Integration superseded before publication".to_owned(),
            failures: Vec::new(),
            integration_epics: vec![request.epic_id.clone()],
            base_failure: None,
            after_deferrals: 0,
        });
    }
    // The detached checkout can move freely; publication alone changes the
    // shared integration ref, with a CAS guard against non-Cassy writers.
    git_output(
        project_root,
        &[
            "update-ref",
            &format!("refs/heads/{branch}"),
            &tip,
            previous.as_deref().unwrap_or(""),
        ],
    )?;
    receipt.tip = Some(tip.clone());
    write_receipt(&receipt_path, &receipt)?;
    let guard = crate::factory_build_guard::inspect(cas_dir, &settings_to_config(settings), 1);
    if !guard.violations().is_empty() {
        receipt.status = "DEFERRED".to_owned();
        receipt.detail = guard.violations().join("; ");
        receipt.deferrals = prior_deferrals.saturating_add(1);
        write_receipt(&receipt_path, &receipt)?;
        return Ok(SweepResult {
            request: request.clone(),
            status: SweepStatus::Deferred,
            log_path: receipt_path,
            summary: format!("{branch} updated; sweep deferred: {}", receipt.detail),
            failures: Vec::new(),
            integration_epics: vec![request.epic_id.clone()],
            base_failure: None,
            after_deferrals: 0,
        });
    }
    let mut result = execute_sweep(
        project_root,
        cas_dir,
        SweepRequest {
            commit: tip.clone(),
            ..synthetic.clone()
        },
        settings.clone(),
        Arc::clone(cancel),
    );
    // GH #1006: a configured sweep command is not the release gate's nextest
    // row, so its pass must not certify that row.
    if result.status == SweepStatus::Passed && settings.command.is_none() {
        if let Err(error) =
            write_sweep_row_receipt(project_root, &shared_cas, &result.request, NEXTTEST_ROW)
        {
            result.status = SweepStatus::SetupFailed;
            result.summary.push_str(&format!(
                "; could not write release-gate row receipt: {error}"
            ));
        }
    }
    let mut affected = vec![request.epic_id.clone()];
    let mut base_failure = None;
    if result.status == SweepStatus::Failed {
        let mut probe_settings = settings.clone();
        probe_settings.nextest_filter = failing_filter(&result.failures);
        let mut probe = |commit: &str| {
            let probe_result = execute_sweep(
                project_root,
                cas_dir,
                SweepRequest {
                    commit: commit.to_owned(),
                    ..synthetic.clone()
                },
                probe_settings.clone(),
                Arc::clone(cancel),
            );
            match probe_result.status {
                SweepStatus::Passed => Ok(true),
                SweepStatus::Failed => Ok(false),
                other => Err(format!(
                    "attribution probe {} at {commit}; log {}",
                    status_text(other),
                    probe_result.log_path.display()
                )),
            }
        };
        if let Some(prior) = &previous {
            match probe(prior) {
                Ok(true) => result
                    .summary
                    .push_str("; failing targets passed on the previous integration tip"),
                Ok(false) => result
                    .summary
                    .push_str("; failing targets also fail on the previous integration tip"),
                Err(error) => result.summary.push_str(&format!("; {error}")),
            }
        }
        match introducing_epic(&prefixes, &mut probe) {
            Ok(Some(index)) => {
                result.summary.push_str(&format!(
                    "; reported failing targets introduced by {}",
                    receipt.epics[index].id
                ));
                affected = receipt.epics[..=index]
                    .iter()
                    .map(|epic| epic.id.clone())
                    .collect();
            }
            Ok(None) => {
                result.summary.push_str(&format!(
                    "; failing targets also fail on {base_label} (no epic attribution)"
                ));
                affected = receipt.epics.iter().map(|epic| epic.id.clone()).collect();
                let evidence = BaseFailure {
                    base: base.clone(),
                    failing: normalized_failures(&result.failures),
                };
                base_failure = Some(evidence.clone());
                result.base_failure = Some(evidence);
            }
            Err(error) => result
                .summary
                .push_str(&format!("; attribution incomplete: {error}")),
        }
    }
    // Leave the scratch checkout at the published tip, even after probes.
    git_output(&worktree, &["reset", "--hard", &tip])?;
    result.summary = format!("{branch} at {tip}: {}", result.summary);
    result.request = request.clone();
    if !affected.contains(&request.epic_id) {
        affected.push(request.epic_id.clone());
    }
    result.integration_epics = affected.clone();
    receipt.status = status_text(result.status).to_owned();
    receipt.detail = format!(
        "{}; {}",
        sweep_detail(&result),
        super::test_process_identity_receipt_note()
    );
    receipt.affected = affected;
    receipt.base_failure = base_failure;
    receipt.deferrals = 0;
    result.after_deferrals = prior_deferrals;
    // Keep the raw sweep log and the machine-readable fix queue together. A
    // passing sweep clears a prior report so a supervisor can never accept
    // stale proposals after a later green integration tip.
    let sweep_tasks = if result.status == SweepStatus::Failed {
        crate::factory_sweep_tasks::build_report(
            project_root,
            &result.log_path,
            status_text(result.status),
            &branch,
            &tip,
            &request.epic_id,
            &result.integration_epics,
        )
    } else {
        Ok(crate::factory_sweep_tasks::SweepTaskReport {
            schema_version: 1,
            status: status_text(result.status).to_owned(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            integration_branch: branch.clone(),
            integration_tip: tip.clone(),
            source_epic: request.epic_id.clone(),
            affected_epics: result.integration_epics.clone(),
            log_path: result.log_path.display().to_string(),
            failure_count: result.failures.len(),
            classes: Vec::new(),
            accepted_at: None,
        })
    };
    match sweep_tasks.and_then(|report| {
        crate::factory_sweep_tasks::write_report(&shared_cas, &report)
    }) {
        Ok(path) if result.status == SweepStatus::Failed && !result.failures.is_empty() => {
            result
                .summary
                .push_str(&format!("; fix proposals: {}", path.display()));
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "could not publish sweep task report");
            result.summary.push_str(&format!("; fix proposal report failed: {error}"));
        }
    }
    write_receipt(&receipt_path, &receipt)?;
    Ok(result)
}

fn validate_recovery_target_under_lock(
    project_root: &Path,
    request: &SweepRequest,
) -> Result<(), String> {
    let reference = format!("refs/heads/{}", request.target_branch);
    git_output(project_root, &["check-ref-format", &reference])?;
    let current = git_output(
        project_root,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )?;
    if current != request.commit {
        return Err(format!(
            "stale recovery event: {} currently points to {}, event target was {}",
            request.target_branch, current, request.commit
        ));
    }
    Ok(())
}

/// Resolve exactly one durable merge event for the registered session's
/// focused open epic. A current local ref must match the event's explicit
/// `target_tip`; legacy `commit` fallback is intentionally insufficient for
/// recovery because it does not prove the delivery ref that was merged.
pub(super) fn recovery_request_for_focus(
    project_root: &Path,
    cas_dir: &Path,
    session_name: &str,
    epic_id: &str,
) -> Result<SweepRequest, String> {
    let tasks = crate::store::open_task_store(cas_dir).map_err(|error| error.to_string())?;
    let task = tasks
        .get(epic_id)
        .map_err(|error| format!("recovery focus {epic_id} is not a known task: {error}"))?;
    if task.task_type != TaskType::Epic
        || matches!(task.status, TaskStatus::Closed | TaskStatus::Cancelled)
    {
        return Err(format!("recovery focus {epic_id} is not an open epic"));
    }
    let branch = epic_branch(&task)
        .ok_or_else(|| format!("recovery focus {epic_id} has no epic coordination branch"))?
        .to_owned();
    let reference = format!("refs/heads/{branch}");
    git_output(project_root, &["check-ref-format", &reference])?;
    let current = git_output(
        project_root,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )?;
    let latest_day = chrono::Utc::now().date_naive();
    let earliest_day = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch date");
    let mut candidates = Vec::new();
    for path in super::session_log_paths_between(cas_dir, earliest_day, latest_day) {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let complete_len = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        for line in String::from_utf8_lossy(&bytes[..complete_len]).lines() {
            let Ok(event) = serde_json::from_str::<MergeEvent>(line) else {
                continue;
            };
            if event.event.as_deref() != Some("worktree_merged")
                || event.factory_session.as_deref() != Some(session_name)
                || event.epic_id.as_deref() != Some(epic_id)
                || event.target_branch.as_deref() != Some(branch.as_str())
            {
                continue;
            }
            let Some(tip) = event
                .target_tip
                .as_deref()
                .map(str::trim)
                .filter(|tip| !tip.is_empty() && *tip != "none")
            else {
                continue;
            };
            candidates.push(tip.to_owned());
        }
    }
    let matching = candidates
        .iter()
        .filter(|tip| tip.as_str() == current)
        .count();
    if matching != 1 {
        let detail = if matching > 1 {
            format!("{} merge events match current tip {current}", matching)
        } else if candidates.is_empty() {
            "no complete authentic worktree_merged event matches the focused session, epic, and branch".to_owned()
        } else {
            format!(
                "recorded event tip {} is stale or mismatched with current ref {current}",
                candidates.last().expect("nonempty candidates")
            )
        };
        return Err(format!("recovery refused: {detail}"));
    }
    Ok(SweepRequest {
        epic_id: epic_id.to_owned(),
        target_branch: branch,
        commit: current,
    })
}

/// Resolve exactly one durable merge event for any open epic owned by the
/// registered session. This is the recovery path when session metadata points
/// at a stale or closed focus: the event itself, its canonical epic branch,
/// and the current branch tip must still agree before it can be trusted.
pub(super) fn recovery_request_for_any_open_epic(
    project_root: &Path,
    cas_dir: &Path,
    session_name: &str,
) -> Result<SweepRequest, String> {
    let tasks = crate::store::open_task_store(cas_dir).map_err(|error| error.to_string())?;
    let open_epics = open_epics(tasks.list(None).map_err(|error| error.to_string())?)
        .into_iter()
        .filter_map(|task| {
            let branch = epic_branch(&task)?.to_owned();
            Some((task.id, branch))
        })
        .collect::<Vec<_>>();
    let latest_day = chrono::Utc::now().date_naive();
    let earliest_day = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch date");
    let mut candidates = Vec::new();
    for path in super::session_log_paths_between(cas_dir, earliest_day, latest_day) {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let complete_len = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        for line in String::from_utf8_lossy(&bytes[..complete_len]).lines() {
            let Ok(event) = serde_json::from_str::<MergeEvent>(line) else {
                continue;
            };
            if event.event.as_deref() != Some("worktree_merged")
                || event.factory_session.as_deref() != Some(session_name)
            {
                continue;
            }
            let Some(epic_id) = event.epic_id.as_deref() else {
                continue;
            };
            let Some(target_branch) = event.target_branch.as_deref() else {
                continue;
            };
            let Some((_, branch)) = open_epics
                .iter()
                .find(|(id, branch)| id == epic_id && branch == target_branch)
            else {
                continue;
            };
            let Some(tip) = event
                .target_tip
                .as_deref()
                .map(str::trim)
                .filter(|tip| !tip.is_empty() && *tip != "none")
            else {
                continue;
            };
            let current = ref_tip(project_root, &format!("refs/heads/{branch}"));
            candidates.push((epic_id.to_owned(), branch.clone(), tip.to_owned(), current));
        }
    }
    let matching = candidates
        .iter()
        .filter(|(_, _, tip, current)| current.as_deref() == Some(tip.as_str()))
        .collect::<Vec<_>>();
    if matching.len() == 1 {
        let (epic_id, target_branch, commit, _) = matching[0];
        return Ok(SweepRequest {
            epic_id: epic_id.clone(),
            target_branch: target_branch.clone(),
            commit: commit.clone(),
        });
    }
    let detail = if matching.len() > 1 {
        format!(
            "{} merge events match current open-epic tips",
            matching.len()
        )
    } else if candidates.is_empty() {
        "no complete authentic worktree_merged event matches an open epic".to_owned()
    } else {
        "recorded open-epic event tip is stale or mismatched with its current ref".to_owned()
    };
    Err(format!("recovery refused: {detail}"))
}

/// A base-only recovery has no event-owned branch to validate. The rolling
/// integration path fetches and resolves the trunk before it constructs the
/// union, so this request only identifies the recovery mode in logs and
/// receipts. It names the trunk the same way (GH #954), falling back to
/// `main` only as a label when nothing resolves yet.
pub(super) fn base_only_recovery_request(project_root: &Path) -> Result<SweepRequest, String> {
    let target_branch =
        resolve_integration_trunk(project_root, configured_trunk(project_root).as_deref())
            .map(|(trunk, _)| trunk)
            .unwrap_or_else(|_| "main".to_owned());
    Ok(SweepRequest {
        epic_id: "base-only".to_owned(),
        target_branch,
        commit: "base-only".to_owned(),
    })
}

fn resolve_epic_tip(root: &Path, branch: &str) -> Result<String, String> {
    let local = ref_tip(root, &format!("refs/heads/{branch}"));
    let remote = ref_tip(root, &format!("refs/remotes/origin/{branch}"));
    match (local, remote) {
        (Some(local), Some(remote)) => {
            if git_output(root, &["merge-base", "--is-ancestor", &local, &remote]).is_ok() {
                Ok(remote)
            } else if git_output(root, &["merge-base", "--is-ancestor", &remote, &local]).is_ok() {
                Ok(local)
            } else {
                Err(format!(
                    "Local and remote {branch} diverged; reconcile the epic before integration"
                ))
            }
        }
        (Some(tip), None) | (None, Some(tip)) => Ok(tip),
        (None, None) => Err(format!("Open epic branch {branch} is missing")),
    }
}

fn normalized_failures(failures: &[String]) -> Vec<String> {
    failures
        .iter()
        .filter_map(|line| failure_identity(line))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn write_receipt(path: &Path, receipt: &IntegrationReceipt) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("receipt parent missing")?)
        .map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(receipt).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

/// Publish the successful sweep in the release gate's flat row-cache format.
/// The key is deliberately independent of the release version and linked
/// worktree path: assembly and the later gate use the same Git repository and
/// content, but not the same checkout directory.
fn write_sweep_row_receipt(
    project_root: &Path,
    shared_cas: &Path,
    request: &SweepRequest,
    row: &str,
) -> Result<(), String> {
    let worktree = prepare_merge_worktree(project_root, request)?;
    // The release gate's row-cache contract is Rust/Cargo-specific. A
    // package-manager sweep has its own durable integration receipt and must
    // not be turned into a setup failure by probing Cargo-only files/tools.
    if !worktree.join("Cargo.toml").is_file() {
        return Ok(());
    }
    let common_dir = git_output(
        &worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let checkout_identity = sha256_hex(format!("{common_dir}\n").as_bytes());
    let tree_ref = format!("{}^{{tree}}", request.commit);
    let input_hash = git_output(project_root, &["rev-parse", &tree_ref])?;
    let effective_zig = resolve_zig(&worktree);
    let environment = environment_fingerprint(effective_zig.as_deref());
    let toolchain = toolchain_fingerprint()?;
    let implementation = sha256_file(&worktree.join("scripts/release-gate.sh"))?;
    let key = row_cache_key(
        row,
        &checkout_identity,
        &input_hash,
        &environment,
        &toolchain,
        &implementation,
    );
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock before Unix epoch: {error}"))?
        .as_secs();
    let cache_dir = shared_cas.join(LOG_DIR).join(ROW_CACHE_DIR);
    fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let path = cache_dir.join(format!("{row}.{key}"));
    let temporary = cache_dir.join(format!(".{row}.{key}.tmp.{}", std::process::id()));
    let receipt = format!(
        "{key} {} {epoch} PASS {checkout_identity} {input_hash} {environment} {toolchain} {implementation}\n",
        request.commit
    );
    fs::write(&temporary, receipt).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

fn row_cache_key(
    row: &str,
    checkout_identity: &str,
    input_hash: &str,
    environment: &str,
    toolchain: &str,
    implementation: &str,
) -> String {
    sha256_hex(
        format!(
            "{ROW_CACHE_FORMAT}\n{row}\n{checkout_identity}\n{input_hash}\n{environment}\n{toolchain}\n{implementation}\n"
        )
        .as_bytes(),
    )
}

fn environment_fingerprint(effective_zig: Option<&Path>) -> String {
    let ignored = [
        "_",
        "SHLVL",
        "CAS_FACTORY_SESSION",
        "CAS_AGENT_ROLE",
        "CAS_AGENT_NAME",
        "CAS_SUPERVISOR_NAME",
        "CAS_AGENT_ID",
        "CAS_RELEASE_GATE_LOG_DIR",
        "CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE",
        "CAS_RELEASE_GATE_CACHE_DIR",
        "CAS_RELEASE_GATE_SWEEP_CACHE_DIR",
        "CAS_RELEASE_GATE_HOME_DIR",
    ];
    let mut values: std::collections::BTreeMap<String, String> = std::env::vars()
        .filter(|(name, _)| {
            !ignored.contains(&name.as_str()) && !name.starts_with("CAS_RELEASE_TRAIN_")
        })
        .collect();
    if let Some(zig) = effective_zig {
        values.insert("ZIG".to_owned(), zig.to_string_lossy().into_owned());
    }
    values
        .entry("CAS_INIT_TIMEOUT_SECS".to_owned())
        .or_insert_with(|| GATE_INIT_TIMEOUT_SECS.to_owned());
    let material = values
        .into_iter()
        .map(|(name, value)| format!("{name}={value}\n"))
        .collect::<String>();
    sha256_hex(material.as_bytes())
}

fn toolchain_fingerprint() -> Result<String, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let npm = std::env::var("NPM").unwrap_or_else(|_| "npm".to_owned());
    let output = Command::new("bash")
        .args([
            "-c",
            r#"set -o pipefail; { "$1" --version && "$1" nextest --version && rustc -Vv && node --version && "$2" --version; } 2>&1 | sha256sum | cut -d' ' -f1"#,
            "row-cache",
            &cargo,
            &npm,
        ])
        .output()
        .map_err(|error| format!("run toolchain fingerprint: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "toolchain fingerprint command failed: {}",
            first_output_line(&output.stderr)
        ));
    }
    let digest = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(digest)
    } else {
        Err("toolchain fingerprint was not a SHA-256 digest".to_owned())
    }
}

/// The final prefix is known red. Re-run only failing targets on earlier tips;
/// the first transition from green to red identifies the introducing merge.
fn introducing_epic(
    prefixes: &[String],
    probe: &mut impl FnMut(&str) -> Result<bool, String>,
) -> Result<Option<usize>, String> {
    for index in (0..prefixes.len().saturating_sub(1)).rev() {
        if probe(&prefixes[index])? {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

fn failing_filter(failures: &[String]) -> Option<String> {
    let names: Vec<String> = failures
        .iter()
        .filter_map(|line| failure_target(line))
        .map(|name| {
            format!("test(/^{}$/)", regex::escape(&name).replace('/', "\\/"))
        })
        .collect();
    (!names.is_empty()).then(|| names.join(" | "))
}

fn failure_target(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(']')?;
    let mut fields = rest.split_whitespace();
    let first = fields.next()?;
    if first.starts_with('(') {
        fields.next()?;
    }
    let name = fields.collect::<Vec<_>>().join(" ");
    (!name.is_empty()).then_some(name)
}

fn failure_identity(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(']')?;
    let mut fields = rest.split_whitespace();
    let mut binary = fields.next()?;
    if binary.starts_with('(') {
        binary = fields.next()?;
    }
    let test_name = fields.collect::<Vec<_>>().join(" ");
    (!test_name.is_empty()).then(|| format!("{binary} {test_name}"))
}

/// Fan out via the existing durable supervisor notification + prompt outbox,
/// explicitly selecting each epic's owner rather than the daemon's session.
pub(super) fn record_result(cas_dir: &Path, result: &SweepResult) {
    if result.status == SweepStatus::Superseded {
        return;
    }
    let Ok(tasks) = crate::store::open_task_store(cas_dir) else {
        return;
    };
    let mut owners = std::collections::BTreeSet::new();
    for epic_id in &result.integration_epics {
        let Ok(mut task) = tasks.get(epic_id) else {
            continue;
        };
        let note = format!(
            "Rolling integration {}: {}. Log: {}",
            status_text(result.status),
            sweep_detail(result),
            result.log_path.display()
        );
        if !task.notes.contains(&note) {
            task.notes = format!(
                "{}\n\n[{}] 📝 PROGRESS {}",
                task.notes,
                chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                note
            );
            if let Err(error) = tasks.update(&task) {
                tracing::error!(%epic_id, %error, "integration note write failed");
            }
        }
        if let Some(owner) = task.epic_verification_owner.or(task.assignee) {
            owners.insert(owner);
        }
    }
    // GH #1006: a deferral is recorded on the epic but relays nothing; the
    // run that eventually goes ahead reports once, pass or fail.
    if result.status == SweepStatus::Deferred
        || (result.status == SweepStatus::Passed && result.after_deferrals == 0)
    {
        return;
    }
    for owner in owners {
        if let Err(error) = notify_owner(cas_dir, &owner, result) {
            tracing::error!(%owner, %error, "integration supervisor notification failed");
        }
    }
}

fn notify_owner(cas_dir: &Path, owner: &str, result: &SweepResult) -> Result<(), String> {
    use cas_store::{NotificationPriority, NotifyIdempotentResult, QueueOrigin};
    let agents = crate::store::open_agent_store(cas_dir).map_err(|error| error.to_string())?;
    let agent = agents.get(owner).map_err(|error| error.to_string())?;
    if agent.role != cas_types::AgentRole::Supervisor {
        return Err(format!("epic owner {owner} is not a supervisor"));
    }
    let queue =
        crate::store::open_supervisor_queue_store(cas_dir).map_err(|error| error.to_string())?;
    let after_deferrals = match result.after_deferrals {
        0 => String::new(),
        1 => " after 1 deferral".to_owned(),
        count => format!(" after {count} deferrals"),
    };
    let detail = format!(
        "Rolling integration {}{after_deferrals}. Epics: {}. {}. Log: {}",
        status_text(result.status),
        result.integration_epics.join(", "),
        sweep_detail(result),
        result.log_path.display()
    );
    let kind = if result.status == SweepStatus::Passed {
        "sweep_passed"
    } else {
        "sweep_failed"
    };
    let key = format!(
        "integration:{owner}:{}",
        result
            .base_failure
            .as_ref()
            .map(|failure| {
                format!(
                    "base-only:{}:{}",
                    failure.base,
                    sha256_hex(failure.failing.join("\n").as_bytes())
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "{}:{}:{:?}",
                    result.request.epic_id, result.request.commit, result.status
                )
            })
    );
    let payload = serde_json::json!({ "kind": kind, "detail": detail,
        "epics": result.integration_epics, "factory_session": agent.factory_session })
    .to_string();
    let notification = queue
        .notify_idempotent(owner, kind, &payload, NotificationPriority::High, &key)
        .map_err(|error| error.to_string())?;
    let id = match notification {
        NotifyIdempotentResult::Created(id) => id,
        NotifyIdempotentResult::AlreadyExists {
            prompt_delivered: true,
            ..
        } => return Ok(()),
        NotifyIdempotentResult::AlreadyExists { id, .. } => id,
    };
    let session = agent
        .factory_session
        .as_deref()
        .ok_or("epic supervisor has no factory session")?;
    let body = format!(
        "<worker-attention kind=\"{kind}\" worker=\"supervisor\" notification_id=\"{id}\">\n{detail}\n</worker-attention>"
    );
    let prompts =
        crate::store::open_prompt_queue_store(cas_dir).map_err(|error| error.to_string())?;
    prompts
        .enqueue_idempotent(
            &format!("lifecycle-wake:integration:{id}"),
            "supervisor",
            &body,
            Some(session),
            Some(if kind == "sweep_passed" {
                "Rolling integration ran after deferral"
            } else {
                "Rolling integration requires attention"
            }),
            Some(NotificationPriority::High),
            &format!("integration-outbox:{id}"),
            Some(&QueueOrigin::Daemon),
        )
        .map_err(|error| error.to_string())?;
    queue
        .mark_prompt_delivered(id)
        .map_err(|error| error.to_string())?;
    super::super::delivery::wake_daemon_after_enqueue(cas_dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(path: &Path, args: &[&str]) -> String {
        git_output(path, args).unwrap()
    }
    fn fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-b", "main"]);
        git(temp.path(), &["config", "user.name", "Integration Test"]);
        git(
            temp.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(temp.path(), &["config", "commit.gpgsign", "false"]);
        fs::create_dir_all(temp.path().join("scripts")).unwrap();
        fs::write(temp.path().join("scripts/release-gate.sh"), "#!/bin/sh\n").unwrap();
        // The sweep fixture represents a Rust target; declare its manifest so
        // runner resolution exercises the same Cargo boundary as production.
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"integration-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(temp.path().join("shared"), "base\n").unwrap();
        git(temp.path(), &["add", "."]);
        git(temp.path(), &["commit", "-m", "base"]);
        temp
    }
    fn package_fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-b", "main"]);
        git(temp.path(), &["config", "user.name", "Integration Test"]);
        git(
            temp.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(temp.path(), &["config", "commit.gpgsign", "false"]);
        fs::write(
            temp.path().join("package.json"),
            r#"{"name":"integration-recovery-fixture","version":"1.0.0","scripts":{"test":"node smoke-test.js"}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("package-lock.json"),
            r#"{"name":"integration-recovery-fixture","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"integration-recovery-fixture","version":"1.0.0"}}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("smoke-test.js"),
            "const fs = require('node:fs');\nif (!fs.existsSync('recovery-feature.txt')) process.exit(1);\n",
        )
        .unwrap();
        git(temp.path(), &["add", "."]);
        git(temp.path(), &["commit", "-m", "base"]);
        temp
    }
    fn epic(path: &Path, id: &str, file: &str, content: &str) -> EpicTip {
        let branch = format!("epic/{id}");
        git(path, &["checkout", "-b", &branch, "main"]);
        fs::write(path.join(file), content).unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-m", id]);
        EpicTip {
            id: id.to_owned(),
            branch,
            tip: git(path, &["rev-parse", "HEAD"]),
            owner: None,
        }
    }
    #[test]
    fn conflicting_epics_report_pair_and_files_without_moving_sources() {
        let repo = fixture();
        let first = epic(repo.path(), "a", "shared", "first\n");
        let second = epic(repo.path(), "b", "shared", "second\n");
        git(repo.path(), &["checkout", "--detach", "main"]);
        let result = assemble(
            repo.path(),
            "main",
            "main",
            &[first.clone(), second.clone()],
        )
        .unwrap();
        let Assembly::Conflict { detail, affected } = result else {
            panic!("expected conflict");
        };
        assert_eq!(affected, ["a", "b"]);
        assert!(detail.contains("shared"), "{detail}");
        assert_eq!(git(repo.path(), &["rev-parse", &first.branch]), first.tip);
        assert_eq!(git(repo.path(), &["rev-parse", &second.branch]), second.tip);
        assert!(!repo.path().join(".git/MERGE_HEAD").exists());
    }
    #[test]
    fn clean_union_contains_both_epics_and_main() {
        let repo = fixture();
        let first = epic(repo.path(), "a", "a", "first\n");
        let second = epic(repo.path(), "b", "b", "second\n");
        git(repo.path(), &["checkout", "--detach", "main"]);
        let Assembly::Clean { tip, prefixes } = assemble(
            repo.path(),
            "main",
            "main",
            &[first.clone(), second.clone()],
        )
        .unwrap() else {
            panic!("expected clean union");
        };
        assert_eq!(prefixes.len(), 3);
        for input in [&first.tip, &second.tip, "main"] {
            git(repo.path(), &["merge-base", "--is-ancestor", input, &tip]);
        }
        assert_eq!(
            fs::read_to_string(repo.path().join("a")).unwrap(),
            "first\n"
        );
        assert_eq!(
            fs::read_to_string(repo.path().join("b")).unwrap(),
            "second\n"
        );
    }
    #[test]
    fn failing_union_is_attributed_by_executing_test_on_premerge_tip() {
        let repo = fixture();
        let first = epic(repo.path(), "a", "a", "first\n");
        let second = epic(repo.path(), "b", "b", "second\n");
        git(repo.path(), &["checkout", "--detach", "main"]);
        let Assembly::Clean { prefixes, .. } =
            assemble(repo.path(), "main", "main", &[first, second]).unwrap()
        else {
            panic!("expected clean Git merge");
        };
        let mut check = |tip: &str| -> Result<bool, String> {
            git_output(repo.path(), &["reset", "--hard", tip])?;
            Ok(Command::new("sh")
                .current_dir(repo.path())
                .args(["-c", "test ! -f a || test ! -f b"])
                .status()
                .unwrap()
                .success())
        };
        assert!(!check(prefixes.last().unwrap()).unwrap());
        assert_eq!(introducing_epic(&prefixes, &mut check).unwrap(), Some(1));
        assert_eq!(
            introducing_epic(&prefixes, &mut |_| Ok(false)).unwrap(),
            None
        );
        assert!(introducing_epic(&prefixes, &mut |_| Err("timeout".to_owned())).is_err());
    }
    #[test]
    fn reporting_fans_out_to_owning_sessions_once_and_notes_both_epics() {
        let temp = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        for id in ["a", "b"] {
            let mut agent = cas_types::Agent::new(format!("owner-{id}"), format!("lead-{id}"));
            agent.role = cas_types::AgentRole::Supervisor;
            agent.factory_session = Some(format!("session-{id}"));
            agents.register(&agent).unwrap();
            let mut task = Task::new(id.to_owned(), id.to_owned());
            task.task_type = TaskType::Epic;
            task.epic_verification_owner = Some(agent.id);
            tasks.add(&task).unwrap();
        }
        let result = SweepResult {
            request: SweepRequest {
                epic_id: "b".to_owned(),
                target_branch: "epic/b".to_owned(),
                commit: "abc".to_owned(),
            },
            status: SweepStatus::Failed,
            log_path: temp.path().join("sweep.log"),
            summary: "Conflict a and b: shared".to_owned(),
            failures: Vec::new(),
            integration_epics: vec!["a".to_owned(), "b".to_owned()],
            base_failure: None,
            after_deferrals: 0,
        };
        record_result(&cas_dir, &result);
        record_result(&cas_dir, &result);
        let rows = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(10)
            .unwrap();
        assert_eq!(rows.len(), 2);
        for id in ["a", "b"] {
            assert_eq!(
                tasks
                    .get(id)
                    .unwrap()
                    .notes
                    .matches("Conflict a and b: shared")
                    .count(),
                1
            );
            let session = format!("session-{id}");
            let row = rows
                .iter()
                .find(|row| row.factory_session.as_deref() == Some(session.as_str()))
                .unwrap();
            assert_eq!(row.target, "supervisor");
            assert!(crate::prompt_revalidation::is_supervisor_wake_envelope(
                &row.prompt
            ));
        }
    }
    #[test]
    fn open_epics_are_sorted_by_creation_and_closed_epics_are_removed() {
        let mut older = Task::new("a".to_owned(), "older".to_owned());
        older.task_type = TaskType::Epic;
        let mut newer = Task::new("b".to_owned(), "newer".to_owned());
        newer.task_type = TaskType::Epic;
        newer.created_at = older.created_at + chrono::Duration::seconds(1);
        let mut closed = older.clone();
        closed.status = TaskStatus::Closed;
        let normal = Task::new("normal".to_owned(), "normal".to_owned());
        let tasks = open_epics(vec![newer, closed, normal, older]);
        assert_eq!(
            tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn live_open_epics_include_ownerless_origin_branch_and_exclude_closed_epic() {
        let repo = fixture();
        let origin_only = epic(repo.path(), "origin-open", "origin-open", "open\n");
        let closed_branch = epic(repo.path(), "closed", "closed", "closed\n");
        git(repo.path(), &["checkout", "--detach", "main"]);
        git(
            repo.path(),
            &[
                "update-ref",
                &format!("refs/remotes/origin/{}", origin_only.branch),
                &origin_only.tip,
            ],
        );
        git(repo.path(), &["branch", "-D", &origin_only.branch]);

        let mut open = Task::new("origin-open".to_owned(), "origin-open".to_owned());
        open.task_type = TaskType::Epic;
        open.branch = Some(origin_only.branch);
        open.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:fixture".to_owned(),
            target_branch: "main".to_owned(),
        });
        let mut closed = Task::new("closed".to_owned(), "closed".to_owned());
        closed.task_type = TaskType::Epic;
        closed.status = TaskStatus::Closed;
        closed.branch = Some(closed_branch.branch);

        let (epics, already_integrated) =
            live_open_epics(repo.path(), "main", vec![closed, open]).unwrap();
        assert_eq!(epics.len(), 1);
        assert_eq!(epics[0].id, "origin-open");
        assert_eq!(epics[0].owner, None);
        assert!(already_integrated.is_empty());
    }

    #[test]
    fn stale_merged_epic_is_reported_integrated_and_base_conflict_names_main() {
        let repo = fixture();
        let stale = epic(repo.path(), "cas-stale", "stale", "stale\n");
        let initial = git(repo.path(), &["rev-parse", "main"]);
        git(repo.path(), &["checkout", "main"]);
        git(repo.path(), &["merge", "--ff-only", &stale.branch]);

        git(repo.path(), &["checkout", "-b", "epic/cas-live", &initial]);
        fs::write(repo.path().join("shared"), "live\n").unwrap();
        git(repo.path(), &["add", "shared"]);
        git(repo.path(), &["commit", "-m", "cas-live"]);
        let live = EpicTip {
            id: "cas-live".to_owned(),
            branch: "epic/cas-live".to_owned(),
            tip: git(repo.path(), &["rev-parse", "HEAD"]),
            owner: None,
        };

        git(repo.path(), &["checkout", "main"]);
        fs::write(repo.path().join("shared"), "main\n").unwrap();
        git(repo.path(), &["add", "shared"]);
        git(repo.path(), &["commit", "-m", "main-rewrite"]);

        let mut stale_task = Task::new("cas-stale".to_owned(), "stale".to_owned());
        stale_task.task_type = TaskType::Epic;
        stale_task.branch = Some(stale.branch.clone());
        let mut live_task = Task::new("cas-live".to_owned(), "live".to_owned());
        live_task.task_type = TaskType::Epic;
        live_task.branch = Some(live.branch.clone());
        let (epics, already_integrated) =
            live_open_epics(repo.path(), "main", vec![stale_task, live_task]).unwrap();
        assert_eq!(
            epics
                .iter()
                .map(|epic| epic.id.as_str())
                .collect::<Vec<_>>(),
            ["cas-live"]
        );
        assert_eq!(
            already_integrated
                .iter()
                .map(|epic| epic.id.as_str())
                .collect::<Vec<_>>(),
            ["cas-stale"]
        );

        git(repo.path(), &["checkout", "--detach", "main"]);
        let Assembly::Conflict { detail, affected } =
            assemble(repo.path(), "main", "origin/main", &epics).unwrap()
        else {
            panic!("expected the live epic to conflict with main");
        };
        assert!(
            detail.contains("Conflict adding cas-live against origin/main"),
            "{detail}"
        );
        assert!(!detail.contains("cas-stale"), "{detail}");
        assert_eq!(affected, ["cas-live"]);
    }

    #[test]
    fn base_only_failure_relay_is_suppressed_until_evidence_changes() {
        let temp = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        let mut agent = cas_types::Agent::new("owner".to_owned(), "lead".to_owned());
        agent.role = cas_types::AgentRole::Supervisor;
        agent.factory_session = Some("session".to_owned());
        let owner = agent.id.clone();
        agents.register(&agent).unwrap();
        let mut task = Task::new("epic".to_owned(), "epic".to_owned());
        task.task_type = TaskType::Epic;
        task.epic_verification_owner = Some(owner);
        tasks.add(&task).unwrap();

        let result = SweepResult {
            request: SweepRequest {
                epic_id: "epic".to_owned(),
                target_branch: "epic/epic".to_owned(),
                commit: "dispatch-1".to_owned(),
            },
            status: SweepStatus::Failed,
            log_path: temp.path().join("sweep.log"),
            summary: "failing targets also fail on origin/main".to_owned(),
            failures: vec!["FAIL [0.1s] (1/1) cas::fixture test_base".to_owned()],
            integration_epics: vec!["epic".to_owned()],
            base_failure: Some(BaseFailure {
                base: "base-1".to_owned(),
                failing: vec!["cas::fixture test_base".to_owned()],
            }),
            after_deferrals: 0,
        };
        assert_eq!(
            normalized_failures(&result.failures),
            vec!["cas::fixture test_base"]
        );
        record_result(&cas_dir, &result);
        let mut retry = result;
        retry.request.commit = "dispatch-2".to_owned();
        retry.failures = vec!["FAIL [0.8s] (2/2) cas::fixture test_base".to_owned()];
        assert_eq!(
            normalized_failures(&retry.failures),
            vec!["cas::fixture test_base"]
        );
        record_result(&cas_dir, &retry);
        assert_eq!(
            crate::store::open_prompt_queue_store(&cas_dir)
                .unwrap()
                .peek_all(10)
                .unwrap()
                .len(),
            1
        );

        retry.base_failure.as_mut().unwrap().base = "base-2".to_owned();
        record_result(&cas_dir, &retry);
        assert_eq!(
            crate::store::open_prompt_queue_store(&cas_dir)
                .unwrap()
                .peek_all(10)
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn failure_filter_accepts_nextest_progress_and_exact_test_names() {
        assert_eq!(
            failing_filter(&["FAIL [0.1s] (2/3) cas module::broken".into()]),
            Some("test(/^module::broken$/)".into())
        );
        assert_eq!(
            failing_filter(&["FAIL [0.1s] cas module::broken".into()]),
            Some("test(/^module::broken$/)".into())
        );
        assert_eq!(failing_filter(&["error: failed to compile".into()]), None);
    }

    #[test]
    fn runtime_union_sweep_receipt_and_close_reopen_with_stub_cargo() {
        let repo = fixture();
        let first = epic(repo.path(), "cas-0081", "a", "one");
        let second = epic(repo.path(), "cas-ed91a", "b", "two");
        git(repo.path(), &["checkout", "--detach", "main"]);
        git(
            repo.path(),
            &["remote", "add", "origin", repo.path().to_str().unwrap()],
        );
        let stub = repo.path().join("cargo-stub.sh");
        crate::test_paths::warm_stub(
            &stub,
            r#"#!/bin/sh
if [ -f a ] && [ -f b ]; then
  echo 'FAIL [0.1s] fixture union_test'
  echo 'Summary: 1 failed'
  exit 1
fi
echo 'Summary: 1 passed'
"#,
        );
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CARGO", stub.to_str().unwrap()),
            ("CAS_FACTORY_BUILD_GUARD", "off"),
        ]);
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        for epic in [&first, &second] {
            let mut task = Task::new(epic.id.clone(), epic.id.clone());
            task.task_type = TaskType::Epic;
            task.branch = Some(epic.branch.clone());
            task.deliverables.work_target = Some(cas_types::WorkTarget {
                repo_selector: "project:fixture".to_owned(),
                target_branch: "main".to_owned(),
            });
            tasks.add(&task).unwrap();
        }
        let mut settings = SweepSettings::from(&FactoryConfig::default());
        settings.nice_cargo = false;
        let request = SweepRequest {
            epic_id: "cas-ed91a".into(),
            target_branch: second.branch,
            commit: second.tip,
        };
        for (state, expected) in [
            (TaskStatus::Open, SweepStatus::Failed),
            (TaskStatus::Closed, SweepStatus::Passed),
            (TaskStatus::Open, SweepStatus::Failed),
        ] {
            let mut task = tasks.get("cas-ed91a").unwrap();
            task.status = state;
            tasks.update(&task).unwrap();
            let result = execute(
                repo.path(),
                &cas_dir,
                request.clone(),
                settings.clone(),
                Arc::new(AtomicBool::new(false)),
                false,
            );
            assert_eq!(result.status, expected, "{}", result.summary);
            let receipt: IntegrationReceipt = serde_json::from_slice(
                &fs::read(cas_dir.join(LOG_DIR).join("integration.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(receipt.status, status_text(expected));
            let row_cache = cas_dir.join(LOG_DIR).join(ROW_CACHE_DIR);
            let row_receipts = fs::read_dir(&row_cache)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .filter(|entry| entry.file_name().to_string_lossy().starts_with("nextest."))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if expected == SweepStatus::Passed {
                assert_eq!(row_receipts.len(), 1);
                let fields = fs::read_to_string(row_receipts[0].path()).unwrap();
                let fields = fields.split_whitespace().collect::<Vec<_>>();
                assert_eq!(fields.len(), 9);
                assert_eq!(fields[1], receipt.tip.as_deref().unwrap());
                assert_eq!(fields[3], "PASS");
            }
            if expected == SweepStatus::Failed {
                assert!(
                    result.summary.contains("introduced by cas-ed91a"),
                    "{}",
                    result.summary
                );
                assert_eq!(result.integration_epics, ["cas-0081", "cas-ed91a"]);
                assert_eq!(
                    receipt
                        .epics
                        .iter()
                        .map(|epic| epic.id.as_str())
                        .collect::<Vec<_>>(),
                    ["cas-0081", "cas-ed91a"]
                );
            } else {
                assert_eq!(receipt.epics.len(), 1);
            }
            assert!(receipt.tip.is_some());
            let tip = receipt.tip.as_deref().unwrap();
            assert_ne!(receipt.base, tip);
            git(
                repo.path(),
                &["merge-base", "--is-ancestor", &receipt.base, tip],
            );
        }
    }

    /// GH #1006: a deferred rolling integration relays nothing when it
    /// defers. The run that finally goes ahead sends exactly one relay naming
    /// its result and tip, and a later ordinary pass sends none.
    #[test]
    fn deferred_integration_reports_once_when_it_finally_runs() {
        let repo = fixture();
        let only = epic(repo.path(), "cas-45de", "a", "one");
        git(repo.path(), &["checkout", "--detach", "main"]);
        git(
            repo.path(),
            &["remote", "add", "origin", repo.path().to_str().unwrap()],
        );
        let stub = repo.path().join("cargo-stub.sh");
        crate::test_paths::warm_stub(&stub, "#!/bin/sh\necho 'Summary: 1 passed'\n");
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let mut owner = cas_types::Agent::new("owner-45de".to_owned(), "lead".to_owned());
        owner.role = cas_types::AgentRole::Supervisor;
        owner.factory_session = Some("session-45de".to_owned());
        agents.register(&owner).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        let mut task = Task::new(only.id.clone(), only.id.clone());
        task.task_type = TaskType::Epic;
        task.branch = Some(only.branch.clone());
        task.epic_verification_owner = Some(owner.id.clone());
        task.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:fixture".to_owned(),
            target_branch: "main".to_owned(),
        });
        tasks.add(&task).unwrap();
        let request = SweepRequest {
            epic_id: only.id.clone(),
            target_branch: only.branch.clone(),
            commit: only.tip.clone(),
        };
        let receipt = || -> IntegrationReceipt {
            serde_json::from_slice(
                &fs::read(cas_dir.join(LOG_DIR).join("integration.json")).unwrap(),
            )
            .unwrap()
        };
        let relays = || {
            crate::store::open_prompt_queue_store(&cas_dir)
                .unwrap()
                .peek_all(20)
                .unwrap()
        };
        let run = |settings: &SweepSettings| {
            let result = execute(
                repo.path(),
                &cas_dir,
                request.clone(),
                settings.clone(),
                Arc::new(AtomicBool::new(false)),
                false,
            );
            record_result(&cas_dir, &result);
            result
        };

        // Two deferrals: the build guard is live and refuses every builder.
        {
            let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[
                ("CARGO", Some(stub.to_str().unwrap())),
                ("CAS_FACTORY_BUILD_GUARD", None),
            ]);
            let mut refused = SweepSettings::from(&FactoryConfig::default());
            refused.nice_cargo = false;
            refused.max_concurrent_builders = 0;
            for expected in [1, 2] {
                let result = run(&refused);
                assert_eq!(result.status, SweepStatus::Deferred, "{}", result.summary);
                assert_eq!(receipt().deferrals, expected);
            }
        }
        assert!(
            relays().is_empty(),
            "a deferral must not relay: {:?}",
            relays()
        );

        // The run that finally goes ahead reports once.
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CARGO", stub.to_str().unwrap()),
            ("CAS_FACTORY_BUILD_GUARD", "off"),
        ]);
        let mut settings = SweepSettings::from(&FactoryConfig::default());
        settings.nice_cargo = false;
        let result = run(&settings);
        assert_eq!(result.status, SweepStatus::Passed, "{}", result.summary);
        assert_eq!(result.after_deferrals, 2);
        let finished = receipt();
        assert_eq!(finished.deferrals, 0);
        let tip = finished.tip.clone().expect("published tip");
        let rows = relays();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].target, "supervisor");
        assert_eq!(rows[0].factory_session.as_deref(), Some("session-45de"));
        assert!(
            rows[0]
                .prompt
                .contains("Rolling integration PASSED after 2 deferrals")
                && rows[0].prompt.contains(&tip),
            "{}",
            rows[0].prompt
        );
        assert!(crate::prompt_revalidation::is_supervisor_wake_envelope(
            &rows[0].prompt
        ));

        // An ordinary green run afterwards stays quiet.
        let again = run(&settings);
        assert_eq!(again.status, SweepStatus::Passed, "{}", again.summary);
        assert_eq!(again.after_deferrals, 0);
        assert_eq!(relays().len(), 1);
    }

    /// GH #1006: the deferral count survives the daemon (it lives in the
    /// receipt), and a legacy DEFERRED receipt without a count is one.
    #[test]
    fn outstanding_deferrals_read_from_the_previous_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("integration.json");
        let write = |status: &str, deferrals: Option<u32>| {
            let mut value = serde_json::json!({
                "base": "b", "epics": [], "tip": null, "status": status,
                "detail": "", "affected": []
            });
            if let Some(count) = deferrals {
                value["deferrals"] = count.into();
            }
            fs::write(&path, value.to_string()).unwrap();
        };
        assert_eq!(outstanding_deferrals(&path), 0, "no receipt yet");
        write("DEFERRED", None);
        assert_eq!(outstanding_deferrals(&path), 1);
        write("DEFERRED", Some(3));
        assert_eq!(outstanding_deferrals(&path), 3);
        write("RUNNING", Some(3));
        assert_eq!(
            outstanding_deferrals(&path),
            3,
            "an interrupted run still owes a report"
        );
        write("PASSED", Some(3));
        assert_eq!(outstanding_deferrals(&path), 0);
    }

    #[test]
    fn stale_recovery_refuses_under_shared_lock_without_invalidating_receipt() {
        let repo = fixture();
        let initial = epic(repo.path(), "cas-stale1", "feature", "initial\n");
        git(repo.path(), &["checkout", &initial.branch]);
        fs::write(repo.path().join("feature"), "advanced\n").unwrap();
        git(repo.path(), &["add", "feature"]);
        git(
            repo.path(),
            &["commit", "-m", "advance epic after recorded event"],
        );
        let current = git(repo.path(), &["rev-parse", "HEAD"]);
        git(repo.path(), &["checkout", "--detach", "main"]);
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        let mut task = Task::new(initial.id.clone(), initial.id.clone());
        task.task_type = TaskType::Epic;
        task.branch = Some(initial.branch.clone());
        tasks.add(&task).unwrap();

        let log_path = super::super::factory_session_log_path_for_date(
            &cas_dir,
            chrono::Utc::now().date_naive(),
        );
        fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        fs::write(
            &log_path,
            format!(
                "{{\"event\":\"worktree_merged\",\"factory_session\":\"s1\",\"epic_id\":\"{}\",\"target_branch\":\"{}\",\"target_tip\":\"{}\"}}\n",
                initial.id, initial.branch, initial.tip
            ),
        )
        .unwrap();
        let error =
            recovery_request_for_focus(repo.path(), &cas_dir, "s1", &initial.id).unwrap_err();
        assert!(error.contains("stale or mismatched"), "{error}");

        let receipt_path = cas_dir.join(LOG_DIR).join("integration.json");
        fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
        fs::write(&receipt_path, "prior-receipt-must-remain-byte-identical").unwrap();
        let mut settings = SweepSettings::from(&FactoryConfig::default());
        settings.timeout = Duration::from_secs(3);
        let result = execute(
            repo.path(),
            &cas_dir,
            SweepRequest {
                epic_id: initial.id,
                target_branch: initial.branch.clone(),
                commit: initial.tip,
            },
            settings,
            Arc::new(AtomicBool::new(false)),
            true,
        );
        assert_eq!(result.status, SweepStatus::SetupFailed);
        assert!(
            result.summary.contains("stale recovery event"),
            "{}",
            result.summary
        );
        assert_eq!(
            fs::read_to_string(&receipt_path).unwrap(),
            "prior-receipt-must-remain-byte-identical"
        );
        assert_eq!(git(repo.path(), &["rev-parse", &initial.branch]), current);
        let common = PathBuf::from(git(
            repo.path(),
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ));
        let project = common
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        let integration_ref = format!("refs/heads/integration/{}", sanitize_component(&project));
        assert!(git_output(repo.path(), &["rev-parse", "--verify", &integration_ref]).is_err());
    }

    #[test]
    fn ordinary_daemon_sweep_still_accepts_merge_event_behind_current_ref() {
        let repo = fixture();
        let initial = epic(repo.path(), "cas-normal1", "feature", "initial\n");
        git(repo.path(), &["checkout", &initial.branch]);
        fs::write(repo.path().join("feature"), "advanced\n").unwrap();
        git(repo.path(), &["add", "feature"]);
        git(repo.path(), &["commit", "-m", "advance epic after event"]);
        let current = git(repo.path(), &["rev-parse", "HEAD"]);
        git(repo.path(), &["checkout", "--detach", "main"]);
        git(
            repo.path(),
            &["remote", "add", "origin", repo.path().to_str().unwrap()],
        );
        let stub = repo.path().join("cargo-stub.sh");
        crate::test_paths::warm_stub(&stub, "#!/bin/sh\necho 'Summary: 1 passed'\n");
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CARGO", stub.to_str().unwrap()),
            ("CAS_FACTORY_BUILD_GUARD", "off"),
        ]);
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        let mut task = Task::new(initial.id.clone(), initial.id.clone());
        task.task_type = TaskType::Epic;
        task.branch = Some(initial.branch.clone());
        tasks.add(&task).unwrap();
        let mut settings = SweepSettings::from(&FactoryConfig::default());
        settings.nice_cargo = false;

        let result = execute(
            repo.path(),
            &cas_dir,
            SweepRequest {
                epic_id: initial.id,
                target_branch: initial.branch,
                commit: initial.tip,
            },
            settings,
            Arc::new(AtomicBool::new(false)),
            false,
        );

        assert_eq!(result.status, SweepStatus::Passed, "{}", result.summary);
        let receipt: IntegrationReceipt = serde_json::from_slice(
            &fs::read(cas_dir.join(LOG_DIR).join("integration.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt.status, "PASSED");
        assert_eq!(receipt.epics[0].tip, current);
    }

    #[test]
    fn recovery_runs_the_recorded_tip_through_the_production_npm_runner() {
        let _env =
            crate::test_support::TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_SESSION", None)]);
        let repo = package_fixture();
        git(repo.path(), &["checkout", "-b", "epic/cas-recov1", "main"]);
        fs::write(repo.path().join("recovery-feature.txt"), "merged\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "merged feature"]);
        let tip = git(repo.path(), &["rev-parse", "HEAD"]);
        git(repo.path(), &["checkout", "--detach", "main"]);
        git(
            repo.path(),
            &["remote", "add", "origin", repo.path().to_str().unwrap()],
        );
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        let mut task = Task::new("cas-recov1".to_owned(), "recovery epic".to_owned());
        task.task_type = TaskType::Epic;
        task.branch = Some("epic/cas-recov1".to_owned());
        tasks.add(&task).unwrap();
        let log_path = super::super::factory_session_log_path_for_date(
            &cas_dir,
            chrono::Utc::now().date_naive(),
        );
        fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        fs::write(
            &log_path,
            format!(
                "{{\"event\":\"worktree_merged\",\"factory_session\":\"recovery-session\",\"epic_id\":\"cas-recov1\",\"target_branch\":\"epic/cas-recov1\",\"target_tip\":\"{tip}\"}}\n"
            ),
        )
        .unwrap();

        let summary = crate::ui::factory::daemon::FactoryDaemon::recover_focused_integration(
            repo.path(),
            &cas_dir,
            "recovery-session",
            "cas-recov1",
            &FactoryConfig::default(),
        )
        .unwrap();

        assert!(summary.starts_with("PASSED:"), "{summary}");
        let receipt: IntegrationReceipt = serde_json::from_slice(
            &fs::read(cas_dir.join(LOG_DIR).join("integration.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt.status, "PASSED");
        assert_eq!(receipt.epics.len(), 1);
        assert_eq!(receipt.epics[0].tip, tip);
        let run_log = fs::read_dir(cas_dir.join(LOG_DIR))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
            .find_map(|path| {
                let contents = fs::read_to_string(path).ok()?;
                contents.contains("sweep: npm test").then_some(contents)
            })
            .expect("the production integration path must record its npm runner log");
        assert!(run_log.contains("sweep: npm test"), "{run_log}");
        assert!(run_log.contains("node smoke-test.js"), "{run_log}");
    }

    #[test]
    fn recovery_refuses_foreign_session_wrong_branch_and_ambiguous_events() {
        let repo = fixture();
        let epic = epic(repo.path(), "cas-ambig1", "feature", "merged\n");
        git(repo.path(), &["checkout", "--detach", "main"]);
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        let mut task = Task::new(epic.id.clone(), epic.id.clone());
        task.task_type = TaskType::Epic;
        task.branch = Some(epic.branch.clone());
        tasks.add(&task).unwrap();
        let log_path = super::super::factory_session_log_path_for_date(
            &cas_dir,
            chrono::Utc::now().date_naive(),
        );
        fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        let make_event = |session: &str, branch: &str| {
            format!(
                "{{\"event\":\"worktree_merged\",\"factory_session\":\"{session}\",\"epic_id\":\"{}\",\"target_branch\":\"{branch}\",\"target_tip\":\"{}\"}}\n",
                epic.id, epic.tip
            )
        };
        fs::write(&log_path, make_event("other-session", &epic.branch)).unwrap();
        let error = recovery_request_for_focus(repo.path(), &cas_dir, "recovery-session", &epic.id)
            .unwrap_err();
        assert!(error.contains("no complete authentic"), "{error}");

        fs::write(&log_path, make_event("recovery-session", "epic/wrong")).unwrap();
        let error = recovery_request_for_focus(repo.path(), &cas_dir, "recovery-session", &epic.id)
            .unwrap_err();
        assert!(error.contains("no complete authentic"), "{error}");

        fs::write(
            &log_path,
            format!(
                "{}{}",
                make_event("recovery-session", &epic.branch),
                make_event("recovery-session", &epic.branch)
            ),
        )
        .unwrap();
        let error = recovery_request_for_focus(repo.path(), &cas_dir, "recovery-session", &epic.id)
            .unwrap_err();
        assert!(error.contains("2 merge events match"), "{error}");
    }

    #[test]
    fn recovery_finds_authentic_event_for_any_open_epic_after_stale_focus() {
        let repo = fixture();
        let closed_focus = epic(repo.path(), "cas-closed-focus", "closed", "closed\n");
        let open_delivery = epic(repo.path(), "cas-open-delivery", "open", "open\n");
        git(repo.path(), &["checkout", "--detach", "main"]);
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        let mut closed_task = Task::new(closed_focus.id.clone(), closed_focus.id.clone());
        closed_task.task_type = TaskType::Epic;
        closed_task.status = TaskStatus::Closed;
        closed_task.branch = Some(closed_focus.branch.clone());
        tasks.add(&closed_task).unwrap();
        let mut open_task = Task::new(open_delivery.id.clone(), open_delivery.id.clone());
        open_task.task_type = TaskType::Epic;
        open_task.branch = Some(open_delivery.branch.clone());
        tasks.add(&open_task).unwrap();

        let log_path = super::super::factory_session_log_path_for_date(
            &cas_dir,
            chrono::Utc::now().date_naive(),
        );
        fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        fs::write(
            &log_path,
            format!(
                "{{\"event\":\"worktree_merged\",\"factory_session\":\"recovery-session\",\"epic_id\":\"{}\",\"target_branch\":\"{}\",\"target_tip\":\"{}\"}}\n",
                open_delivery.id, open_delivery.branch, open_delivery.tip
            ),
        )
        .unwrap();

        let request = recovery_request_for_any_open_epic(
            repo.path(),
            &cas_dir,
            "recovery-session",
        )
        .unwrap();
        assert_eq!(request.epic_id, open_delivery.id);
        assert_eq!(request.target_branch, open_delivery.branch);
        assert_eq!(request.commit, open_delivery.tip);
    }

    /// GH #954: the trunk is resolved, not assumed: an explicit setting first,
    /// then origin/HEAD, then origin/main and origin/master.
    #[test]
    fn integration_trunk_resolves_config_then_origin_head_then_main_or_master_gh954() {
        let repo = fixture();
        git(repo.path(), &["branch", "-m", "main", "master"]);
        git(repo.path(), &["branch", "release"]);
        git(
            repo.path(),
            &["remote", "add", "origin", repo.path().to_str().unwrap()],
        );
        git(repo.path(), &["fetch", "origin"]);
        let master = git(repo.path(), &["rev-parse", "master"]);

        // No origin/HEAD and no origin/main: origin/master is the trunk.
        assert_eq!(
            resolve_integration_trunk(repo.path(), None).unwrap(),
            ("master".to_owned(), master.clone())
        );
        // An explicit setting wins, written with or without "origin/".
        for configured in ["release", "origin/release", " release "] {
            assert_eq!(
                resolve_integration_trunk(repo.path(), Some(configured))
                    .unwrap()
                    .0,
                "release"
            );
        }
        // A setting that does not resolve on origin is an error, never skipped.
        let error = resolve_integration_trunk(repo.path(), Some("staging")).unwrap_err();
        assert!(error.contains("staging"), "{error}");
        // origin/HEAD, when set, wins over the main/master fallback.
        git(repo.path(), &["remote", "set-head", "origin", "release"]);
        assert_eq!(
            resolve_integration_trunk(repo.path(), None).unwrap().0,
            "release"
        );
        // Nothing to resolve: a clear error naming both ways out.
        let bare = tempfile::tempdir().unwrap();
        git(bare.path(), &["init", "-b", "trunk"]);
        let error = resolve_integration_trunk(bare.path(), None).unwrap_err();
        assert!(error.contains("epic_base_branch"), "{error}");
        assert!(error.contains("set-head"), "{error}");
    }

    /// GH #954: a repository whose trunk is `master` integrates and sweeps.
    /// Before the fix the sweep failed at `rev-parse refs/remotes/origin/main`.
    #[test]
    fn base_only_recovery_integrates_on_a_master_trunk_gh954() {
        let repo = fixture();
        git(repo.path(), &["branch", "-m", "main", "master"]);
        git(
            repo.path(),
            &["checkout", "-b", "epic/cas-master-open", "master"],
        );
        fs::write(repo.path().join("open"), "open\n").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-m", "cas-master-open"]);
        let epic_tip = git(repo.path(), &["rev-parse", "HEAD"]);
        git(repo.path(), &["checkout", "--detach", "master"]);
        git(
            repo.path(),
            &["remote", "add", "origin", repo.path().to_str().unwrap()],
        );
        let stub = repo.path().join("cargo-stub.sh");
        crate::test_paths::warm_stub(&stub, "#!/bin/sh\necho 'Summary: 1 passed'\n");
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CARGO", stub.to_str().unwrap()),
            ("CAS_FACTORY_BUILD_GUARD", "off"),
        ]);
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        let mut open_task = Task::new("cas-master-open".to_owned(), "cas-master-open".to_owned());
        open_task.task_type = TaskType::Epic;
        open_task.branch = Some("epic/cas-master-open".to_owned());
        tasks.add(&open_task).unwrap();

        assert_eq!(
            base_only_recovery_request(repo.path())
                .unwrap()
                .target_branch,
            "main",
            "before the fetch nothing resolves on origin yet; the label falls back"
        );
        let summary = crate::ui::factory::daemon::FactoryDaemon::recover_integration(
            repo.path(),
            &cas_dir,
            "master-trunk-session",
            None,
            true,
            &FactoryConfig::default(),
        )
        .unwrap();
        assert!(summary.starts_with("PASSED:"), "{summary}");
        let receipt: IntegrationReceipt = serde_json::from_slice(
            &fs::read(cas_dir.join(LOG_DIR).join("integration.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt.status, "PASSED");
        assert_eq!(receipt.trunk, "master");
        assert_eq!(receipt.base, git(repo.path(), &["rev-parse", "master"]));
        let tip = receipt.tip.as_deref().expect("integration tip published");
        git(
            repo.path(),
            &["merge-base", "--is-ancestor", &receipt.base, tip],
        );
        git(
            repo.path(),
            &["merge-base", "--is-ancestor", &epic_tip, tip],
        );
        assert_eq!(
            base_only_recovery_request(repo.path())
                .unwrap()
                .target_branch,
            "master",
            "after the fetch the recovery label names the resolved trunk"
        );
    }

    #[test]
    fn base_only_recovery_sweeps_origin_main_and_open_epics_without_closed_epics() {
        let repo = fixture();
        let open = epic(repo.path(), "cas-base-open", "open", "open\n");
        let closed = epic(repo.path(), "cas-base-closed", "closed", "closed\n");
        git(repo.path(), &["checkout", "--detach", "main"]);
        git(
            repo.path(),
            &["remote", "add", "origin", repo.path().to_str().unwrap()],
        );
        let stub = repo.path().join("cargo-stub.sh");
        crate::test_paths::warm_stub(&stub, "#!/bin/sh\necho 'Summary: 1 passed'\n");
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CARGO", stub.to_str().unwrap()),
            ("CAS_FACTORY_BUILD_GUARD", "off"),
        ]);
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let tasks = crate::store::open_task_store(&cas_dir).unwrap();
        let mut open_task = Task::new(open.id.clone(), open.id.clone());
        open_task.task_type = TaskType::Epic;
        open_task.branch = Some(open.branch.clone());
        tasks.add(&open_task).unwrap();
        let mut closed_task = Task::new(closed.id.clone(), closed.id.clone());
        closed_task.task_type = TaskType::Epic;
        closed_task.status = TaskStatus::Closed;
        closed_task.branch = Some(closed.branch.clone());
        tasks.add(&closed_task).unwrap();

        let summary = crate::ui::factory::daemon::FactoryDaemon::recover_integration(
            repo.path(),
            &cas_dir,
            "base-only-session",
            None,
            true,
            &FactoryConfig::default(),
        )
        .unwrap();
        assert!(summary.starts_with("PASSED:"), "{summary}");
        let receipt: IntegrationReceipt = serde_json::from_slice(
            &fs::read(cas_dir.join(LOG_DIR).join("integration.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt.status, "PASSED");
        assert_eq!(
            receipt
                .epics
                .iter()
                .map(|epic| epic.id.as_str())
                .collect::<Vec<_>>(),
            [open.id.as_str()]
        );
        assert!(!receipt
            .epics
            .iter()
            .any(|epic| epic.id == closed.id));
        assert_eq!(
            receipt.test_process_env_scrubbed,
            scrubbed_test_process_identity_names()
        );
    }

    #[test]
    fn recovery_cancellation_terminates_the_owned_test_runner() {
        let repo = fixture();
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let marker = cas_dir.join("runner-started");
        let stub = repo.path().join("cargo-stub.sh");
        crate::test_paths::warm_stub(
            &stub,
            &format!(
                "#!/bin/sh\nprintf started > '{}'\nexec sleep 30\n",
                marker.display()
            ),
        );
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CARGO", stub.to_str().unwrap()),
            ("CAS_FACTORY_BUILD_GUARD", "off"),
        ]);
        let request = SweepRequest {
            epic_id: "cas-runnerkill".to_owned(),
            target_branch: "main".to_owned(),
            commit: git(repo.path(), &["rev-parse", "HEAD"]),
        };
        let mut settings = SweepSettings::from(&FactoryConfig::default());
        settings.timeout = Duration::from_secs(30);
        settings.nice_cargo = false;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_root = repo.path().to_path_buf();
        let worker_cas = cas_dir.clone();
        let worker_request = request.clone();
        let worker_settings = settings.clone();
        let worker = std::thread::spawn(move || {
            super::super::execute_sweep(
                &worker_root,
                &worker_cas,
                worker_request,
                worker_settings,
                worker_cancel,
            )
        });
        let started = Instant::now();
        while !marker.exists() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "the owned runner did not start");
        let cancelled_at = Instant::now();
        cancel.store(true, Ordering::Relaxed);
        let result = worker.join().unwrap();
        assert_eq!(result.status, SweepStatus::Superseded);
        assert!(cancelled_at.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn recovery_deadline_cancels_while_waiting_for_the_shared_lock() {
        let repo = fixture();
        let cas_dir = crate::store::init_cas_dir(repo.path()).unwrap();
        let common = PathBuf::from(git(
            repo.path(),
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ));
        let project = common
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        let lock_branch = format!("integration/{}", sanitize_component(&project));
        let held =
            crate::worktree::target_lock::try_lock_delivery_target(&cas_dir, &common, &lock_branch)
                .unwrap()
                .expect("fixture owns the recovery lock");
        let request = SweepRequest {
            epic_id: "cas-lockwait".to_owned(),
            target_branch: "epic/cas-lockwait".to_owned(),
            commit: git(repo.path(), &["rev-parse", "HEAD"]),
        };
        let mut settings = SweepSettings::from(&FactoryConfig::default());
        settings.timeout = Duration::from_secs(10);
        let mut coordinator = super::super::MergeSweepCoordinator::new(&cas_dir, "s1");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let started = Instant::now();
        let result = runtime
            .block_on(coordinator.recover_once_with_deadline(
                repo.path(),
                &cas_dir,
                request,
                &settings,
                true,
                Duration::from_millis(40),
            ))
            .unwrap();
        drop(held);
        assert_eq!(result.status, SweepStatus::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(result.summary.contains("execution cancellation completed"));
    }
}
