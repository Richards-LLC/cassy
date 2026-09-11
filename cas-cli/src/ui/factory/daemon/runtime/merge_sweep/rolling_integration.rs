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
    epics: Vec<EpicTip>,
    tip: Option<String>,
    status: String,
    detail: String,
    affected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_failure: Option<BaseFailure>,
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
fn assemble(worktree: &Path, base: &str, epics: &[EpicTip]) -> Result<Assembly, String> {
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
            // Probe base+previous epic against the incoming epic without
            // touching the assembly checkout or manufacturing a commit.
            for prior in &epics[..index] {
                let output = Command::new("git")
                    .current_dir(worktree)
                    .args(["merge-tree", "--write-tree", &prior.tip, &epic.tip])
                    .output()
                    .map_err(|error| error.to_string())?;
                if output.status.code() == Some(1) {
                    pairs.push(prior.id.clone());
                } else if !output.status.success() {
                    return Err(format!(
                        "conflict attribution probe {}: {}",
                        prior.id,
                        first_output_line(&output.stderr)
                    ));
                }
            }
            if pairs.is_empty() {
                // Main or a higher-order interaction; name it honestly instead
                // of inventing a pair unsupported by a merge probe.
                pairs.extend(epics[..index].iter().map(|prior| prior.id.clone()));
            }
            pairs.push(epic.id.clone());
            return Ok(Assembly::Conflict {
                detail: format!(
                    "Conflict adding {} against {}. Files: {}",
                    epic.id,
                    if index == 0 {
                        "origin/main".to_owned()
                    } else {
                        pairs[..pairs.len() - 1].join(", ")
                    },
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

/// Build the set from all open task rows, independent of ownership. Rows with
/// no live local or origin ref are ignored; a divergent local/origin pair is
/// still an actionable integration error and is preserved for the caller.
fn live_open_epics(root: &Path, tasks: Vec<Task>) -> Result<Vec<EpicTip>, String> {
    open_epics(tasks)
        .into_iter()
        .filter_map(|task| {
            let branch = epic_branch(&task)?.to_owned();
            let local = ref_tip(root, &format!("refs/heads/{branch}"));
            let remote = ref_tip(root, &format!("refs/remotes/origin/{branch}"));
            if local.is_none() && remote.is_none() {
                return None;
            }
            let tip = match resolve_epic_tip(root, &branch) {
                Ok(tip) => tip,
                Err(error) => return Some(Err(error)),
            };
            Some(Ok(EpicTip {
                id: task.id,
                branch,
                tip,
                owner: task.epic_verification_owner.or(task.assignee),
            }))
        })
        .collect()
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
) -> SweepResult {
    match integrate(project_root, cas_dir, &request, &settings, &cancel) {
        Ok(result) => result,
        Err(error) => SweepResult {
            integration_epics: vec![request.epic_id.clone()],
            request,
            status: SweepStatus::SetupFailed,
            log_path: cas_dir.join(LOG_DIR).join("integration.json"),
            summary: format!("Rolling integration setup failed: {error}"),
            failures: Vec::new(),
            base_failure: None,
        },
    }
}

fn integrate(
    project_root: &Path,
    cas_dir: &Path,
    request: &SweepRequest,
    settings: &SweepSettings,
    cancel: &Arc<AtomicBool>,
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
    let receipt_path = shared_cas.join(LOG_DIR).join("integration.json");
    // Invalidate an old green receipt before any fallible setup, so a failed
    // fetch or missing epic can never leave release assembly looking green.
    let mut receipt = IntegrationReceipt {
        base: String::new(),
        epics: Vec::new(),
        tip: None,
        status: "RUNNING".to_owned(),
        detail: format!("Triggered by {} at {}", request.epic_id, request.commit),
        affected: vec![request.epic_id.clone()],
        base_failure: None,
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
    let base = git_output(
        project_root,
        &["rev-parse", "--verify", "refs/remotes/origin/main^{commit}"],
    )?;
    receipt.base = base.clone();
    let store = crate::store::open_task_store(&shared_cas).map_err(|error| error.to_string())?;
    let mut tasks = store.list(None).map_err(|error| error.to_string())?;
    if let Some(focused_id) = focused_epic_id_for_project(main_root) {
        if !tasks.iter().any(|task| task.id == focused_id) {
            if let Ok(task) = store.get(&focused_id) {
                tasks.push(task);
            }
        }
    }
    receipt.epics = live_open_epics(project_root, tasks)?;
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
    let (tip, prefixes) = match assemble(&worktree, &base, &receipt.epics)? {
        Assembly::Conflict { detail, affected } => {
            receipt.status = "CONFLICT".to_owned();
            receipt.detail = detail.clone();
            receipt.affected = affected.clone();
            write_receipt(&receipt_path, &receipt)?;
            return Ok(SweepResult {
                request: request.clone(),
                status: SweepStatus::Failed,
                log_path: receipt_path,
                summary: detail,
                failures: Vec::new(),
                integration_epics: affected,
                base_failure: None,
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
        write_receipt(&receipt_path, &receipt)?;
        return Ok(SweepResult {
            request: request.clone(),
            status: SweepStatus::Deferred,
            log_path: receipt_path,
            summary: format!("{branch} updated; sweep deferred: {}", receipt.detail),
            failures: Vec::new(),
            integration_epics: vec![request.epic_id.clone()],
            base_failure: None,
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
    if result.status == SweepStatus::Passed {
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
                result
                    .summary
                    .push_str("; failing targets also fail on origin/main (no epic attribution)");
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
    receipt.detail = sweep_detail(&result);
    receipt.affected = affected;
    receipt.base_failure = base_failure;
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
            format!(
                "test(/^{}$/)",
                regex::escape(&name).replace('/', "\\/")
            )
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
    if result.status == SweepStatus::Passed {
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
    let detail = format!(
        "Rolling integration {}. Epics: {}. {}. Log: {}",
        status_text(result.status),
        result.integration_epics.join(", "),
        sweep_detail(result),
        result.log_path.display()
    );
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
    let payload = serde_json::json!({ "kind": "sweep_failed", "detail": detail,
        "epics": result.integration_epics, "factory_session": agent.factory_session })
    .to_string();
    let notification = queue
        .notify_idempotent(
            owner,
            "sweep_failed",
            &payload,
            NotificationPriority::High,
            &key,
        )
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
        "<worker-attention kind=\"sweep_failed\" worker=\"supervisor\" notification_id=\"{id}\">\n{detail}\n</worker-attention>"
    );
    let prompts =
        crate::store::open_prompt_queue_store(cas_dir).map_err(|error| error.to_string())?;
    prompts
        .enqueue_idempotent(
            &format!("lifecycle-wake:integration:{id}"),
            "supervisor",
            &body,
            Some(session),
            Some("Rolling integration requires attention"),
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
        fs::write(temp.path().join("shared"), "base\n").unwrap();
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
        let result = assemble(repo.path(), "main", &[first.clone(), second.clone()]).unwrap();
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
        let Assembly::Clean { tip, prefixes } =
            assemble(repo.path(), "main", &[first.clone(), second.clone()]).unwrap()
        else {
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
            assemble(repo.path(), "main", &[first, second]).unwrap()
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

        let epics = live_open_epics(repo.path(), vec![closed, open]).unwrap();
        assert_eq!(epics.len(), 1);
        assert_eq!(epics[0].id, "origin-open");
        assert_eq!(epics[0].owner, None);
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
        use std::os::unix::fs::PermissionsExt;
        let repo = fixture();
        let first = epic(repo.path(), "cas-0081", "a", "one");
        let second = epic(repo.path(), "cas-ed91a", "b", "two");
        git(repo.path(), &["checkout", "--detach", "main"]);
        git(
            repo.path(),
            &["remote", "add", "origin", repo.path().to_str().unwrap()],
        );
        let stub = repo.path().join("cargo-stub.sh");
        fs::write(
            &stub,
            r#"#!/bin/sh
if [ -f a ] && [ -f b ]; then
  echo 'FAIL [0.1s] fixture union_test'
  echo 'Summary: 1 failed'
  exit 1
fi
echo 'Summary: 1 passed'
"#,
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
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
            git(repo.path(), &["merge-base", "--is-ancestor", &receipt.base, tip]);
        }
    }
}
