//! Bounded validation of the merged epic tip.
//!
//! `worktree_merge` only records a durable event.  The daemon tails those
//! events and validates the rolling union in a detached checkout so the MCP
//! merge request is not held open by a workspace-sized nextest run. A newer
//! merge supersedes the older job; the shared target lock serializes sessions.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::config::FactoryConfig;

mod rolling_integration;

const LOG_DIR: &str = "merge-sweeps";
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_FAILURE_LINES: usize = 12;
const MAX_NOTE_CHARS: usize = 1400;

#[derive(Debug, Clone, Deserialize)]
struct MergeEvent {
    event: Option<String>,
    factory_session: Option<String>,
    epic_id: Option<String>,
    target_branch: Option<String>,
    commit: Option<String>,
    target_tip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SweepRequest {
    epic_id: String,
    target_branch: String,
    commit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepStatus {
    Passed,
    Failed,
    Unavailable,
    TimedOut,
    SetupFailed,
    Superseded,
    Deferred,
}

#[derive(Debug)]
struct SweepResult {
    request: SweepRequest,
    status: SweepStatus,
    log_path: PathBuf,
    summary: String,
    failures: Vec<String>,
    integration_epics: Vec<String>,
    base_failure: Option<rolling_integration::BaseFailure>,
}

#[derive(Debug)]
struct ActiveSweep {
    request: SweepRequest,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<SweepResult>,
    pending: Option<SweepRequest>,
}

#[derive(Debug, Clone)]
struct SweepSettings {
    enabled: bool,
    timeout: Duration,
    cargo_build_jobs: String,
    nice_cargo: bool,
    max_concurrent_builders: usize,
    nextest_filter: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestRunnerKind {
    Cargo,
    Package,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestRunner {
    kind: TestRunnerKind,
    program: String,
    args: Vec<String>,
    package_manager: Option<&'static str>,
}

impl From<&FactoryConfig> for SweepSettings {
    fn from(config: &FactoryConfig) -> Self {
        Self {
            enabled: config.merge_sweep,
            timeout: Duration::from_secs(config.merge_sweep_timeout_secs.max(1)),
            cargo_build_jobs: config.cargo_build_jobs.clone(),
            nice_cargo: config.nice_cargo,
            max_concurrent_builders: config.max_concurrent_builders,
            nextest_filter: None,
        }
    }
}

/// Process-local state for the daemon-owned post-merge sweeps.
#[derive(Debug)]
pub(crate) struct MergeSweepCoordinator {
    cas_dir: PathBuf,
    session_name: String,
    first_log_day: chrono::NaiveDate,
    log_offsets: HashMap<PathBuf, u64>,
    active: HashMap<String, ActiveSweep>,
    completed: HashMap<String, String>,
    retry_after: Option<(Instant, SweepRequest)>,
}

impl MergeSweepCoordinator {
    pub(crate) fn new(cas_dir: &Path, session_name: &str) -> Self {
        Self::new_at(cas_dir, session_name, chrono::Utc::now().date_naive())
    }

    fn new_at(cas_dir: &Path, session_name: &str, first_log_day: chrono::NaiveDate) -> Self {
        let first_log_path = factory_session_log_path_for_date(cas_dir, first_log_day);
        let first_log_offset = fs::metadata(&first_log_path)
            .map(|meta| meta.len())
            .unwrap_or(0);
        Self {
            cas_dir: cas_dir.to_path_buf(),
            session_name: session_name.to_string(),
            first_log_day,
            log_offsets: HashMap::from([(first_log_path, first_log_offset)]),
            active: HashMap::new(),
            completed: HashMap::new(),
            retry_after: None,
        }
    }

    pub(super) async fn poll(
        &mut self,
        project_root: &Path,
        cas_dir: &Path,
        config: &FactoryConfig,
    ) {
        let mut requests = self.read_merge_events();
        if self
            .retry_after
            .as_ref()
            .is_some_and(|(when, _)| Instant::now() >= *when)
        {
            if let Some((_, request)) = self.retry_after.take() {
                requests.push(request);
            }
        }
        requests.extend(self.reap_finished(cas_dir).await);

        let settings = SweepSettings::from(config);
        for request in requests {
            self.schedule(project_root, cas_dir, request, &settings, false);
        }
    }

    async fn recover_once(
        &mut self,
        project_root: &Path,
        cas_dir: &Path,
        request: SweepRequest,
        settings: &SweepSettings,
    ) -> Result<SweepResult, String> {
        let deadline = settings
            .timeout
            .saturating_mul(4)
            .min(Duration::from_secs(4 * 60 * 60));
        self.recover_once_with_deadline(project_root, cas_dir, request, settings, deadline)
            .await
    }

    async fn recover_once_with_deadline(
        &mut self,
        project_root: &Path,
        cas_dir: &Path,
        request: SweepRequest,
        settings: &SweepSettings,
        deadline: Duration,
    ) -> Result<SweepResult, String> {
        if !settings.enabled {
            return Err("recovery refused: factory.merge_sweep=false".to_owned());
        }
        if !self.active.is_empty() {
            return Err(
                "recovery refused: this coordinator already has an active sweep".to_owned(),
            );
        }

        self.schedule(project_root, cas_dir, request, settings, true);
        let mut active = self
            .active
            .remove("integration")
            .ok_or("recovery coordinator did not start the requested sweep")?;
        let result = match tokio::time::timeout(deadline, &mut active.handle).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => failed_join_result(cas_dir, &active.request, error.to_string()),
            Err(_) => {
                active.cancel.store(true, Ordering::Relaxed);
                match active.handle.await {
                    Ok(mut result) => {
                        if result.status == SweepStatus::Superseded {
                            result.status = SweepStatus::TimedOut;
                            result.summary = format!(
                                "recovery exceeded its bounded {deadline:?} deadline; execution cancellation completed"
                            );
                        }
                        result
                    }
                    Err(error) => failed_join_result(cas_dir, &active.request, error.to_string()),
                }
            }
        };
        self.record_result(cas_dir, &result);
        Ok(result)
    }

    pub(super) async fn shutdown(&mut self) {
        for active in self.active.values() {
            active.cancel.store(true, Ordering::Relaxed);
        }
        let active = self.active.drain().map(|(_, sweep)| sweep);
        for sweep in active {
            let _ = sweep.handle.await;
        }
    }

    fn read_merge_events(&mut self) -> Vec<SweepRequest> {
        self.read_merge_events_at(chrono::Utc::now().date_naive())
    }

    fn read_merge_events_at(&mut self, through_day: chrono::NaiveDate) -> Vec<SweepRequest> {
        session_log_paths_between(&self.cas_dir, self.first_log_day, through_day)
            .into_iter()
            .flat_map(|path| self.read_merge_events_from_path(&path))
            .collect()
    }

    fn read_merge_events_from_path(&mut self, path: &Path) -> Vec<SweepRequest> {
        let Ok(bytes) = fs::read(path) else {
            return Vec::new();
        };
        let offset = self.log_offsets.entry(path.to_path_buf()).or_default();
        if *offset > bytes.len() as u64 {
            *offset = 0;
        }
        let start = *offset as usize;
        let Some(last_newline) = bytes[start..].iter().rposition(|byte| *byte == b'\n') else {
            // Keep the cursor before a partial JSONL line; the next poll will
            // retry it with the appended bytes instead of discarding it.
            return Vec::new();
        };
        let consumed = start + last_newline + 1;
        *offset = consumed as u64;
        String::from_utf8_lossy(&bytes[start..consumed])
            .lines()
            .filter_map(|line| parse_merge_event(line, &self.session_name))
            .collect()
    }

    async fn reap_finished(&mut self, cas_dir: &Path) -> Vec<SweepRequest> {
        let finished: Vec<String> = self
            .active
            .iter()
            .filter_map(|(epic_id, sweep)| sweep.handle.is_finished().then_some(epic_id.clone()))
            .collect();
        let mut pending = Vec::new();
        for epic_id in finished {
            let Some(mut active) = self.active.remove(&epic_id) else {
                continue;
            };
            let result = match active.handle.await {
                Ok(result) => result,
                Err(error) => SweepResult {
                    request: active.request.clone(),
                    status: SweepStatus::Failed,
                    log_path: cas_dir.join(LOG_DIR).join("unknown.log"),
                    summary: format!("sweep task join failed: {error}"),
                    failures: Vec::new(),
                    integration_epics: Vec::new(),
                    base_failure: None,
                },
            };
            let superseded = active.pending.is_some() || result.status == SweepStatus::Superseded;
            if !superseded {
                self.record_result(cas_dir, &result);
            }
            if let Some(next) = active.pending.take() {
                pending.push(next);
            } else if !superseded && result.status == SweepStatus::Deferred {
                self.retry_after = Some((Instant::now() + Duration::from_secs(30), result.request));
            } else if !superseded {
                self.completed.insert(
                    result.request.epic_id.clone(),
                    result.request.commit.clone(),
                );
            }
        }
        pending
    }

    fn schedule(
        &mut self,
        project_root: &Path,
        cas_dir: &Path,
        request: SweepRequest,
        settings: &SweepSettings,
        strict_target: bool,
    ) {
        if let Some(active) = self.active.get_mut("integration") {
            if active.request != request {
                active.pending = Some(request);
                active.cancel.store(true, Ordering::Relaxed);
            }
            return;
        }
        if self.completed.get(&request.epic_id) == Some(&request.commit) {
            return;
        }
        if !settings.enabled {
            self.record_deferred(cas_dir, &request, "disabled by factory.merge_sweep=false");
            self.completed
                .insert(request.epic_id.clone(), request.commit.clone());
            return;
        }

        self.retry_after = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let project_root = project_root.to_path_buf();
        let cas_dir = cas_dir.to_path_buf();
        let sweep_settings = settings.clone();
        let worker_request = request.clone();
        let handle = tokio::task::spawn_blocking(move || {
            rolling_integration::execute(
                &project_root,
                &cas_dir,
                worker_request,
                sweep_settings,
                worker_cancel,
                strict_target,
            )
        });
        tracing::info!(
            epic = %request.epic_id,
            commit = %request.commit,
            "started bounded post-merge workspace sweep"
        );
        self.active.insert(
            "integration".to_owned(),
            ActiveSweep {
                request,
                cancel,
                handle,
                pending: None,
            },
        );
    }

    fn record_deferred(&self, cas_dir: &Path, request: &SweepRequest, reason: &str) {
        let result = SweepResult {
            request: request.clone(),
            status: SweepStatus::Deferred,
            log_path: cas_dir.join(LOG_DIR).join(log_file_name(request)),
            summary: format!("workspace sweep deferred: {reason}"),
            failures: Vec::new(),
            integration_epics: Vec::new(),
            base_failure: None,
        };
        append_epic_note(cas_dir, &result);
        tracing::warn!(epic = %request.epic_id, reason, "post-merge workspace sweep deferred");
    }

    fn record_result(&self, cas_dir: &Path, result: &SweepResult) {
        if result.status == SweepStatus::Superseded {
            tracing::debug!(epic = %result.request.epic_id, commit = %result.request.commit, "post-merge sweep superseded");
            return;
        }
        if !result.integration_epics.is_empty() {
            rolling_integration::record_result(cas_dir, result);
            return;
        }
        append_epic_note(cas_dir, result);
        if matches!(
            result.status,
            SweepStatus::Failed | SweepStatus::TimedOut | SweepStatus::SetupFailed
        ) {
            let detail = sweep_detail(result);
            let occurrence = format!(
                "{}:{}:{:?}",
                result.request.epic_id, result.request.commit, result.status
            );
            let _ = super::lifecycle::enqueue_merge_sweep_failure_relay(
                cas_dir,
                &result.request.epic_id,
                &result.request.commit,
                &detail,
                &occurrence,
            );
        }
    }
}

fn failed_join_result(cas_dir: &Path, request: &SweepRequest, error: String) -> SweepResult {
    SweepResult {
        request: request.clone(),
        status: SweepStatus::Failed,
        log_path: cas_dir.join(LOG_DIR).join("integration.json"),
        summary: format!("sweep task join failed: {error}"),
        failures: Vec::new(),
        integration_epics: vec![request.epic_id.clone()],
        base_failure: None,
    }
}

impl crate::ui::factory::daemon::FactoryDaemon {
    /// Fresh-process entrypoint for a one-shot recovery. It constructs a local
    /// coordinator and uses the same rolling integration execution/recording
    /// path as daemon polling, without reaching into another process's state.
    pub(crate) fn recover_focused_integration(
        project_root: &Path,
        cas_dir: &Path,
        session_name: &str,
        epic_id: &str,
        config: &FactoryConfig,
    ) -> Result<String, String> {
        let request = rolling_integration::recovery_request_for_focus(
            project_root,
            cas_dir,
            session_name,
            epic_id,
        )?;
        let mut coordinator = MergeSweepCoordinator::new(cas_dir, session_name);
        let settings = SweepSettings::from(config);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("could not initialize bounded recovery runtime: {error}"))?;
        let result = runtime.block_on(coordinator.recover_once(
            project_root,
            cas_dir,
            request,
            &settings,
        ))?;
        let receipt_path = cas_dir.join(LOG_DIR).join("integration.json");
        let summary = format!(
            "{}: {}; receipt: {}",
            status_text(result.status),
            result.summary,
            receipt_path.display()
        );
        if result.status != SweepStatus::Passed {
            return Err(summary);
        }
        Ok(summary)
    }

    pub(super) async fn poll_merge_sweep(&mut self) {
        let config = crate::config::Config::load(self.app.cas_dir())
            .unwrap_or_default()
            .factory()
            .clone();
        let project_root = self.app.project_path().to_path_buf();
        let cas_dir = self.app.cas_dir().to_path_buf();
        self.merge_sweep
            .poll(&project_root, &cas_dir, &config)
            .await;
    }
}

fn parse_merge_event(line: &str, session_name: &str) -> Option<SweepRequest> {
    let event: MergeEvent = serde_json::from_str(line).ok()?;
    if event.event.as_deref() != Some("worktree_merged")
        || event.factory_session.as_deref() != Some(session_name)
    {
        return None;
    }
    let epic_id = nonempty_event_value(event.epic_id.as_deref())?;
    let target_branch = event.target_branch?.trim().to_string();
    if !target_branch.starts_with("epic/") {
        return None;
    }
    let commit = nonempty_event_value(event.target_tip.as_deref())
        .or_else(|| nonempty_event_value(event.commit.as_deref()))?;
    Some(SweepRequest {
        epic_id,
        target_branch,
        commit,
    })
}

fn nonempty_event_value(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty() && value != "none").then(|| value.to_string())
}

fn factory_session_log_path_for_date(cas_dir: &Path, day: chrono::NaiveDate) -> PathBuf {
    cas_dir
        .join("logs")
        .join(format!("factory-session-{day}.log"))
}

fn session_log_paths_between(
    cas_dir: &Path,
    first_day: chrono::NaiveDate,
    through_day: chrono::NaiveDate,
) -> Vec<PathBuf> {
    if through_day < first_day {
        return Vec::new();
    }
    let log_dir = cas_dir.join("logs");
    let Ok(entries) = fs::read_dir(log_dir) else {
        return Vec::new();
    };
    let mut dated_paths: Vec<(chrono::NaiveDate, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let date = name
                .strip_prefix("factory-session-")?
                .strip_suffix(".log")?;
            let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
            (first_day <= day && day <= through_day).then(|| (day, entry.path()))
        })
        .collect();
    dated_paths.sort_by_key(|(day, _)| *day);
    dated_paths.into_iter().map(|(_, path)| path).collect()
}

