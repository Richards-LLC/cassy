//! Durable ownership and completion of capped detached worker commands.
//! Queue acceptance is idempotent; an interrupted notification retries on the
//! daemon's existing reminder scan, without changing the command's exit status.
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Run {
    id: String,
    worker_id: String,
    worker: String,
    session: Option<String>,
    task: Option<String>,
    repo: PathBuf,
    log: PathBuf,
    log_offset: u64,
    started_at: DateTime<Utc>,
    pid: Option<u32>,
    fingerprint: Option<u64>,
    finished_at: Option<DateTime<Utc>>,
    exit_code: Option<i32>,
    notified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Completion {
    pub id: String,
    pub at: DateTime<Utc>,
    pub exit_code: Option<i32>,
    pub log: PathBuf,
}
impl Completion {
    pub(crate) fn detail(&self) -> String {
        let status = self
            .exit_code
            .map(|code| format!("EXIT {code}"))
            .unwrap_or_else(|| "exit status unavailable".into());
        format!(
            "Detached job {} exited (observed {}) with {status}. Log: {}",
            self.id,
            self.at.to_rfc3339(),
            self.log.display()
        )
    }
}

#[derive(Default)]
pub(crate) struct Snapshot {
    pub running: bool,
    pub completion: Option<Completion>,
}
fn directory(root: &Path) -> PathBuf {
    root.join("worker-runs")
}
fn path(root: &Path, id: &str) -> Result<PathBuf> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid detached worker run id");
    }
    Ok(directory(root).join(format!("{id}.json")))
}
fn lock(root: &Path) -> Result<File> {
    std::fs::create_dir_all(directory(root))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory(root).join("state.lock"))?;
    file.lock_exclusive()?;
    Ok(file)
}
fn save(root: &Path, run: &Run) -> Result<()> {
    let mut temporary = tempfile::NamedTempFile::new_in(directory(root))?;
    serde_json::to_writer(temporary.as_file_mut(), run)?;
    temporary.persist(path(root, &run.id)?)?;
    Ok(())
}
fn load(root: &Path, id: &str) -> Result<Run> {
    Ok(serde_json::from_slice(&std::fs::read(path(root, id)?)?)?)
}
fn entries(root: &Path) -> Result<Vec<Run>> {
    let files = match std::fs::read_dir(directory(root)) {
        Ok(files) => files,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut runs = Vec::new();
    for entry in files {
        let file = entry?.path();
        if file.extension().is_some_and(|ext| ext == "json") {
            match std::fs::read(&file)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| Ok(serde_json::from_slice::<Run>(&bytes)?))
            {
                Ok(run) => runs.push(run),
                Err(error) => tracing::warn!(%error, "unreadable detached worker run record"),
            }
        }
    }
    Ok(runs)
}

/// Only a distinct live worker owning this checkout can receive a result.
/// Standalone capped commands keep their existing log-only behaviour.
pub(crate) fn register(
    root: &Path,
    repo: &Path,
    log: &Path,
    now: DateTime<Utc>,
) -> Result<Option<String>> {
    let repo = repo.canonicalize()?;
    let agents = crate::store::open_agent_store(root)?;
    let mut owners = agents
        .list(None)?
        .into_iter()
        .filter(|agent| {
            agent.role == cas_types::AgentRole::Worker
                && agent.is_alive()
                && agent.machine_id == Some(cas_types::Agent::get_or_generate_machine_id())
                && agent
                    .metadata
                    .get("clone_path")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| root.join("worktrees").join(&agent.name))
                    .canonicalize()
                    .ok()
                    .as_ref()
                    == Some(&repo)
        })
        .collect::<Vec<_>>();
    if owners.len() > 1 {
        bail!("detached run has ambiguous checkout ownership");
    }
    let Some(owner) = owners.pop() else {
        return Ok(None);
    };
    let leases = agents.list_agent_leases(&owner.id)?;
    let task = leases
        .into_iter()
        .find(|lease| lease.expires_at > now)
        .map(|lease| lease.task_id);
    let id = uuid::Uuid::new_v4().simple().to_string();
    let run = Run {
        id: id.clone(),
        worker_id: owner.id,
        worker: owner.name,
        session: owner.factory_session,
        task,
        repo,
        log: log.canonicalize()?,
        log_offset: std::fs::metadata(log)?.len(),
        started_at: now,
        pid: None,
        fingerprint: None,
        finished_at: None,
        exit_code: None,
        notified: false,
    };
    let _lock = lock(root)?;
    save(root, &run)?;
    Ok(Some(id))
}

