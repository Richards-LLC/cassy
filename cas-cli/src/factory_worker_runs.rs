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
    let workers: Vec<_> = agents
        .list(None)?
        .into_iter()
        .filter(|agent| {
            agent.role == cas_types::AgentRole::Worker
                && agent.is_alive()
                && agent.machine_id == Some(cas_types::Agent::get_or_generate_machine_id())
        })
        .collect();
    // cas-c152: a run anywhere inside a worker's checkout (e.g. its hub-web
    // package) is that worker's; the deepest checkout wins over an enclosing one.
    let clone = |agent: &cas_types::Agent| {
        agent.metadata.get("clone_path").map(PathBuf::from)
            .unwrap_or_else(|| root.join("worktrees").join(&agent.name))
            .canonicalize().ok()
    };
    let depth = workers.iter()
        .filter_map(|agent| clone(agent).filter(|path| repo.starts_with(path)))
        .map(|path| path.components().count())
        .max();
    let mut owners: Vec<_> = workers.iter()
        .filter(|agent| clone(agent).is_some_and(|path| {
            repo.starts_with(&path) && Some(path.components().count()) == depth
        }))
        .cloned()
        .collect();
    // A task-owned scratch checkout belongs to that task's live assignee.
    let scratch_task = owners.is_empty()
        .then(|| crate::factory_review_scratch::managed_checkout_task(root, &repo))
        .flatten();
    if let Some(task_id) = &scratch_task {
        let task = crate::store::open_task_store_local(root)?.get(task_id)?;
        owners = workers.iter()
            .filter(|agent| task.assignee.as_deref()
                .is_some_and(|assignee| assignee == agent.id || assignee == agent.name))
            .cloned()
            .collect();
    }
    if owners.len() > 1 {
        bail!("detached run has ambiguous checkout ownership");
    }
    let Some(owner) = owners.pop() else {
        return Ok(None);
    };
    let leases = agents.list_agent_leases(&owner.id)?;
    let mut task = scratch_task.or_else(|| leases
        .into_iter()
        .find(|lease| lease.expires_at > now)
        .map(|lease| lease.task_id));
    if task.is_none() {
        let tasks = crate::store::open_task_store_local(root)?;
        let owned = tasks
            .list(Some(cas_types::TaskStatus::InProgress))?
            .into_iter()
            .filter(|task| {
                task.assignee
                    .as_deref()
                    .is_some_and(|assignee| assignee == owner.id || assignee == owner.name)
            })
            .collect::<Vec<_>>();
        if owned.len() == 1 {
            task = Some(owned[0].id.clone());
        }
    }
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

/// The queue can wait only for this session's current worker and its actual
/// live producer. An unknown or leaked lease retains the normal refusal.
pub(crate) fn recycle_waits_for_build(root: &Path, worker: &str, session: &str) -> Result<bool> {
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    let Some(owner) = agents.iter().filter(|agent| {
        agent.name == worker
            && agent.role == cas_types::AgentRole::Worker
            && agent.factory_session.as_deref() == Some(session)
            && agent.machine_id == Some(cas_types::Agent::get_or_generate_machine_id())
    }).max_by_key(|agent| agent.registered_at) else {
        return Ok(false);
    };
    let repo = owner.metadata.get("clone_path").map(PathBuf::from)
        .unwrap_or_else(|| root.join("worktrees").join(worker));
    // Non-isolated workers need not have a conventional private checkout.
    if !repo.exists() { return Ok(false); }
    let repo = repo.canonicalize()?;
    if repo.parent() != root.join("worktrees").canonicalize().ok().as_deref() {
        return Ok(false);
    }
    if let Ok(Some((pid, fingerprint))) = crate::factory_target_cache::owner::builder_identity(root, &repo)
        && entries(root)?.iter().any(|run| {
            run.worker_id == owner.id && run.worker == worker
                && run.session.as_deref() == Some(session) && run.repo == repo
                && run.finished_at.is_none() && run.exit_code.is_none()
                && run.pid == Some(pid) && run.fingerprint == Some(fingerprint)
        })
    {
        return Ok(true);
    }
    crate::factory_target_cache::owner::for_retirement(root, &repo)?;
    Ok(false)
}