fn settings_to_config(settings: &SweepSettings) -> FactoryConfig {
    let mut config = FactoryConfig::default();
    config.cargo_build_jobs = settings.cargo_build_jobs.clone();
    config.nice_cargo = settings.nice_cargo;
    config.max_concurrent_builders = settings.max_concurrent_builders;
    config
}

/// Resolve the target project's declared test entry point in the detached
/// merge checkout. Rust keeps the existing configured `CARGO` path; Node
/// projects use the repository's lockfile/package-manager convention and
/// invoke only the declared `test` script. No install or shell interpolation
/// is performed by the sweep.
fn resolve_test_runner(worktree: &Path) -> Result<TestRunner, String> {
    if worktree.join("Cargo.toml").is_file() {
        return Ok(TestRunner {
            kind: TestRunnerKind::Cargo,
            program: std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()),
            args: vec![
                "nextest".to_owned(),
                "run".to_owned(),
                "--workspace".to_owned(),
                "--no-fail-fast".to_owned(),
            ],
            package_manager: None,
        });
    }

    let manifest_path = worktree.join("package.json");
    let contents = fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
            "sweep unavailable: target has no supported test runner; expected Cargo.toml or a readable package.json ({error})"
        )
    })?;
    let manifest: Value = serde_json::from_str(&contents).map_err(|error| {
        format!(
            "sweep unavailable: cannot parse target package.json; expected a JSON manifest with scripts.test ({error})"
        )
    })?;
    let has_test_script = manifest
        .get("scripts")
        .and_then(Value::as_object)
        .and_then(|scripts| scripts.get("test"))
        .and_then(Value::as_str)
        .is_some_and(|script| !script.trim().is_empty());
    if !has_test_script {
        return Err(
            "sweep unavailable: target package.json must declare a non-empty scripts.test entry"
                .to_owned(),
        );
    }

    let manager = package_manager(worktree, &manifest)?;
    let env_name = manager.to_ascii_uppercase();
    let program = std::env::var(&env_name).unwrap_or_else(|_| manager.to_owned());
    let args = if manager == "bun" {
        // `bun test` invokes Bun's built-in test runner. `bun run test` is the
        // package-manager form that executes the declared scripts.test entry.
        vec!["run".to_owned(), "test".to_owned()]
    } else {
        vec!["test".to_owned()]
    };
    Ok(TestRunner {
        kind: TestRunnerKind::Package,
        program,
        args,
        package_manager: Some(manager),
    })
}