pub(crate) fn bind(root: &Path, id: &str, pid: u32) -> Result<()> {
    let _lock = lock(root)?;
    let mut run = load(root, id)?;
    run.pid = Some(pid);
    run.fingerprint = crate::mcp::daemon::read_pid_starttime(pid);
    save(root, &run)
}
fn alive(run: &Run, now: DateTime<Utc>) -> bool {
    run.finished_at.is_none()
        && run.pid.map_or_else(
            || now.signed_duration_since(run.started_at) < chrono::Duration::minutes(2),
            |pid| {
                run.fingerprint.map_or_else(
                    || crate::mcp::daemon::pid_alive(pid),
                    |fingerprint| crate::mcp::daemon::pid_matches_fingerprint(pid, fingerprint),
                )
            },
        )
}
pub(crate) fn snapshot(
    root: &Path,
    worker: &str,
    task: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Snapshot> {
    let agents = crate::store::open_agent_store(root)?;
    let mut result = Snapshot::default();
    for run in entries(root)? {
        if run.worker != worker || task.is_some_and(|task| run.task.as_deref() != Some(task)) {
            continue;
        }
        let current = agents
            .get(&run.worker_id)
            .ok()
            .is_some_and(|agent| agent.is_alive() && agent.factory_session == run.session);
        if !current {
            continue;
        }
        result.running |= alive(&run, now);
        if let Some(at) = run.finished_at
            && result
                .completion
                .as_ref()
                .is_none_or(|previous| at > previous.at)
        {
            result.completion = Some(Completion {
                id: run.id,
                at,
                exit_code: run.exit_code,
                log: run.log,
            });
        }
    }
    Ok(result)
}

pub(crate) fn finish(root: &Path, id: &str, code: i32, now: DateTime<Utc>) -> Result<()> {
    let _lock = lock(root)?;
    let mut run = load(root, id)?;
    if run.finished_at.is_none() {
        run.finished_at = Some(now);
        run.exit_code = Some(code);
        save(root, &run)?;
    }
    notify(root, &mut run)
}

fn notify(_root: &Path, _run: &mut Run) -> Result<()> {
    Ok(())
}

/// Executes the same capped producer; notification failure never changes its result.
pub(crate) fn monitor(root: &Path, id: &str, execute: impl FnOnce() -> Result<()>) -> Result<()> {
    let run = load(root, id)?;
    if std::env::current_dir()?.canonicalize()? != run.repo {
        bail!("detached completion belongs to another checkout");
    }
    let result = execute();
    if let Err(error) = finish(root, id, if result.is_ok() { 0 } else { 1 }, Utc::now()) {
        eprintln!("worker-check completion pending daemon retry: {error}");
    }
    result
}

pub(crate) fn reconcile(root: &Path, session: Option<&str>, now: DateTime<Utc>) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let _lock = lock(root)?;
    for mut run in entries(root)? {
        if session.is_some() && run.session.as_deref() != session {
            continue;
        }
        if run.finished_at.is_none() && !alive(&run, now) {
            // A killed runner cannot call finish. Its session wrapper still
            // records EXIT; fence the bounded tail to this launch's log offset.
            let code = (|| -> Result<Option<i32>> {
                let mut file = File::open(&run.log)?;
                let start = file
                    .metadata()?
                    .len()
                    .saturating_sub(4096)
                    .max(run.log_offset);
                file.seek(SeekFrom::Start(start))?;
                let mut tail = String::new();
                file.take(4096).read_to_string(&mut tail)?;
                Ok(tail.lines().rev().find_map(|line| {
                    line.strip_prefix("worker-check: EXIT ")?
                        .trim()
                        .parse()
                        .ok()
                }))
            })()
            .unwrap_or(None);
            run.finished_at = Some(now);
            run.exit_code = code;
            save(root, &run)?;
        }
        if run.finished_at.is_some() && !run.notified {
            notify(root, &mut run)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::AgentStore;

    #[test]
    fn registered_job_notifies_its_worker_once_with_exit_and_log() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let repo = env.home().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let root = crate::store::init_cas_dir(&repo).unwrap();
        env.set_current_dir(&repo);
        let agents = crate::store::open_agent_store(&root).unwrap();
        let mut worker = cas_types::Agent::new_with_role(
            "run-worker-id".into(),
            "run-worker".into(),
            cas_types::AgentRole::Worker,
        );
        worker.factory_session = Some("run-session".into());
        worker.machine_id = Some(cas_types::Agent::get_or_generate_machine_id());
        worker
            .metadata
            .insert("clone_path".into(), repo.display().to_string());
        agents.register(&worker).unwrap();
        let log = repo.join("run.log");
        std::fs::write(&log, "").unwrap();
        let now = Utc::now();
        let id = register(&root, &repo, &log, now).unwrap().unwrap();
        bind(&root, &id, std::process::id()).unwrap();
        assert!(snapshot(&root, "run-worker", None, now).unwrap().running);
        assert!(
            !snapshot(&root, "another-worker", None, now)
                .unwrap()
                .running
        );
        let result = monitor(&root, &id, || bail!("fixture command failed"));
        assert!(result.is_err(), "keep the producer exit result");
        let completed = snapshot(&root, "run-worker", None, now).unwrap();
        assert!(!completed.running);
        let completion = completed.completion.unwrap();
        assert_eq!(completion.exit_code, Some(1));
        assert_eq!(completion.log, log.canonicalize().unwrap());
        finish(&root, &id, 0, now).unwrap();
        reconcile(&root, Some("run-session"), now).unwrap();
        let queue = crate::store::open_prompt_queue_store(&root).unwrap();
        let rows = queue
            .peek_for_targets(&["run-worker"], Some("run-session"), 10)
            .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "a completed detached run owes its worker one durable wake"
        );
        assert!(
            rows[0].prompt.contains("EXIT 1") && rows[0].prompt.contains(log.to_str().unwrap())
        );
        assert_eq!(rows[0].origin, Some(cas_store::QueueOrigin::Daemon));
    }
}
