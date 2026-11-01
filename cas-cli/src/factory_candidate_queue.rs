//! A worker's candidate request, built by the factory daemon.
//!
//! Workers never compile a candidate, and every hand-built one cost a
//! supervisor turn. `factory action=candidate_request` writes one file per task
//! here; a thread in the factory daemon builds them, one at a time, with the
//! same `cas qa candidate build` the supervisor runs, and tells the worker the
//! result directly. A newer request for a task overwrites its queued file, which
//! is the whole of supersession. The file queue needs no store worker, and the
//! job lanes are not used because their budgets are in seconds.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

const QUEUE_DIR: &str = "candidate-queue";
/// The daemon's own source checkout: the project's main checkout may be stale,
/// and a stale checkout runs a stale builder.
const BUILDER_CHECKOUT: &str = "candidate-builder";
const RUNNER_LOCK: &str = "runner.lock";
const RUNNER_HEARTBEAT: &str = "runner.heartbeat";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const HEARTBEAT_FRESH: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(250);
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const FAILURE_TAIL_BYTES: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CandidateRequest {
    pub task: String,
    pub revision: String,
    pub worker: String,
    pub factory_session: Option<String>,
    pub requested_at: DateTime<Utc>,
    #[serde(default)]
    pub admission_notice_sent: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Enqueued {
    Queued { superseded: Option<String> },
    AlreadyBuilding,
}

fn queue_dir(cas_root: &Path) -> PathBuf {
    cas_root.join(QUEUE_DIR)
}

fn queued_path(cas_root: &Path, task: &str) -> PathBuf {
    queue_dir(cas_root).join(format!("{task}.json"))
}

fn building_path(cas_root: &Path, task: &str) -> PathBuf {
    queue_dir(cas_root).join(format!("{task}.building"))
}

fn read_request(path: &Path) -> Result<CandidateRequest> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

/// Atomic, so the runner never reads a partial request.
fn write_request(path: &Path, request: &CandidateRequest) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(request)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

pub(crate) fn enqueue(cas_root: &Path, request: &CandidateRequest) -> Result<Enqueued> {
    std::fs::create_dir_all(queue_dir(cas_root))?;
    let building = read_request(&building_path(cas_root, &request.task)).ok();
    let path = queued_path(cas_root, &request.task);
    let previous = read_request(&path).ok();
    if previous.is_none()
        && building.is_some_and(|building| building.revision == request.revision)
    {
        return Ok(Enqueued::AlreadyBuilding);
    }
    write_request(&path, request)?;
    Ok(Enqueued::Queued {
        superseded: previous
            .map(|previous| previous.revision)
            .filter(|revision| revision != &request.revision),
    })
}

/// True when a daemon's runner has shown life recently. Without this a request
/// to a daemon that predates the queue would wait forever in silence.
pub(crate) fn runner_is_live(cas_root: &Path) -> bool {
    std::fs::metadata(queue_dir(cas_root).join(RUNNER_HEARTBEAT))
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < HEARTBEAT_FRESH)
}

pub(crate) fn mark_runner_live(cas_root: &Path) -> Result<()> {
    std::fs::create_dir_all(queue_dir(cas_root))?;
    std::fs::write(queue_dir(cas_root).join(RUNNER_HEARTBEAT), b"")?;
    Ok(())
}

/// The one-build-at-a-time lock, per machine rather than per project.
pub(crate) fn machine_lock_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home directory for the candidate build lock")?
        .join(".cas/candidate-build.lock"))
}

fn open_lock(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Rust opens with O_CLOEXEC, so a build's children never hold this lock.
    Ok(std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?)
}

/// Held by `cas qa candidate build` itself, so a supervisor's direct build and
/// a queued one still run one at a time. Waits rather than refuses.
pub(crate) fn hold_machine_build_lock() -> Result<std::fs::File> {
    let lock = open_lock(&machine_lock_path()?)?;
    if lock.try_lock_exclusive().is_err() {
        eprintln!("Waiting for another candidate build on this machine to finish");
        lock.lock_exclusive()?;
    }
    Ok(lock)
}