/// Keep this in lockstep with the worktree dependency setup resolver. A
/// committed lockfile wins over `packageManager`; without either, the
/// existing canonical fallback is npm.
fn package_manager(worktree: &Path, manifest: &Value) -> Result<&'static str, String> {
    let manager = if worktree.join("package-lock.json").is_file()
        || worktree.join("npm-shrinkwrap.json").is_file()
    {
        "npm".to_owned()
    } else if worktree.join("pnpm-lock.yaml").is_file() {
        "pnpm".to_owned()
    } else if worktree.join("yarn.lock").is_file() {
        "yarn".to_owned()
    } else if worktree.join("bun.lock").is_file() || worktree.join("bun.lockb").is_file() {
        "bun".to_owned()
    } else {
        manifest
            .get("packageManager")
            .and_then(Value::as_str)
            .and_then(|value| value.split('@').next())
            .filter(|value| !value.is_empty())
            .unwrap_or("npm")
            .to_owned()
    };
    match manager.as_str() {
        "npm" => Ok("npm"),
        "pnpm" => Ok("pnpm"),
        "yarn" => Ok("yarn"),
        "bun" => Ok("bun"),
        other => Err(format!(
            "sweep unavailable: unsupported package manager `{other}`; use npm, pnpm, yarn, or bun in the target package.json/lockfile"
        )),
    }
}

fn format_command(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_owned())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn package_install_command(worktree: &Path, manager: &str) -> &'static str {
    match manager {
        "npm"
            if worktree.join("package-lock.json").is_file()
                || worktree.join("npm-shrinkwrap.json").is_file() =>
        {
            "npm ci"
        }
        "pnpm" if worktree.join("pnpm-lock.yaml").is_file() => "pnpm install --frozen-lockfile",
        "yarn" if worktree.join("yarn.lock").is_file() => {
            if worktree.join(".yarnrc.yml").is_file() {
                "yarn install --immutable"
            } else {
                "yarn install --frozen-lockfile"
            }
        }
        "bun" if worktree.join("bun.lock").is_file() || worktree.join("bun.lockb").is_file() => {
            "bun install --frozen-lockfile"
        }
        "pnpm" => "pnpm install",
        "yarn" => "yarn install",
        "bun" => "bun install",
        _ => "npm install",
    }
}

