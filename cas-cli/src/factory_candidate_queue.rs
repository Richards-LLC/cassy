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
const BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
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
    let _queue = hold_queue_lock(cas_root)?;
    let building = read_request(&building_path(cas_root, &request.task)).ok();
    let path = queued_path(cas_root, &request.task);
    let previous = read_request(&path).ok();
    if previous.is_none() && building.is_some_and(|building| building.revision == request.revision)
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
    std::fs::write(
        queue_dir(cas_root).join(RUNNER_HEARTBEAT),
        Utc::now().to_rfc3339(),
    )?;
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

/// Request writers and the runner serialize short read/modify/rename operations.
/// Never hold this across admission or a build: workers can supersede either.
fn hold_queue_lock(cas_root: &Path) -> Result<std::fs::File> {
    let lock = open_lock(&queue_dir(cas_root).join("requests.lock"))?;
    lock.lock_exclusive()?;
    Ok(lock)
}

fn mark_admission_notice(cas_root: &Path, observed: &CandidateRequest) -> Result<bool> {
    let _queue = hold_queue_lock(cas_root)?;
    let path = queued_path(cas_root, &observed.task);
    let mut current = read_request(&path)?;
    if current.revision != observed.revision
        || current.requested_at != observed.requested_at
        || current.worker != observed.worker
        || current.factory_session != observed.factory_session
        || current.admission_notice_sent
    {
        return Ok(false);
    }
    current.admission_notice_sent = true;
    write_request(&path, &current)?;
    Ok(true)
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

fn git(program: &Path, repo: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new(program);
    command
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    let deadline = Instant::now() + FETCH_TIMEOUT;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            stop_child(&mut child)?;
            bail!("git {} timed out", args.join(" "));
        }
        std::thread::sleep(CHILD_POLL_INTERVAL);
    }
    let output = child.wait_with_output()?;
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
fn fetch_bounded(program: &Path, repo: &Path, branch: &str) -> Result<()> {
    let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
    let mut command = Command::new(program);
    command
        .arg("-C")
        .arg(repo)
        .args(["fetch", "--quiet", "origin", refspec.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn()?;
    let deadline = Instant::now() + FETCH_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            bail!("git fetch origin {branch} exited {status}");
        }
        if Instant::now() >= deadline {
            stop_child(&mut child)?;
            bail!("git fetch origin {branch} timed out");
        }
        std::thread::sleep(CHILD_POLL_INTERVAL);
    }
}

/// The leader is still owned and unreaped when its group is signalled. For a
/// sandboxed build, killing its PID-namespace init also kills detached sessions
/// (ChildScope starts npm/Cargo in new sessions, outside this process group).
fn stop_child(child: &mut std::process::Child) -> Result<()> {
    #[cfg(unix)]
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    child.kill()?;
    child.wait()?;
    Ok(())
}

