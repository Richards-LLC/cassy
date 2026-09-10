//! Bounded validation of the merged epic tip.
//!
//! `worktree_merge` only records a durable event.  The daemon tails those
//! events and runs one detached checkout per epic so the MCP merge request is
//! not held open by a workspace-sized nextest run.  A newer merge supersedes
//! the older run; at most one sweep is active for an epic at a time.

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
use tokio::task::JoinHandle;

use crate::config::FactoryConfig;

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
}

impl From<&FactoryConfig> for SweepSettings {
    fn from(config: &FactoryConfig) -> Self {
        Self {
            enabled: config.merge_sweep,
            timeout: Duration::from_secs(config.merge_sweep_timeout_secs.max(1)),
            cargo_build_jobs: config.cargo_build_jobs.clone(),
            nice_cargo: config.nice_cargo,
            max_concurrent_builders: config.max_concurrent_builders,
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
        }
    }

    pub(super) async fn poll(
        &mut self,
        project_root: &Path,
        cas_dir: &Path,
        config: &FactoryConfig,
    ) {
        let mut requests = self.read_merge_events();
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
                },
            };
            let superseded = active.pending.is_some() || result.status == SweepStatus::Superseded;
            if !superseded {
                self.record_result(cas_dir, &result);
            }
            if let Some(next) = active.pending.take() {
                pending.push(next);
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
        if let Some(active) = self.active.get_mut(&request.epic_id) {
            if active.request.commit != request.commit {
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

        let guard = crate::factory_build_guard::inspect(cas_dir, &settings_to_config(settings), 1);
        if !guard.violations().is_empty() {
            self.record_deferred(cas_dir, &request, &guard.violations().join("; "));
            self.completed
                .insert(request.epic_id.clone(), request.commit.clone());
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let project_root = project_root.to_path_buf();
        let cas_dir = cas_dir.to_path_buf();
        let sweep_settings = settings.clone();
        let worker_request = request.clone();
        let handle = tokio::task::spawn_blocking(move || {
            execute_sweep(
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
            request.epic_id.clone(),
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
        };
        append_epic_note(cas_dir, &result);
        tracing::warn!(epic = %request.epic_id, reason, "post-merge workspace sweep deferred");
    }

    fn record_result(&self, cas_dir: &Path, result: &SweepResult) {
        if result.status == SweepStatus::Superseded {
            tracing::debug!(epic = %result.request.epic_id, commit = %result.request.commit, "post-merge sweep superseded");
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
            };
        }
    };
    let _ = writeln!(
        log,
        "sweep: cargo nextest run --workspace --no-fail-fast\nworktree: {}\ntarget: {}",
        worktree.display(),
        request.commit
    );
    let Some(mut child) = spawn_nextest(&worktree, &settings, &log) else {
        let summary = "could not start cargo nextest".to_string();
        let _ = writeln!(log, "{summary}");
        return SweepResult {
            request,
            status: SweepStatus::SetupFailed,
            log_path,
            summary,
            failures: Vec::new(),
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
                let _ = writeln!(log, "failed polling cargo nextest: {error}");
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

fn spawn_nextest(worktree: &Path, settings: &SweepSettings, log: &File) -> Option<Child> {
    let cargo_binary = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut command = if settings.nice_cargo {
        let level = nice_level();
        let mut command = Command::new("nice");
        command.args(["-n", level.as_str()]).arg("cargo");
        command
    } else {
        Command::new(cargo_binary)
    };
    command
        .current_dir(worktree)
        .args(["nextest", "run", "--workspace", "--no-fail-fast"])
        .env("CARGO_BUILD_JOBS", effective_build_jobs(settings))
        .stdout(Stdio::from(log.try_clone().ok()?))
        .stderr(Stdio::from(log.try_clone().ok()?));
    command.spawn().ok()
}

fn terminate_child(child: &mut Child) {
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
        summary = "cargo nextest completed; see sweep log".to_string();
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
    format!("{}{}", result.summary, failures)
}

fn status_text(status: SweepStatus) -> &'static str {
    match status {
        SweepStatus::Passed => "PASSED",
        SweepStatus::Failed => "FAILED",
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
}