fn missing_package_setup(worktree: &Path, runner: &TestRunner) -> Option<String> {
    let manager = runner.package_manager?;
    let manifest = fs::read_to_string(worktree.join("package.json"))
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())?;
    let mut dependencies = Vec::new();
    for section in ["dependencies", "devDependencies"] {
        if let Some(entries) = manifest.get(section).and_then(Value::as_object) {
            dependencies.extend(entries.keys().cloned());
        }
    }
    if dependencies.is_empty() {
        if let Ok(metadata) = fs::symlink_metadata(worktree.join("node_modules"))
            && (metadata.file_type().is_symlink() || !metadata.is_dir())
        {
            return Some(package_setup_error(
                worktree,
                manager,
                format_args!("target node_modules is not a private directory in this worktree"),
            ));
        }
        return None;
    }
    let node_modules = worktree.join("node_modules");
    let yarn_pnp = manager == "yarn"
        && [".pnp.cjs", ".pnp.js"].into_iter().any(|loader| {
            fs::metadata(worktree.join(loader)).is_ok_and(|metadata| metadata.is_file())
        });
    if yarn_pnp {
        if let Ok(metadata) = fs::symlink_metadata(&node_modules)
            && (metadata.file_type().is_symlink() || !metadata.is_dir())
        {
            return Some(package_setup_error(
                worktree,
                manager,
                format_args!("target node_modules is not a private directory in this worktree"),
            ));
        }
        // Yarn Plug'n'Play records the installed dependency graph in its
        // loader rather than a node_modules tree. The Yarn runner loads this
        // file when it executes the declared script.
        return None;
    }
    let Some(node_modules_metadata) = fs::symlink_metadata(&node_modules).ok() else {
        return Some(package_setup_error(
            worktree,
            manager,
            format_args!(
                "target dependencies are not installed at {}",
                node_modules.display()
            ),
        ));
    };
    if node_modules_metadata.file_type().is_symlink() {
        return Some(package_setup_error(
            worktree,
            manager,
            format_args!(
                "target node_modules is a symlink at {}; use dependencies installed in this detached worktree",
                node_modules.display()
            ),
        ));
    }
    if !node_modules_metadata.is_dir() {
        return Some(package_setup_error(
            worktree,
            manager,
            format_args!(
                "target node_modules is not a directory at {}",
                node_modules.display()
            ),
        ));
    }
    dependencies
        .into_iter()
        .find(|dependency| {
            !fs::metadata(node_modules.join(dependency)).is_ok_and(|metadata| metadata.is_dir())
        })
        .map(|dependency| {
            package_setup_error(
                worktree,
                manager,
                format_args!(
                    "declared dependency `{dependency}` is not resolvable from {}",
                    node_modules.display()
                ),
            )
        })
}

fn package_setup_error(worktree: &Path, manager: &str, detail: std::fmt::Arguments<'_>) -> String {
    format!(
        "sweep unavailable: {detail}; run `{}` in this detached worktree and retry; Cassy will not install dependencies automatically",
        package_install_command(worktree, manager)
    )
}

fn execute_sweep(
    project_root: &Path,
    cas_dir: &Path,
    request: SweepRequest,
    settings: SweepSettings,
    cancel: Arc<AtomicBool>,
) -> SweepResult {
    let log_path = cas_dir.join(LOG_DIR).join(log_file_name(&request));
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut log = match OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&log_path)
    {
        Ok(log) => log,
        Err(error) => {
            return SweepResult {
                request,
                status: SweepStatus::SetupFailed,
                log_path,
                summary: format!("cannot create sweep log: {error}"),
                failures: Vec::new(),
                integration_epics: Vec::new(),
                base_failure: None,
            };
        }
    };

    let worktree = match prepare_merge_worktree(project_root, &request) {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(log, "workspace setup failed: {error}");
            return SweepResult {
                request,
                status: SweepStatus::SetupFailed,
                log_path: log_path.clone(),
                summary: format!("workspace setup failed: {error}"),
                failures: Vec::new(),
                integration_epics: Vec::new(),
                base_failure: None,
            };
        }
    };
    let runner = match resolve_test_runner(&worktree) {
        Ok(runner) => runner,
        Err(error) => {
            let _ = writeln!(log, "{error}");
            return SweepResult {
                request,
                status: SweepStatus::Unavailable,
                log_path,
                summary: error,
                failures: Vec::new(),
                integration_epics: Vec::new(),
                base_failure: None,
            };
        }
    };
    if let Some(error) = missing_package_setup(&worktree, &runner) {
        let _ = writeln!(log, "{error}");
        return SweepResult {
            request,
            status: SweepStatus::Unavailable,
            log_path,
            summary: error,
            failures: Vec::new(),
            integration_epics: Vec::new(),
            base_failure: None,
        };
    }
    let mut command_display = format_command(&runner.program, &runner.args);
    if runner.kind == TestRunnerKind::Cargo && settings.nice_cargo {
        command_display = format!("nice -n {} {command_display}", nice_level());
    }
    let _ = writeln!(
        log,
        "sweep: {command_display}\nworktree: {}\ntarget: {}",
        worktree.display(),
        request.commit
    );
    let Some(mut child) = spawn_test_runner(&worktree, &settings, &log, &runner) else {
        let summary = format!(
            "sweep unavailable: could not start `{}`; ensure the target project's configured test runner is installed and on PATH",
            runner.program
        );
        let _ = writeln!(log, "{summary}");
        return SweepResult {
            request,
            status: SweepStatus::Unavailable,
            log_path,
            summary,
            failures: Vec::new(),
            integration_epics: Vec::new(),
            base_failure: None,
        };
    };

    let started = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            terminate_child(&mut child);
            break SweepStatus::Superseded;
        }
        if started.elapsed() >= settings.timeout {
            terminate_child(&mut child);
            let _ = writeln!(log, "sweep timed out after {}s", settings.timeout.as_secs());
            break SweepStatus::TimedOut;
        }
        match child.try_wait() {
            Ok(Some(exit)) => {
                break if exit.success() {
                    SweepStatus::Passed
                } else {
                    SweepStatus::Failed
                };
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => {
                let _ = writeln!(log, "failed polling test runner: {error}");
                terminate_child(&mut child);
                break SweepStatus::Failed;
            }
        }
    };
    drop(log);
    let (summary, failures) = summarize_log(&log_path);
    SweepResult {
        request,
        status,
        log_path,
        summary,
        failures,
        integration_epics: Vec::new(),
        base_failure: None,
    }
}