fn machine_lock_is_free() -> Result<bool> {
    let lock = open_lock(&machine_lock_path()?)?;
    Ok(lock.try_lock_exclusive().is_ok())
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// A hung remote must not stop the queue; the last fetched tip still builds.
fn fetch_bounded(repo: &Path, branch: &str) -> Result<()> {
    let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["fetch", "--quiet", "origin", refspec.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + FETCH_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            bail!("git fetch origin {branch} exited {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("git fetch origin {branch} timed out");
        }
        std::thread::sleep(CHILD_POLL_INTERVAL);
    }
}

/// Bring the daemon's own checkout to the default branch tip, and return it.
fn refresh_builder_checkout(cas_root: &Path) -> Result<PathBuf> {
    let project = cas_root
        .parent()
        .context("the Cassy root has no project directory")?;
    let branch = git(project, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|name| name.strip_prefix("origin/").map(str::to_owned))
        .unwrap_or_else(|| "main".to_owned());
    if let Err(error) = fetch_bounded(project, &branch) {
        tracing::warn!(%error, "candidate builder uses the last fetched source tip");
    }
    let remote = format!("refs/remotes/origin/{branch}^{{commit}}");
    let local = format!("refs/heads/{branch}^{{commit}}");
    let tip = [remote.as_str(), local.as_str(), "HEAD^{commit}"]
        .iter()
        .find_map(|name| git(project, &["rev-parse", "--verify", "--quiet", name]).ok())
        .context("no source tip to build the candidate builder from")?;
    let checkout = cas_root.join(BUILDER_CHECKOUT);
    if checkout.join(".git").exists() {
        git(&checkout, &["checkout", "--quiet", "--detach", "--force", &tip])?;
        git(&checkout, &["clean", "-fdq"])?;
    } else {
        git(project, &["worktree", "prune"])?;
        let path = checkout.display().to_string();
        git(project, &["worktree", "add", "--quiet", "--detach", "--force", &path, &tip])?;
    }
    Ok(checkout)
}

/// What took the machine's builder slot; dropped when the build ends.
pub(crate) type BuilderAdmission = Result<std::fs::File, String>;

pub(crate) struct Runner {
    pub cas_root: PathBuf,
    pub program: PathBuf,
    pub shutdown: Arc<AtomicBool>,
    pub admit: fn(&Path) -> BuilderAdmission,
}

/// The fields of `cas qa candidate build --json`.
#[derive(Debug, Deserialize)]
struct BuiltCandidate {
    candidate_dir: PathBuf,
    receipt_file: PathBuf,
    revision: String,
    version: String,
    sha256: String,
}

impl Runner {
    fn notify(&self, request: &CandidateRequest, summary: &str, message: &str) {
        let sent = crate::store::open_prompt_queue_store(&self.cas_root).and_then(|queue| {
            Ok(queue.enqueue_with_summary(
                "director",
                &request.worker,
                message,
                request.factory_session.as_deref(),
                Some(summary),
            )?)
        });
        match sent {
            Ok(_) => crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(
                &self.cas_root,
            ),
            Err(error) => tracing::warn!(%error, %message, "could not deliver a candidate result"),
        }
    }

    fn queued(&self) -> Vec<CandidateRequest> {
        let Ok(entries) = std::fs::read_dir(queue_dir(&self.cas_root)) else {
            return Vec::new();
        };
        let mut requests: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
            .filter_map(|path| read_request(&path).ok())
            .collect();
        requests.sort_by_key(|request| request.requested_at);
        requests
    }

    /// A claim left by a daemon that died goes back in line, unless a newer
    /// request already replaced it. A build that survived its daemon published
    /// its candidate, so the retry only reports that receipt.
    fn requeue_orphans(&self) -> Result<()> {
        let Ok(entries) = std::fs::read_dir(queue_dir(&self.cas_root)) else {
            return Ok(());
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().is_none_or(|extension| extension != "building") {
                continue;
            }
            let queued = path.with_extension("json");
            if queued.exists() {
                std::fs::remove_file(&path)?;
            } else {
                std::fs::rename(&path, &queued)?;
            }
        }
        Ok(())
    }

    /// Build the oldest request, if the machine has room. Returns whether one ran.
    pub(crate) fn run_next(&self) -> Result<bool> {
        let dir = queue_dir(&self.cas_root);
        if !dir.is_dir() {
            return Ok(false);
        }
        let runner_lock = open_lock(&dir.join(RUNNER_LOCK))?;
        if runner_lock.try_lock_exclusive().is_err() {
            return Ok(false);
        }
        self.requeue_orphans()?;
        let Some(mut request) = self.queued().into_iter().next() else {
            return Ok(false);
        };
        if !machine_lock_is_free()? {
            return Ok(false);
        }
        let slot = match (self.admit)(&self.cas_root) {
            Ok(slot) => slot,
            Err(reason) => {
                if !request.admission_notice_sent {
                    request.admission_notice_sent = true;
                    write_request(&queued_path(&self.cas_root, &request.task), &request)?;
                    self.notify(
                        &request,
                        "Candidate queued, waiting for build admission",
                        &format!(
                            "Candidate for {} at {} is queued and waiting for build admission: {reason}. \
                             It starts on its own; you need do nothing.",
                            request.task, request.revision
                        ),
                    );
                }
                return Ok(false);
            }
        };
        let claim = building_path(&self.cas_root, &request.task);
        // Re-read through the rename: a newer request may have just replaced it.
        std::fs::rename(queued_path(&self.cas_root, &request.task), &claim)?;
        let request = read_request(&claim)?;
        let started = Instant::now();
        let outcome = self.build(&request);
        drop(slot);
        match outcome {
            BuildOutcome::Interrupted => return Ok(true),
            BuildOutcome::Built(built, log) => self.notify(
                &request,
                "Candidate ready",
                &format!(
                    "Candidate ready for {}.\nPath: {}\nVersion: {}\nRevision: {}\nSHA-256: {}\nReceipt: {}\nLog: {}\n\
                     Built in {}s; {}s from your request.",
                    request.task,
                    built.candidate_dir.join("cas").display(),
                    built.version,
                    built.revision,
                    built.sha256,
                    built.receipt_file.display(),
                    log.display(),
                    started.elapsed().as_secs(),
                    (Utc::now() - request.requested_at).num_seconds(),
                ),
            ),
            BuildOutcome::Failed(error, log) => self.notify(
                &request,
                "Candidate build failed",
                &format!(
                    "Candidate build FAILED for {} at {}.\n{error}\nLog: {}",
                    request.task,
                    request.revision,
                    log.map_or_else(|| "none".to_owned(), |log| log.display().to_string()),
                ),
            ),
        }
        std::fs::remove_file(&claim)?;
        Ok(true)
    }

    fn build(&self, request: &CandidateRequest) -> BuildOutcome {
        let log = match crate::cli::qa::candidate_task_dir(&self.cas_root, &request.task)
            .and_then(|dir| {
                std::fs::create_dir_all(&dir)?;
                Ok(dir.join(format!("candidate-build-{}.log", &request.revision[..12])))
            }) {
            Ok(log) => log,
            Err(error) => return BuildOutcome::Failed(format!("{error:#}"), None),
        };
        match self.run_builder(request, &log) {
            Ok(outcome) => outcome,
            Err(error) => BuildOutcome::Failed(format!("{error:#}"), Some(log)),
        }
    }

    fn run_builder(&self, request: &CandidateRequest, log: &Path) -> Result<BuildOutcome> {
        let checkout = refresh_builder_checkout(&self.cas_root)?;
        let stdout_path = log.with_extension("json");
        let mut command = Command::new(&self.program);
        command
            .args(["qa", "candidate", "build", "--task", request.task.as_str()])
            .args(["--rev", request.revision.as_str(), "--json"])
            .current_dir(&checkout)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&stdout_path)?)
            .stderr(std::fs::File::create(log)?);
        // The daemon's own identity must not reach the build, and the build
        // runs with the supervisor's authority, not the requester's.
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CAS_") {
                command.env_remove(name);
            }
        }
        command.env("CAS_ROOT", &self.cas_root).env("CAS_AGENT_ROLE", "supervisor");
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().context("start the candidate build")?;
        let mut beat = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if self.shutdown.load(Ordering::Relaxed) {
                #[cfg(unix)]
                unsafe {
                    libc::killpg(child.id() as libc::pid_t, libc::SIGTERM);
                }
                return Ok(BuildOutcome::Interrupted);
            }
            if beat.elapsed() >= HEARTBEAT_INTERVAL {
                let _ = mark_runner_live(&self.cas_root);
                beat = Instant::now();
            }
            std::thread::sleep(CHILD_POLL_INTERVAL);
        };
        if !status.success() {
            return Ok(BuildOutcome::Failed(
                format!("`cas qa candidate build` exited {status}:\n{}", log_tail(log)),
                Some(log.to_path_buf()),
            ));
        }
        let stdout = std::fs::read_to_string(&stdout_path)?;
        let _ = std::fs::remove_file(&stdout_path);
        let built: BuiltCandidate = serde_json::from_str(
            stdout.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default(),
        )
        .with_context(|| format!("read the candidate receipt from {stdout:?}"))?;
        if built.revision != request.revision {
            bail!("candidate receipt names {}, not {}", built.revision, request.revision);
        }
        Ok(BuildOutcome::Built(built, log.to_path_buf()))
    }
}