/// Uses the worker's credential-store policy, with a private PID namespace so
/// every build descendant, including a new session, dies with the sandbox.
fn sandboxed_builder_command(program: &Path) -> Result<Command> {
    if !cfg!(target_os = "linux") {
        bail!("queued candidate builds require Linux bubblewrap credential and process isolation");
    }
    let mut command = Command::new("bwrap");
    command.args([
        "--dev-bind",
        "/",
        "/",
        "--unshare-user",
        "--unshare-pid",
        "--proc",
        "/proc",
        "--die-with-parent",
    ]);
    let mut stores =
        cas_pty::store_isolation::operator_credential_stores(|name| std::env::var_os(name));
    for store in stores.clone() {
        if let Ok(resolved) = store.canonicalize() {
            if !stores.contains(&resolved) {
                stores.push(resolved);
            }
        }
    }
    for store in stores {
        match std::fs::metadata(&store) {
            Ok(metadata) if metadata.is_dir() => {
                command
                    .arg("--tmpfs")
                    .arg(&store)
                    .arg("--remount-ro")
                    .arg(&store);
            }
            Ok(_) => {
                command.args(["--ro-bind", "/dev/null"]).arg(&store);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect a candidate credential-store mask"),
        }
    }
    command.arg("--").arg(program);
    Ok(command)
}

/// Red-test seam, replaced by the implementation in the next commit.
#[cfg(test)]
fn sandbox_problem(_os: &str, _bwrap: &str, _program: &Path) -> Option<String> {
    None
}

/// Red-test seam, replaced by the implementation in the next commit.
#[cfg(test)]
fn record_sandbox_state(_cas_root: &Path, _problem: Option<&str>) -> Result<()> {
    Ok(())
}

/// Preparation can block in fetch or worktree setup. Its heartbeat must not
/// depend on reaching the compiler's polling loop.
struct RunnerHeartbeat {
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RunnerHeartbeat {
    fn start(cas_root: &Path, interval: Duration) -> Result<Self> {
        mark_runner_live(cas_root)?;
        let root = cas_root.to_path_buf();
        let (stop, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("cas-candidate-heartbeat".into())
            .spawn(move || {
                while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    receiver.recv_timeout(interval.max(Duration::from_millis(1)))
                {
                    let _ = mark_runner_live(&root);
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for RunnerHeartbeat {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Bring the daemon's own checkout to the default branch tip, and return it.
fn refresh_builder_checkout(cas_root: &Path, program: &Path) -> Result<PathBuf> {
    let project = cas_root
        .parent()
        .context("the Cassy root has no project directory")?;
    let branch = git(
        program,
        project,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .and_then(|name| name.strip_prefix("origin/").map(str::to_owned))
    .unwrap_or_else(|| "main".to_owned());
    if let Err(error) = fetch_bounded(program, project, &branch) {
        tracing::warn!(%error, "candidate builder uses the last fetched source tip");
    }
    let remote = format!("refs/remotes/origin/{branch}^{{commit}}");
    let local = format!("refs/heads/{branch}^{{commit}}");
    let tip = [remote.as_str(), local.as_str(), "HEAD^{commit}"]
        .iter()
        .find_map(|name| {
            git(
                program,
                project,
                &["rev-parse", "--verify", "--quiet", name],
            )
            .ok()
        })
        .context("no source tip to build the candidate builder from")?;
    let checkout = cas_root.join(BUILDER_CHECKOUT);
    if checkout.join(".git").exists() {
        git(
            program,
            &checkout,
            &["checkout", "--quiet", "--detach", "--force", &tip],
        )?;
        git(program, &checkout, &["clean", "-fdq"])?;
    } else {
        // --force reclaims a registration whose directory was deleted.
        let path = checkout.display().to_string();
        git(
            program,
            project,
            &[
                "worktree", "add", "--quiet", "--detach", "--force", &path, &tip,
            ],
        )?;
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
    build_elapsed: fn(Instant) -> Duration,
    heartbeat_interval: Duration,
    git_program: PathBuf,
    build_command: fn(&Path) -> Result<Command>,
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
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .filter_map(|path| read_request(&path).ok())
            .collect();
        requests.sort_by_key(|request| request.requested_at);
        requests
    }

    /// A claim left by a daemon that died goes back in line, unless a newer
    /// request already replaced it. A build that survived its daemon published
    /// its candidate, so the retry only reports that receipt.
    fn requeue_orphans(&self) -> Result<()> {
        let _queue = hold_queue_lock(&self.cas_root)?;
        let Ok(entries) = std::fs::read_dir(queue_dir(&self.cas_root)) else {
            return Ok(());
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path
                .extension()
                .is_none_or(|extension| extension != "building")
            {
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
        let Some(request) = self.queued().into_iter().next() else {
            return Ok(false);
        };
        if !machine_lock_is_free()? {
            return Ok(false);
        }
        let slot = match (self.admit)(&self.cas_root) {
            Ok(slot) => slot,
            Err(reason) => {
                if mark_admission_notice(&self.cas_root, &request)? {
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
        let request = {
            let _queue = hold_queue_lock(&self.cas_root)?;
            std::fs::rename(queued_path(&self.cas_root, &request.task), &claim)?;
            read_request(&claim)?
        };
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
        let log = match crate::cli::qa::candidate_task_dir(&self.cas_root, &request.task).and_then(
            |dir| {
                std::fs::create_dir_all(&dir)?;
                Ok(dir.join(format!("candidate-build-{}.log", &request.revision[..12])))
            },
        ) {
            Ok(log) => log,
            Err(error) => return BuildOutcome::Failed(format!("{error:#}"), None),
        };
        match self.run_builder(request, &log) {
            Ok(outcome) => outcome,
            Err(error) => BuildOutcome::Failed(format!("{error:#}"), Some(log)),
        }
    }

    fn run_builder(&self, request: &CandidateRequest, log: &Path) -> Result<BuildOutcome> {
        let started = Instant::now();
        let _heartbeat = RunnerHeartbeat::start(&self.cas_root, self.heartbeat_interval)?;
        let checkout = refresh_builder_checkout(&self.cas_root, &self.git_program)?;
        let stdout_path = log.with_extension("json");
        let mut command = (self.build_command)(&self.program)?;
        command
            .args(["qa", "candidate", "build", "--task", request.task.as_str()])
            .args(["--rev", request.revision.as_str(), "--json"])
            .current_dir(&checkout)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&stdout_path)?)
            .stderr(std::fs::File::create(log)?);
        // Compilation needs tools, caches and locale, never daemon identity,
        // API tokens, SSH agents or arbitrary inherited provider variables.
        command.env_clear();
        for name in [
            "HOME",
            "PATH",
            "USER",
            "LOGNAME",
            "LANG",
            "LC_ALL",
            "TERM",
            "TMPDIR",
            "TMP",
            "TEMP",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "RUSTUP_TOOLCHAIN",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CC",
            "CXX",
            "AR",
            "LD",
            "ZIG",
            "PKG_CONFIG_PATH",
            "LIBCLANG_PATH",
            "LIBRARY_PATH",
            "LD_LIBRARY_PATH",
            "CPATH",
            "C_INCLUDE_PATH",
            "CPLUS_INCLUDE_PATH",
            "OPENSSL_DIR",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.env("CAS_ROOT", &self.cas_root);
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().context("start the candidate build")?;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if self.shutdown.load(Ordering::Relaxed) {
                stop_child(&mut child)?;
                return Ok(BuildOutcome::Interrupted);
            }
            if (self.build_elapsed)(started) >= BUILD_TIMEOUT {
                stop_child(&mut child)?;
                return Ok(BuildOutcome::Failed(
                    format!(
                        "`cas qa candidate build` timed out after {}s:\n{}",
                        BUILD_TIMEOUT.as_secs(),
                        log_tail(log)
                    ),
                    Some(log.to_path_buf()),
                ));
            }
            std::thread::sleep(CHILD_POLL_INTERVAL);
        };
        if !status.success() {
            return Ok(BuildOutcome::Failed(
                format!(
                    "`cas qa candidate build` exited {status}:\n{}",
                    log_tail(log)
                ),
                Some(log.to_path_buf()),
            ));
        }
        let stdout = std::fs::read_to_string(&stdout_path)?;
        let _ = std::fs::remove_file(&stdout_path);
        let built: BuiltCandidate = serde_json::from_str(
            stdout
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_default(),
        )
        .with_context(|| format!("read the candidate receipt from {stdout:?}"))?;
        if built.revision != request.revision {
            bail!(
                "candidate receipt names {}, not {}",
                built.revision,
                request.revision
            );
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
        build_elapsed: |started| started.elapsed(),
        heartbeat_interval: HEARTBEAT_INTERVAL,
        git_program: PathBuf::from("git"),
        build_command: sandboxed_builder_command,
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