fn prepare_merge_worktree(project_root: &Path, request: &SweepRequest) -> Result<PathBuf, String> {
    let common_dir = git_output(
        project_root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let main_root = common_dir
        .strip_suffix("/.git")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(&common_dir)
                .parent()
                .unwrap_or_else(|| Path::new(&common_dir))
                .to_path_buf()
        });
    let worktree = main_root.join(".cas").join(format!(
        "epic-{}-merge",
        sanitize_component(&request.epic_id)
    ));
    if worktree.exists() {
        let inside = git_output(&worktree, &["rev-parse", "--is-inside-work-tree"])?;
        if inside != "true" {
            return Err(format!(
                "existing path is not a Git worktree: {}",
                worktree.display()
            ));
        }
        let actual_common = git_output(
            &worktree,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        if actual_common != common_dir
            || git_output(&worktree, &["symbolic-ref", "-q", "HEAD"]).is_ok()
        {
            return Err(format!(
                "sweep requires a detached worktree in the same repository: {}",
                worktree.display()
            ));
        }
        let dirty = git_output_allow_empty(&worktree, &["status", "--porcelain"])?;
        if !dirty.is_empty() {
            return Err(format!(
                "merge sweep worktree is dirty: {}",
                worktree.display()
            ));
        }
        git_output(&worktree, &["reset", "--hard", &request.commit])?;
    } else {
        fs::create_dir_all(main_root.join(".cas"))
            .map_err(|error| format!("create .cas: {error}"))?;
        let worktree_text = worktree.to_string_lossy().to_string();
        git_output(
            project_root,
            &[
                "worktree",
                "add",
                "--detach",
                &worktree_text,
                &request.commit,
            ],
        )?;
    }
    hardlink_context_zig(&main_root, &worktree)?;
    Ok(worktree)
}

fn hardlink_context_zig(main_root: &Path, worktree: &Path) -> Result<(), String> {
    let source = main_root.join(".context").join("zig");
    if !source.is_dir() {
        return Ok(());
    }
    let destination = worktree.join(".context").join("zig");
    hardlink_tree(&source, &destination)
}

fn hardlink_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("create {}: {error}", destination.display()))?;
    for entry in
        fs::read_dir(source).map_err(|error| format!("read {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("read directory entry: {error}"))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)
            .map_err(|error| format!("stat {}: {error}", source_path.display()))?;
        if metadata.is_dir() {
            hardlink_tree(&source_path, &destination_path)?;
        } else if metadata.file_type().is_symlink() {
            #[cfg(unix)]
            {
                if !destination_path.exists() {
                    let target = fs::read_link(&source_path)
                        .map_err(|error| format!("read link {}: {error}", source_path.display()))?;
                    std::os::unix::fs::symlink(target, &destination_path)
                        .map_err(|error| format!("link {}: {error}", destination_path.display()))?;
                }
            }
        } else if !destination_path.exists() {
            fs::hard_link(&source_path, &destination_path)
                .map_err(|error| format!("hardlink {}: {error}", destination_path.display()))?;
        }
    }
    Ok(())
}

fn spawn_test_runner(
    worktree: &Path,
    settings: &SweepSettings,
    log: &File,
    runner: &TestRunner,
) -> Option<Child> {
    let mut command = if runner.kind == TestRunnerKind::Cargo && settings.nice_cargo {
        let level = nice_level();
        let mut command = Command::new("nice");
        command.args(["-n", level.as_str()]).arg(&runner.program);
        command
    } else {
        Command::new(&runner.program)
    };
    command
        .current_dir(worktree)
        .args(&runner.args)
        .env("CARGO_BUILD_JOBS", effective_build_jobs(settings))
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().ok()?))
        .stderr(Stdio::from(log.try_clone().ok()?));
    if runner.kind == TestRunnerKind::Cargo {
        if let Some(filter) = &settings.nextest_filter {
            command.args(["-E", filter]);
        }
    }
    if runner.kind == TestRunnerKind::Cargo {
        if let Some(zig) = resolve_zig(worktree) {
            command.env("ZIG", zig);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: only async-signal-safe calls between fork and exec. This
        // gives the sweep nohup/setsid semantics without a shell wrapper.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                Ok(())
            });
        }
    }
    command.spawn().ok()
}