enum BuildOutcome {
    Built(BuiltCandidate, PathBuf),
    Failed(String, Option<PathBuf>),
    Interrupted,
}

fn log_tail(log: &Path) -> String {
    let text = String::from_utf8_lossy(&std::fs::read(log).unwrap_or_default()).into_owned();
    let mut start = text.len().saturating_sub(FAILURE_TAIL_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].trim().to_owned()
}

/// Starts the queue's thread in a factory daemon. The handle is not joined: an
/// interrupted build's claim is requeued by the next daemon.
pub(crate) fn spawn_runner(
    cas_root: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    let program = match crate::ui::factory::provisioner_executable() {
        Ok(program) => program,
        Err(error) => {
            tracing::warn!(%error, "candidate builds disabled: this daemon cannot name its binary");
            return None;
        }
    };
    let runner = Runner {
        cas_root,
        program,
        shutdown,
        admit: crate::factory_worker_check::admit_daemon_build,
    };
    std::thread::Builder::new()
        .name("cas-candidate-queue".to_string())
        .spawn(move || {
            let mut beat: Option<Instant> = None;
            while !runner.shutdown.load(Ordering::Relaxed) {
                if beat.is_none_or(|beat| beat.elapsed() >= HEARTBEAT_INTERVAL) {
                    let _ = mark_runner_live(&runner.cas_root);
                    beat = Some(Instant::now());
                }
                if let Err(error) = runner.run_next() {
                    tracing::warn!(%error, "candidate queue pass failed");
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        })
        .ok()
}

#[cfg(test)]
#[path = "factory_candidate_queue_tests.rs"]
mod tests;
