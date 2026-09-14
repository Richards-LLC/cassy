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
    log_path: PathBuf,
    session_name: String,
    offset: u64,
    active: HashMap<String, ActiveSweep>,
    completed: HashMap<String, String>,
    retry_after: Option<(Instant, SweepRequest)>,
}

impl MergeSweepCoordinator {
    pub(crate) fn new(cas_dir: &Path, session_name: &str) -> Self {
        let log_path = factory_session_log_path(cas_dir);
        let offset = fs::metadata(&log_path).map(|meta| meta.len()).unwrap_or(0);
        Self {
            log_path,
            session_name: session_name.to_string(),
            offset,
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
            self.schedule(project_root, cas_dir, request, &settings);
        }
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
        let Ok(bytes) = fs::read(&self.log_path) else {
            return Vec::new();
        };
        if self.offset > bytes.len() as u64 {
            self.offset = 0;
        }
        let start = self.offset as usize;
        let Some(last_newline) = bytes[start..].iter().rposition(|byte| *byte == b'\n') else {
            return Vec::new();
        };
        let consumed = start + last_newline + 1;
        self.offset = consumed as u64;
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

impl crate::ui::factory::daemon::FactoryDaemon {
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

fn factory_session_log_path(cas_dir: &Path) -> PathBuf {
    cas_dir.join("logs").join(format!(
        "factory-session-{}.log",
        chrono::Utc::now().format("%Y-%m-%d")
    ))
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
    Ok(TestRunner {
        kind: TestRunnerKind::Package,
        program,
        args: vec!["test".to_owned()],
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
        };

        assert!(spawn_test_runner(temp.path(), &settings, &log, &runner).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn execute_sweep_runs_pnpm_from_the_detached_target_worktree() {
        use std::os::unix::fs::PermissionsExt;

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
            r#"{"packageManager":"pnpm@10.4.0","scripts":{"test":"pnpm -r test"}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .unwrap();
        git(&["add", "package.json", "pnpm-lock.yaml"]);
        git(&["commit", "-m", "package runner fixture"]);
        let commit = git(&["rev-parse", "HEAD"]);
        let runner_script = temp.path().join("pnpm-fixture.sh");
        let cwd_output = temp.path().join("runner-cwd");
        let args_output = temp.path().join("runner-args");
        fs::write(
            &runner_script,
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" > \"$SWEEP_RUNNER_CWD\"\nprintf '%s\\n' \"$@\" > \"$SWEEP_RUNNER_ARGS\"\necho 'Summary: 1 passed'\n",
        )
        .unwrap();
        fs::set_permissions(&runner_script, fs::Permissions::from_mode(0o755)).unwrap();
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("PNPM", runner_script.to_str().unwrap()),
            ("SWEEP_RUNNER_CWD", cwd_output.to_str().unwrap()),
            ("SWEEP_RUNNER_ARGS", args_output.to_str().unwrap()),
        ]);
        let cas_dir = temp.path().join("cas-data");
        let settings = SweepSettings::from(&FactoryConfig::default());
        let result = execute_sweep(
            temp.path(),
            &cas_dir,
            SweepRequest {
                epic_id: "cas-pnpm".to_owned(),
                target_branch: "epic/pnpm".to_owned(),
                commit,
            },
            settings,
            Arc::new(AtomicBool::new(false)),
        );

        assert_eq!(result.status, SweepStatus::Passed, "{}", result.summary);
        let expected_worktree = temp.path().join(".cas/epic-cas-pnpm-merge");
        assert_eq!(
            fs::read_to_string(cwd_output).unwrap().trim(),
            expected_worktree.display().to_string()
        );
        assert_eq!(fs::read_to_string(args_output).unwrap().trim(), "test");
        let log = fs::read_to_string(result.log_path).unwrap();
        assert!(log.contains("sweep: "), "{log}");
        assert!(log.contains(" test\nworktree: "), "{log}");
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

    #[cfg(unix)]
    #[test]
    fn pnpm_runner_executes_in_target_project_cwd() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"packageManager":"pnpm@10.4.0","scripts":{"test":"echo declared"}}"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )
        .unwrap();
        let runner_script = temp.path().join("pnpm-fixture.sh");
        let cwd_output = temp.path().join("runner-cwd");
        let args_output = temp.path().join("runner-args");
        fs::write(
            &runner_script,
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" > \"$SWEEP_RUNNER_CWD\"\nprintf '%s\\n' \"$@\" > \"$SWEEP_RUNNER_ARGS\"\necho 'Summary: 1 passed'\n",
        )
        .unwrap();
        fs::set_permissions(&runner_script, fs::Permissions::from_mode(0o755)).unwrap();
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("PNPM", runner_script.to_str().unwrap()),
            ("SWEEP_RUNNER_CWD", cwd_output.to_str().unwrap()),
            ("SWEEP_RUNNER_ARGS", args_output.to_str().unwrap()),
        ]);
        let runner = resolve_test_runner(temp.path()).unwrap();
        let log_path = temp.path().join("runner.log");
        let log = File::create(&log_path).unwrap();
        let settings = SweepSettings::from(&FactoryConfig::default());
        let mut child = spawn_test_runner(temp.path(), &settings, &log, &runner).unwrap();

        assert!(child.wait().unwrap().success());
        assert_eq!(
            fs::read_to_string(cwd_output).unwrap().trim(),
            temp.path().display().to_string()
        );
        assert_eq!(fs::read_to_string(args_output).unwrap().trim(), "test");
        assert_eq!(
            fs::read_to_string(log_path).unwrap().trim(),
            "Summary: 1 passed"
        );
    }
}