/// Stop the registered producer before recycle can kill its worker cgroup.
/// Cargo may be in a sibling scope, so killing only the producer strands its
/// inherited target lease. The producer's TERM handler drains that tree first.
pub(crate) async fn stop_before_recycle(root: &Path, workers: Vec<String>) -> Result<()> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let runs: Vec<_> = entries(&root)?.into_iter()
            .filter(|run| workers.contains(&run.worker_id) && run.exit_code.is_none())
            .collect();
        for run in &runs {
            if run.finished_at.is_some() { continue; }
            let (Some(pid), Some(fingerprint)) = (run.pid, run.fingerprint) else {
                bail!("Detached run {} has not bound its process identity; retry recycle after launch", run.id);
            };
            if !crate::mcp::daemon::pid_matches_fingerprint(pid, fingerprint) { continue; }
            #[cfg(unix)]
            {
                // Signal only the durably recorded process incarnation. Its
                // handler owns Cargo's separate process group and grace period.
                if unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0
                    && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
            loop {
                if load(&root, &run.id)?.finished_at.is_some()
                    || !crate::mcp::daemon::pid_matches_fingerprint(pid, fingerprint)
                { break; }
                if std::time::Instant::now() >= deadline {
                    bail!("Detached run {} (pid {pid}) still owns its build; recycle refused before worker teardown", run.id);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        for run in runs {
            // Also catch a previously lost producer with inherited survivors.
            // An unverifiable or still-held lease refuses teardown; never kill
            // descendants guessed from a stale PID or another worker's name.
            crate::factory_target_cache::owner::for_retirement(&root, &run.repo)?;
        }
        Ok(())
    }).await?
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

fn notify(root: &Path, run: &mut Run) -> Result<()> {
    if run.notified {
        return Ok(());
    }
    let agents = crate::store::open_agent_store(root)?;
    let current = agents.get(&run.worker_id).ok().is_some_and(|agent| {
        agent.role == cas_types::AgentRole::Worker
            && agent.is_alive()
            && agent.name == run.worker
            && agent.factory_session == run.session
    });
    if !current {
        return Ok(());
    }
    let completion = Completion {
        id: run.id.clone(),
        at: run
            .finished_at
            .ok_or_else(|| anyhow::anyhow!("run has not finished"))?,
        exit_code: run.exit_code,
        log: run.log.clone(),
    };
    let body = format!(
        "{}\nTask: {}\nRead the log and continue the assigned task; obey any recorded admission retry time. This is a command completion, not a task verdict.",
        completion.detail(),
        run.task.as_deref().unwrap_or("not bound at launch")
    );
    let queue = crate::store::open_prompt_queue_store(root)?;
    queue.enqueue_idempotent(
        "worker-check",
        &run.worker,
        &body,
        run.session.as_deref(),
        Some("Detached worker run finished"),
        None,
        &format!("worker-run-complete:{}:{}", run.worker_id, run.id),
        Some(&cas_store::QueueOrigin::Daemon),
    )?;
    run.notified = true;
    save(root, run)?;
    crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(root);
    Ok(())
}

/// Executes the same capped producer; notification failure never changes its result.
pub(crate) fn monitor(root: &Path, id: &str, execute: impl FnOnce() -> Result<()>) -> Result<()> {
    let run = load(root, id)?;
    if std::env::current_dir()?.canonicalize()? != run.repo {
        bail!("detached completion belongs to another checkout");
    }
    bind(root, id, std::process::id())?;
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

    fn fixture() -> (crate::test_support::TestEnvGuard, PathBuf, PathBuf, PathBuf) {
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
        (env, root, repo, log)
    }

    #[test]
    fn registered_job_notifies_its_worker_once_with_exit_and_log() {
        let (_env, root, repo, log) = fixture();
        let now = Utc::now();
        let id = register(&root, &repo, &log, now).unwrap().unwrap();
        bind(&root, &id, std::process::id()).unwrap();
        assert!(snapshot(&root, "run-worker", None, now).unwrap().running);
        assert!(
            !snapshot(&root, "another-worker", None, now)
                .unwrap()
                .running
        );
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "cas",
            "factory",
            "worker-check",
            "--cas-root",
            root.to_str().unwrap(),
            "--completion-id",
            &id,
            "--",
            "invalid-producer-request",
        ])
        .unwrap();
        let Some(crate::cli::Commands::Factory(args)) = &cli.command else {
            panic!("factory command");
        };
        let result = crate::cli::factory::execute(args, &cli, Some(&root));
        assert!(result.is_err(), "keep the producer exit result");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Only package-scoped"),
            "execute the real capped producer validation"
        );
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
    /// cas-c152 (GH #2094): direct `--detach` runs from a worker's package
    /// directory (cas-6af4, worktree/hub-web) or from its task-owned scratch
    /// checkout (cas-fbff) registered nothing, so their completion never woke
    /// the worker. Both belong to that worker; the scratch run to its task.
    /// The scratch run goes through the real detached launch: this test binary
    /// stands in for `cas` and its capped producer runs a stand-in cargo.
    #[test]
    fn detached_runs_in_a_worker_package_or_its_task_scratch_wake_it_cas_c152() {
        const NAME: &str = "factory_worker_runs::tests::detached_runs_in_a_worker_package_or_its_task_scratch_wake_it_cas_c152";
        let argv = std::env::var("CAS_C152_ARGV").unwrap_or_default();
        match std::env::var("CAS_C152_CHILD").as_deref() {
            // `worker-check --detach > log 2>&1`, run from the scratch checkout.
            Ok("launch") => {
                let root = PathBuf::from(std::env::var("CAS_C152_ROOT").unwrap());
                let shim = PathBuf::from(std::env::var("CAS_C152_SHIM").unwrap());
                let args = ["nextest", "run", "-p", "cas", "--lib", "-E", "test(=cas_c152_fixture)"].map(str::to_owned);
                crate::factory_worker_check::execute_detached_with(&shim, &root, &args, None, &[]).unwrap();
                return;
            }
            // The detached runner's `cas factory worker-check --completion-id <id> -- <args>`.
            Ok("produce") => {
                let argv: Vec<String> = argv.lines().map(str::to_owned).collect();
                let after = |flag: &str| argv[argv.iter().position(|arg| arg == flag).unwrap() + 1].clone();
                let root = PathBuf::from(after("--cas-root"));
                let id = after("--completion-id");
                let args = argv[argv.iter().position(|arg| arg == "--").unwrap() + 1..].to_vec();
                let cwd = std::env::current_dir().unwrap();
                let cargo = root.join("fake-cargo");
                let result = monitor(&root, &id, || {
                    crate::factory_worker_check::execute_with_cargo_for_test(&root, &args, &cwd, &cargo)
                });
                std::process::exit(i32::from(result.is_err()));
            }
            _ => {}
        }
        let (mut env, root, repo, log) = fixture();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let lane = root.join("worktrees").join("run-worker");
        std::fs::create_dir_all(lane.join("hub-web")).unwrap();
        let agents = crate::store::open_agent_store(&root).unwrap();
        let mut worker = agents.get("run-worker-id").unwrap();
        worker.metadata.insert("clone_path".into(), lane.display().to_string());
        agents.update(&worker).unwrap();
        let now = Utc::now();
        let package = register(&root, &lane.join("hub-web"), &log, now).unwrap();

        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&repo)
            .status().unwrap().success(), "git {args:?}");
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".gitignore"), ".cas/\nrun.log\n").unwrap();
        git(&["add", ".gitignore"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "base"]);
        let head = String::from_utf8(std::process::Command::new("git").args(["rev-parse", "HEAD"])
            .current_dir(&repo).output().unwrap().stdout).unwrap().trim().to_string();
        let mut task = cas_types::Task::new("cas-scratch".into(), "Scratch proof".into());
        task.status = cas_types::TaskStatus::InProgress;
        task.assignee = Some("run-worker".into());
        crate::store::open_task_store(&root).unwrap().add(&task).unwrap();
        let checkout = crate::factory_review_scratch::create(&root, &repo, "cas-scratch", None, &head).unwrap();

        use std::os::unix::fs::PermissionsExt;
        let script = |path: &Path, body: &str| {
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script(&root.join("fake-cargo"), "printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\\n'");
        let shim = root.join("cas-stand-in");
        script(&shim, &format!(
            "CAS_C152_ARGV=$(printf '%s\\n' \"$@\") CAS_C152_CHILD=produce exec \"$CAS_C152_EXE\" --exact {NAME} --nocapture --test-threads=1"
        ));
        let scratch_log = root.join("scratch-run.log");
        let output = std::fs::File::create(&scratch_log).unwrap();
        let exe = std::env::current_exe().unwrap();
        let launched = std::process::Command::new(&exe)
            .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
            .current_dir(&checkout.path)
            .env("CAS_C152_CHILD", "launch")
            .env("CAS_C152_ROOT", &root)
            .env("CAS_C152_SHIM", &shim)
            .env("CAS_C152_EXE", &exe)
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .status()
            .unwrap();
        assert!(launched.success(), "{}", std::fs::read_to_string(&scratch_log).unwrap_or_default());
        // The detached run records its own completion; wait for that record.
        let scratch_repo = checkout.path.canonicalize().unwrap();
        let scratch_runs = || -> Vec<Run> {
            entries(&root).unwrap().into_iter().filter(|run| run.repo == scratch_repo).collect()
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !scratch_runs().iter().any(|run| run.finished_at.is_some()) {
            assert!(std::time::Instant::now() < deadline, "the detached run never finished: {}",
                std::fs::read_to_string(&scratch_log).unwrap_or_default());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let runs = scratch_runs();
        // Both shapes are judged together, so a red run reports each one.
        assert_eq!((package.is_some(), runs.len()), (true, 1), "package run, scratch run tracked");
        assert_eq!(load(&root, &package.clone().unwrap()).unwrap().worker, "run-worker");
        let run = &runs[0];
        assert_eq!((run.worker.as_str(), run.task.as_deref()), ("run-worker", Some("cas-scratch")));
        assert!(run.pid.is_some() && run.finished_at.is_some() && run.exit_code.is_some(), "{run:?}\n{}",
            std::fs::read_to_string(&scratch_log).unwrap_or_default());
        finish(&root, &package.unwrap(), 0, now).unwrap();
        reconcile(&root, Some("run-session"), now).unwrap();
        reconcile(&root, Some("run-session"), now).unwrap();
        let rows = crate::store::open_prompt_queue_store(&root).unwrap()
            .peek_for_targets(&["run-worker"], Some("run-session"), 10).unwrap();
        assert_eq!(rows.len(), 2, "one wake per completed run");
    }

    #[test]
    fn lost_completion_recovers_once_and_never_borrows_an_earlier_log_exit() {
        use std::io::Write;
        let (_env, root, repo, log) = fixture();
        std::fs::write(&log, "worker-check: EXIT 0\n").unwrap();
        let now = Utc::now();
        for (tail, expected) in [("worker-check: EXIT 7\n", Some(7)), ("", None)] {
            let id = register(&root, &repo, &log, now).unwrap().unwrap();
            bind(&root, &id, std::process::id()).unwrap();
            let mut run = load(&root, &id).unwrap();
            // A live recycled PID is not this command's process incarnation.
            run.fingerprint = Some(run.fingerprint.unwrap_or(0) + 1);
            save(&root, &run).unwrap();
            OpenOptions::new()
                .append(true)
                .open(&log)
                .unwrap()
                .write_all(tail.as_bytes())
                .unwrap();
            reconcile(&root, Some("run-session"), now).unwrap();
            reconcile(&root, Some("run-session"), now).unwrap();
            let recovered = load(&root, &id).unwrap();
            assert_eq!(recovered.exit_code, expected);
            assert!(recovered.finished_at.is_some() && recovered.notified);
        }
        let rows = crate::store::open_prompt_queue_store(&root)
            .unwrap()
            .peek_for_targets(&["run-worker"], Some("run-session"), 10)
            .unwrap();
        assert_eq!(
            rows.len(),
            2,
            "one durable wake per recovered run, including unknown status"
        );
    }
}