fn resolve_zig(worktree: &Path) -> Option<PathBuf> {
    let configured = std::env::var_os("ZIG").map(PathBuf::from).map(|path| {
        if path.is_absolute() {
            path
        } else {
            worktree.join(path)
        }
    });
    let main = git_output(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .and_then(|path| Path::new(&path).parent().map(Path::to_path_buf));
    configured
        .into_iter()
        .chain([worktree.join(".context/zig/zig")])
        .chain(main.map(|path| path.join(".context/zig/zig")))
        .find(|path| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                path.metadata()
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                path.is_file()
            }
        })
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    // SAFETY: the child created a private session/process group above.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn git_output(current_dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(current_dir)
        .args(args)
        .output()
        .map_err(|error| format!("git {:?}: {error}", args))?;
    if !output.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            first_output_line(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_output_allow_empty(current_dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(current_dir)
        .args(args)
        .output()
        .map_err(|error| format!("git {:?}: {error}", args))?;
    if !output.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            first_output_line(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn summarize_log(path: &Path) -> (String, Vec<String>) {
    let Ok(contents) = fs::read_to_string(path) else {
        return ("sweep log unavailable".to_string(), Vec::new());
    };
    let mut summary = String::new();
    let mut failures = Vec::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.contains("Summary")
            || trimmed.starts_with("test result:")
            || trimmed.contains("tests failed")
        {
            summary = trimmed.to_string();
        }
        if (trimmed.contains("FAIL") || trimmed.contains("FAILED"))
            && !failures.iter().any(|failure| failure == trimmed)
            && failures.len() < MAX_FAILURE_LINES
        {
            failures.push(trimmed.to_string());
        }
    }
    if summary.is_empty() {
        summary = "test runner completed; see sweep log".to_string();
    }
    (summary, failures)
}

fn append_epic_note(cas_dir: &Path, result: &SweepResult) {
    let Ok(task_store) = crate::store::open_task_store(cas_dir) else {
        tracing::warn!(epic = %result.request.epic_id, "cannot open task store for merge sweep note");
        return;
    };
    let Ok(mut task) = task_store.get(&result.request.epic_id) else {
        tracing::warn!(epic = %result.request.epic_id, "merge sweep note target epic not found");
        return;
    };
    let note = truncate_note(&format!(
        "[{}] 📝 PROGRESS Post-merge workspace sweep at {}: {}. {} Log: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        result.request.commit,
        status_text(result.status),
        sweep_detail(result),
        result.log_path.display(),
    ));
    task.notes = if task.notes.is_empty() {
        note
    } else {
        format!("{}\n\n{}", task.notes, note)
    };
    let _ = task_store.update(&task);
}

fn sweep_detail(result: &SweepResult) -> String {
    let failures = if result.failures.is_empty() {
        String::new()
    } else {
        format!(" Failing rows: {}.", result.failures.join(" | "))
    };
    let base_failure = result
        .base_failure
        .as_ref()
        .map(|failure| {
            format!(
                " Base-only evidence: base tip {}; failing set: {}.",
                failure.base,
                if failure.failing.is_empty() {
                    "(none parsed)".to_owned()
                } else {
                    failure.failing.join(" | ")
                }
            )
        })
        .unwrap_or_default();
    format!("{}{}{}", result.summary, failures, base_failure)
}

fn status_text(status: SweepStatus) -> &'static str {
    match status {
        SweepStatus::Passed => "PASSED",
        SweepStatus::Failed => "FAILED",
        SweepStatus::Unavailable => "SWEEP_UNAVAILABLE",
        SweepStatus::TimedOut => "TIMED OUT",
        SweepStatus::SetupFailed => "SETUP FAILED",
        SweepStatus::Superseded => "SUPERSEDED",
        SweepStatus::Deferred => "DEFERRED",
    }
}

fn truncate_note(note: &str) -> String {
    if note.chars().count() <= MAX_NOTE_CHARS {
        return note.to_string();
    }
    let mut truncated: String = note
        .chars()
        .take(MAX_NOTE_CHARS.saturating_sub(3))
        .collect();
    truncated.push_str("...");
    truncated
}

fn log_file_name(request: &SweepRequest) -> String {
    format!(
        "{}-{}.log",
        sanitize_component(&request.epic_id),
        short_commit(&request.commit)
    )
}

fn sanitize_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn short_commit(commit: &str) -> &str {
    commit.get(..commit.len().min(12)).unwrap_or(commit)
}

fn effective_build_jobs(settings: &SweepSettings) -> String {
    if settings
        .cargo_build_jobs
        .trim()
        .eq_ignore_ascii_case("auto")
        || settings.cargo_build_jobs.trim().is_empty()
    {
        let cpus = std::thread::available_parallelism()
            .map(|parallelism| parallelism.get())
            .unwrap_or(1);
        return (2usize.max(cpus / 4)).to_string();
    }
    settings.cargo_build_jobs.trim().to_string()
}

fn nice_level() -> String {
    std::env::var("CAS_FACTORY_NICE_LEVEL")
        .ok()
        .filter(|value| value.parse::<i32>().is_ok())
        .unwrap_or_else(|| "10".to_string())
}

fn first_output_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or("no diagnostic")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_coordinator_reads_next_day_merge_once_after_partial_line_and_filters_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let cas_dir = temp.path();
        let start_day = chrono::NaiveDate::from_ymd_opt(2026, 9, 14).unwrap();
        let next_day = chrono::NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
        let logs = cas_dir.join("logs");
        fs::create_dir_all(&logs).unwrap();

        let old_day_path = factory_session_log_path_for_date(cas_dir, start_day);
        fs::write(
            &old_day_path,
            "{\"event\":\"worktree_merged\",\"factory_session\":\"s1\",\"epic_id\":\"cas-old1\",\"target_branch\":\"epic/release\",\"target_tip\":\"old-tip\"}\n",
        )
        .unwrap();
        let mut coordinator = MergeSweepCoordinator::new_at(cas_dir, "s1", start_day);

        let next_day_path = factory_session_log_path_for_date(cas_dir, next_day);
        let other_session = r#"{"event":"worktree_merged","factory_session":"s2","epic_id":"cas-nope","target_branch":"epic/release","target_tip":"wrong-session"}"#;
        let expected = SweepRequest {
            epic_id: "cas-new1".to_owned(),
            target_branch: "epic/release".to_owned(),
            commit: "new-tip".to_owned(),
        };
        let own_event = r#"{"event":"worktree_merged","factory_session":"s1","epic_id":"cas-new1","target_branch":"epic/release","target_tip":"new-tip"}"#;
        fs::write(
            &next_day_path,
            format!("{other_session}\n{}", &own_event[..30]),
        )
        .unwrap();

        assert!(coordinator.read_merge_events_at(next_day).is_empty());
        use std::io::Write as _;
        OpenOptions::new()
            .append(true)
            .open(&next_day_path)
            .unwrap()
            .write_all(format!("{}\n", &own_event[30..]).as_bytes())
            .unwrap();

        assert_eq!(coordinator.read_merge_events_at(next_day), vec![expected]);
        assert!(coordinator.read_merge_events_at(next_day).is_empty());
    }

    #[test]
    fn merge_event_requires_current_session_epic_target_and_tip() {
        let line = r#"{"event":"worktree_merged","factory_session":"s1","epic_id":"cas-ab12","target_branch":"epic/release","commit":"old","target_tip":"new"}"#;
        assert_eq!(
            parse_merge_event(line, "s1"),
            Some(SweepRequest {
                epic_id: "cas-ab12".to_string(),
                target_branch: "epic/release".to_string(),
                commit: "new".to_string(),
            })
        );
        assert!(parse_merge_event(line, "other").is_none());
        assert!(parse_merge_event(&line.replace("epic/release", "main"), "s1").is_none());
        assert!(parse_merge_event(&line.replace("cas-ab12", "none"), "s1").is_none());
    }

    #[test]
    fn merge_event_falls_back_to_legacy_commit_field() {
        let line = r#"{"event":"worktree_merged","factory_session":"s1","epic_id":"cas-ab12","target_branch":"epic/release","commit":"abc123"}"#;
        assert_eq!(parse_merge_event(line, "s1").unwrap().commit, "abc123");
    }

    #[test]
    fn summary_preserves_failed_rows_and_summary_line() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(
            temp.path(),
            "running\nFAIL [ 0.1s] crate::broken\nSummary: 1 failed\n",
        )
        .unwrap();
        let (summary, failures) = summarize_log(temp.path());
        assert_eq!(summary, "Summary: 1 failed");
        assert_eq!(failures, vec!["FAIL [ 0.1s] crate::broken"]);
    }

    #[test]
    fn newer_merge_supersedes_only_a_different_tip() {
        let request = SweepRequest {
            epic_id: "cas-ab12".to_string(),
            target_branch: "epic/release".to_string(),
            commit: "new".to_string(),
        };
        let old = SweepRequest {
            commit: "old".to_string(),
            ..request.clone()
        };
        assert_ne!(old.commit, request.commit);
        assert_eq!(request.epic_id, old.epic_id);
    }

    #[test]
    fn runner_resolution_uses_the_declared_pnpm_test_script() {
        let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[("PNPM", None)]);
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"packageManager":"pnpm@10.4.0","scripts":{"test":"pnpm -r test"}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .unwrap();

        let runner = resolve_test_runner(temp.path()).unwrap();

        assert_eq!(runner.kind, TestRunnerKind::Package);
        assert_eq!(runner.program, "pnpm");
        assert_eq!(runner.args, ["test"]);
        assert_eq!(format_command(&runner.program, &runner.args), "pnpm test");
    }

    #[test]
    fn bun_runner_uses_package_script_not_bun_builtin_test_runner() {
        let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[("BUN", None)]);
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"packageManager":"bun@1.2.0","scripts":{"test":"node declared-test.mjs"}}"#,
        )
        .unwrap();
        fs::write(temp.path().join("bun.lock"), "{}").unwrap();

        let runner = resolve_test_runner(temp.path()).unwrap();

        assert_eq!(runner.kind, TestRunnerKind::Package);
        assert_eq!(runner.program, "bun");
        assert_eq!(runner.args, ["run", "test"]);
        assert_eq!(
            format_command(&runner.program, &runner.args),
            "bun run test"
        );
    }

    #[test]
    fn runner_resolution_rejects_missing_test_script_as_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"scripts":{"build":"vite build"}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .unwrap();

        let error = resolve_test_runner(temp.path()).unwrap_err();

        assert!(
            error.contains("package.json") && error.contains("scripts.test"),
            "{error}"
        );
        assert!(error.contains("sweep unavailable"), "{error}");
    }

    #[test]
    fn runner_resolution_rejects_unsupported_package_manager() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"packageManager":"deno@2.0.0","scripts":{"test":"deno test"}}"#,
        )
        .unwrap();

        let error = resolve_test_runner(temp.path()).unwrap_err();

        assert!(
            error.contains("unsupported package manager `deno`"),
            "{error}"
        );
        assert!(error.contains("npm, pnpm, yarn, or bun"), "{error}");
    }

    #[test]
    fn runner_resolution_preserves_configured_cargo_path() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("CARGO", "/owned/cargo")]);
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();

        let runner = resolve_test_runner(temp.path()).unwrap();

        assert_eq!(runner.kind, TestRunnerKind::Cargo);
        assert_eq!(runner.program, "/owned/cargo");
        assert_eq!(
            runner.args,
            ["nextest", "run", "--workspace", "--no-fail-fast"]
        );
    }

    #[test]
    fn missing_package_runner_is_not_spawned_as_a_test_failure() {
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("runner.log");
        let log = File::create(log_path).unwrap();
        let settings = SweepSettings::from(&FactoryConfig::default());
        let runner = TestRunner {
            kind: TestRunnerKind::Package,
            program: temp
                .path()
                .join("missing-pnpm")
                .to_string_lossy()
                .into_owned(),
            args: vec!["test".to_owned()],
            package_manager: Some("pnpm"),
        };

        assert!(spawn_test_runner(temp.path(), &settings, &log, &runner).is_none());
    }

    #[test]
    fn execute_sweep_reports_missing_runner_as_configuration_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(temp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {:?}", output.stderr);
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Runner Fixture"]);
        git(&["config", "user.email", "runner@example.invalid"]);
        fs::write(temp.path().join("README"), "no runner\n").unwrap();
        git(&["add", "README"]);
        git(&["commit", "-m", "missing runner fixture"]);
        let commit = git(&["rev-parse", "HEAD"]);

        let result = execute_sweep(
            temp.path(),
            &temp.path().join("cas-data"),
            SweepRequest {
                epic_id: "cas-no-runner".to_owned(),
                target_branch: "epic/no-runner".to_owned(),
                commit,
            },
            SweepSettings::from(&FactoryConfig::default()),
            Arc::new(AtomicBool::new(false)),
        );

        assert_eq!(result.status, SweepStatus::Unavailable);
        assert!(
            result.summary.contains("sweep unavailable"),
            "{}",
            result.summary
        );
        let log = fs::read_to_string(result.log_path).unwrap();
        assert_eq!(log.matches("sweep unavailable").count(), 1, "{log}");
        assert!(result.failures.is_empty());
    }

    #[test]
    fn yarn_pnp_loader_is_valid_dependency_setup_without_node_modules() {
        for loader in [".pnp.cjs", ".pnp.js"] {
            let temp = tempfile::tempdir().unwrap();
            fs::write(
                temp.path().join("package.json"),
                r#"{"packageManager":"yarn@4.9.2","dependencies":{"fixture-dependency":"1.0.0"},"scripts":{"test":"node test.mjs"}}"#,
            )
            .unwrap();
            fs::write(temp.path().join(loader), "// Yarn PnP loader\n").unwrap();
            let runner = TestRunner {
                kind: TestRunnerKind::Package,
                program: "yarn".to_owned(),
                args: vec!["test".to_owned()],
                package_manager: Some("yarn"),
            };

            assert!(
                missing_package_setup(temp.path(), &runner).is_none(),
                "{loader}"
            );
        }
    }

    #[test]
    fn yarn_pnp_without_loader_is_unavailable_with_install_remedy() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"packageManager":"yarn@4.9.2","dependencies":{"fixture-dependency":"1.0.0"},"scripts":{"test":"node test.mjs"}}"#,
        )
        .unwrap();
        let runner = TestRunner {
            kind: TestRunnerKind::Package,
            program: "yarn".to_owned(),
            args: vec!["test".to_owned()],
            package_manager: Some("yarn"),
        };

        let error = missing_package_setup(temp.path(), &runner).unwrap();
        assert!(error.contains("dependencies are not installed"), "{error}");
        assert!(error.contains("yarn install"), "{error}");
    }

    #[test]
    fn unrelated_pnp_loader_does_not_satisfy_non_yarn_dependency_setup() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"packageManager":"npm@11.0.0","dependencies":{"fixture-dependency":"1.0.0"},"scripts":{"test":"node test.mjs"}}"#,
        )
        .unwrap();
        fs::write(temp.path().join(".pnp.cjs"), "// unrelated loader\n").unwrap();
        let runner = TestRunner {
            kind: TestRunnerKind::Package,
            program: "npm".to_owned(),
            args: vec!["test".to_owned()],
            package_manager: Some("npm"),
        };

        let error = missing_package_setup(temp.path(), &runner).unwrap();
        assert!(error.contains("dependencies are not installed"), "{error}");
        assert!(error.contains("npm install"), "{error}");
    }

    #[test]
    fn npm_runner_executes_declared_script_with_real_dependency_in_detached_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(temp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {:?}", output.stderr);
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Runner Fixture"]);
        git(&["config", "user.email", "runner@example.invalid"]);
        fs::write(temp.path().join(".gitignore"), "node_modules/\n").unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"name":"runner-fixture","private":true,"packageManager":"npm@11.0.0","scripts":{"test":"node test-runner.mjs"},"dependencies":{"fixture-dependency":"file:fixture-dependency"}}"#,
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("fixture-dependency")).unwrap();
        fs::write(
            temp.path().join("fixture-dependency/package.json"),
            r#"{"name":"fixture-dependency","type":"module","exports":"./index.mjs"}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("fixture-dependency/index.mjs"),
            "export default 'dependency-loaded';\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("test-runner.mjs"),
            "import marker from 'fixture-dependency';\nimport { writeFileSync } from 'node:fs';\nwriteFileSync(process.env.SWEEP_RUNNER_RESULT, `${process.cwd()}\\n${marker}\\n`);\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "real package runner fixture"]);
        let commit = git(&["rev-parse", "HEAD"]);
        let request = SweepRequest {
            epic_id: "cas-real-npm".to_owned(),
            target_branch: "epic/real-npm".to_owned(),
            commit,
        };
        let detached = prepare_merge_worktree(temp.path(), &request).unwrap();
        fs::create_dir_all(detached.join("node_modules")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            "../fixture-dependency",
            detached.join("node_modules/fixture-dependency"),
        )
        .unwrap();
        #[cfg(not(unix))]
        fs::create_dir_all(detached.join("node_modules/fixture-dependency")).unwrap();
        #[cfg(not(unix))]
        fs::copy(
            detached.join("fixture-dependency/package.json"),
            detached.join("node_modules/fixture-dependency/package.json"),
        )
        .unwrap();
        #[cfg(not(unix))]
        fs::copy(
            detached.join("fixture-dependency/index.mjs"),
            detached.join("node_modules/fixture-dependency/index.mjs"),
        )
        .unwrap();
        let result_output = temp.path().join("runner-result");
        let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[
            ("NPM", None),
            ("SWEEP_RUNNER_RESULT", Some(result_output.to_str().unwrap())),
        ]);

        let result = execute_sweep(
            temp.path(),
            &temp.path().join("cas-data"),
            request,
            SweepSettings::from(&FactoryConfig::default()),
            Arc::new(AtomicBool::new(false)),
        );

        assert_eq!(result.status, SweepStatus::Passed, "{}", result.summary);
        let result_text = fs::read_to_string(result_output).unwrap();
        assert!(
            result_text.starts_with(&detached.display().to_string()),
            "{result_text}"
        );
        assert!(result_text.contains("dependency-loaded"), "{result_text}");
        let log = fs::read_to_string(result.log_path).unwrap();
        assert!(log.contains("sweep: npm test"), "{log}");
        assert!(result.failures.is_empty());
    }

    #[test]
    fn execute_sweep_reports_missing_dependencies_as_configuration_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(temp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {:?}", output.stderr);
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Runner Fixture"]);
        git(&["config", "user.email", "runner@example.invalid"]);
        fs::write(
            temp.path().join("package.json"),
            r#"{"packageManager":"pnpm@11.20.0","scripts":{"test":"node test.mjs"},"dependencies":{"fixture-dependency":"file:fixture-dependency"}}"#,
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("fixture-dependency")).unwrap();
        fs::write(
            temp.path().join("fixture-dependency/package.json"),
            r#"{"name":"fixture-dependency","type":"module"}"#,
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("node_modules")).unwrap();
        fs::write(temp.path().join("node_modules/.gitkeep"), "").unwrap();
        fs::write(
            temp.path().join("test.mjs"),
            "console.log('not reached');\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "missing dependencies fixture"]);
        let result = execute_sweep(
            temp.path(),
            &temp.path().join("cas-data"),
            SweepRequest {
                epic_id: "cas-no-deps".to_owned(),
                target_branch: "epic/no-deps".to_owned(),
                commit: git(&["rev-parse", "HEAD"]),
            },
            SweepSettings::from(&FactoryConfig::default()),
            Arc::new(AtomicBool::new(false)),
        );

        assert_eq!(result.status, SweepStatus::Unavailable);
        assert!(
            result.summary.contains("fixture-dependency")
                && result.summary.contains("not resolvable"),
            "{}",
            result.summary
        );
        assert!(
            result.summary.contains("pnpm install"),
            "{}",
            result.summary
        );
        assert!(result.failures.is_empty());
    }

    #[test]
    fn execute_sweep_allows_dependency_free_declared_script_without_node_modules() {
        let temp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(temp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {:?}", output.stderr);
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Runner Fixture"]);
        git(&["config", "user.email", "runner@example.invalid"]);
        fs::write(
            temp.path().join("package.json"),
            r#"{"packageManager":"npm@11.0.0","scripts":{"test":"node test.mjs"}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("test.mjs"),
            "console.log('dependency-free-declared-script');\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "dependency-free package runner fixture"]);
        let request = SweepRequest {
            epic_id: "cas-no-deps-script".to_owned(),
            target_branch: "epic/no-deps-script".to_owned(),
            commit: git(&["rev-parse", "HEAD"]),
        };
        let detached = prepare_merge_worktree(temp.path(), &request).unwrap();
        assert!(!detached.join("node_modules").exists());
        let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[("NPM", None)]);

        let result = execute_sweep(
            temp.path(),
            &temp.path().join("cas-data"),
            request,
            SweepSettings::from(&FactoryConfig::default()),
            Arc::new(AtomicBool::new(false)),
        );

        assert_eq!(result.status, SweepStatus::Passed, "{}", result.summary);
        let log = fs::read_to_string(result.log_path).unwrap();
        assert!(log.contains("sweep: npm test"), "{log}");
        assert!(log.contains("dependency-free-declared-script"), "{log}");
    }

    #[test]
    fn execute_sweep_classifies_declared_script_failure_as_test_failure() {
        let temp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(temp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {:?}", output.stderr);
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Runner Fixture"]);
        git(&["config", "user.email", "runner@example.invalid"]);
        fs::write(
            temp.path().join("package.json"),
            r#"{"packageManager":"npm@11.0.0","scripts":{"test":"node test.mjs"}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("test.mjs"),
            "console.error('declared-script-failure'); process.exit(17);\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "failing package runner fixture"]);
        let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[("NPM", None)]);

        let result = execute_sweep(
            temp.path(),
            &temp.path().join("cas-data"),
            SweepRequest {
                epic_id: "cas-test-failure".to_owned(),
                target_branch: "epic/test-failure".to_owned(),
                commit: git(&["rev-parse", "HEAD"]),
            },
            SweepSettings::from(&FactoryConfig::default()),
            Arc::new(AtomicBool::new(false)),
        );

        assert_eq!(result.status, SweepStatus::Failed, "{}", result.summary);
        assert_ne!(result.status, SweepStatus::Unavailable);
        let log = fs::read_to_string(result.log_path).unwrap();
        assert!(log.contains("sweep: npm test"), "{log}");
        assert!(log.contains("declared-script-failure"), "{log}");
    }
}
