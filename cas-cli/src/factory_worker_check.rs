//! Capped compile and targeted-test evidence. The runner serializes admission
//! and holds OS slot/lane locks until Cargo exits; descendants never inherit them.
//! A separate target lifetime lease is inherited to protect surviving children.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(unix)]
static WORKER_WAIT_SIGNAL: AtomicI32 = AtomicI32::new(0);
#[cfg(unix)]
static WORKER_WAIT_HANDLER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
extern "C" fn record_worker_wait_signal(signum: libc::c_int) {
    WORKER_WAIT_SIGNAL.store(signum, Ordering::Relaxed);
}

/// Signal handling applies only while this worker Cargo command is owned.
#[cfg(unix)]
struct WorkerWaitSignals {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

#[cfg(unix)]
impl WorkerWaitSignals {
    fn install() -> std::io::Result<Self> {
        let mut guard = Self {
            _lock: WORKER_WAIT_HANDLER_LOCK.lock().unwrap_or_else(|error| error.into_inner()),
            previous: Vec::new(),
        };
        WORKER_WAIT_SIGNAL.store(0, Ordering::Relaxed);
        for signum in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = record_worker_wait_signal as *const () as libc::sighandler_t;
            unsafe { libc::sigemptyset(&mut action.sa_mask) };
            if unsafe { libc::sigaction(signum, &action, &mut previous) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            guard.previous.push((signum, previous));
        }
        Ok(guard)
    }
}

#[cfg(unix)]
impl Drop for WorkerWaitSignals {
    fn drop(&mut self) {
        for (signum, previous) in self.previous.iter().rev() {
            unsafe { libc::sigaction(*signum, previous, std::ptr::null_mut()) };
        }
        WORKER_WAIT_SIGNAL.store(0, Ordering::Relaxed);
    }
}

#[cfg(unix)]
fn worker_group_has_live_members(group: libc::pid_t) -> bool {
    let Ok(output) = Command::new("ps").args(["-A", "-o", "pgid=,stat="]).output() else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    String::from_utf8_lossy(&output.stdout).lines().any(|row| {
        let mut fields = row.split_whitespace();
        fields.next().and_then(|value| value.parse::<libc::pid_t>().ok()) == Some(group)
            && fields.next().is_some_and(|state| !state.starts_with('Z'))
    })
}

#[cfg(unix)]
fn wait_for_worker_command(child: &mut std::process::Child) -> Result<std::process::ExitStatus> {
    use std::time::{Duration, Instant};

    let group = child.id() as libc::pid_t;
    loop {
        let signum = WORKER_WAIT_SIGNAL.swap(0, Ordering::Relaxed);
        if signum != 0 {
            // The group leader remains unreaped through both signals, so its
            // PGID cannot be reused while its Python supervisor clears Cargo.
            unsafe { libc::killpg(group, signum) };
            let grace_deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < grace_deadline && worker_group_has_live_members(group) {
                // try_wait would reap the group leader before the final KILL.
                std::thread::sleep(Duration::from_millis(50));
            }
            unsafe { libc::killpg(group, libc::SIGKILL) };
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if let Some(status) = child.try_wait()? {
                    return Ok(status);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            bail!("worker command PID {group} did not exit after SIGKILL");
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CheckReceipt {
    head: String,
    repo: PathBuf,
    packages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    test: Option<TestReceipt>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TestReceipt {
    filter: String,
    harness: Option<String>,
    count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_path_opt_out: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    supervisor_recorded: Option<SupervisorRecordedProof>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SupervisorRecordedProof {
    supervisor_id: String,
    task_id: String,
    tested_repo: PathBuf,
    cargo_args: Vec<String>,
    log_path: PathBuf,
    log_sha256: String,
    recorded_at: String,
}

/// An authenticated supervisor attests the exit code of an existing run;
/// recording never starts Cargo or relaxes worker admission.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorTestEvidence {
    head: String,
    repo: PathBuf,
    cargo_args: Vec<String>,
    log_path: PathBuf,
    exit_code: i32,
}

impl TestReceipt {
    fn path_note(&self) -> String {
        let mut note = self
            .host_path_opt_out
            .as_ref()
            .map(|reason| format!(" [host PATH opt-out: {reason}]"))
            .unwrap_or_default();
        if let Some(proof) = &self.supervisor_recorded {
            note.push_str(&format!(
                " [supervisor-recorded by {} for {}; log sha256={}]",
                proof.supervisor_id, proof.task_id, proof.log_sha256
            ));
        }
        note
    }
}

#[derive(Debug)]
struct TargetedTest {
    package: String,
    harness: Option<String>,
    filter: String,
}

pub(crate) fn valid_package(package: &str) -> bool {
    package
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && package
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Targeted tests may disambiguate duplicate crate names with an exact version.
/// Keep URLs, partial versions, ranges and shell syntax outside the capped lane.
fn package_selector(package: &str) -> Option<(&str, Option<&str>)> {
    let (name, version) = match package.split_once('@') {
        Some((name, version)) => {
            semver::Version::parse(version).ok()?;
            (name, Some(version))
        }
        None => (package, None),
    };
    valid_package(name).then_some((name, version))
}

/// Deliberately excludes glob/package-ID expressions, aliases, flags that
/// broaden the scope, and arbitrary Cargo configuration or toolchains.
pub(crate) fn check_packages(args: &[String]) -> Option<Vec<String>> {
    let mut packages = Vec::new();
    let mut target_selected = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-p" if args.get(index + 1).is_some_and(|arg| valid_package(arg)) => {
                packages.push(args[index + 1].clone());
                index += 2;
            }
            "--lib" | "--tests" if !target_selected => {
                target_selected = true;
                index += 1;
            }
            _ => return None,
        }
    }
    (target_selected && !packages.is_empty()).then_some(packages)
}

/// Only positive named-test selectors may be joined by union/intersection.
/// Exclude all(), negation, regex/globs and predicates that select a whole binary.
fn named_filter(filter: &str) -> bool {
    regex::Regex::new(r"^[ \t]*test\(=?[A-Za-z0-9_][A-Za-z0-9_:.-]*\)(?:[ \t]*[|&][ \t]*test\(=?[A-Za-z0-9_][A-Za-z0-9_:.-]*\))*[ \t]*$")
        .is_ok_and(|pattern| pattern.is_match(filter))
}

fn targeted_test(args: &[String]) -> Option<TargetedTest> {
    if args.get(..3)? != ["nextest", "run", "-p"] {
        return None;
    }
    let package = args.get(3)?.clone();
    if package_selector(&package).is_none() {
        return None;
    }
    let mut index = 4;
    let harness = match args.get(index)?.as_str() {
        "--lib" => {
            index += 1;
            None
        }
        "--test" => {
            let name = args.get(index + 1)?;
            if !valid_package(name) {
                return None;
            }
            index += 2;
            Some(name.clone())
        }
        _ => None,
    };
    if args.get(index)? != "-E" || args.len() != index + 2 {
        return None;
    }
    let filter = args[index + 1].clone();
    named_filter(&filter).then_some(TargetedTest {
        package,
        harness,
        filter,
    })
}

pub(crate) fn allowed_args(args: &[String]) -> bool {
    check_packages(args).is_some() || targeted_test(args).is_some()
}

/// Use the same explicit [[test]] name/path inventory as cas-test-targets.py;
/// auto-discovered suites and source-module aliases are not harness names.
fn validate_harness(repo: &Path, test: &TargetedTest) -> Result<()> {
    let Some(harness) = &test.harness else {
        return Ok(());
    };
    let (path, value) = package_manifest(repo, &test.package)?;
    let found = value
        .get("test")
        .and_then(toml::Value::as_array)
        .is_some_and(|targets| {
            targets.iter().any(|target| {
                target.get("name").and_then(toml::Value::as_str) == Some(harness.as_str())
                    && target
                        .get("path")
                        .and_then(toml::Value::as_str)
                        .is_some_and(|source| {
                            path.parent().is_some_and(|dir| dir.join(source).is_file())
                        })
            })
        });
    if !found {
        bail!(
            "Worker test refused: {harness} is not an explicit test harness in {} (cas-test-targets.py inventory)",
            test.package
        );
    }
    Ok(())
}

fn package_manifest(repo: &Path, package: &str) -> Result<(PathBuf, toml::Value)> {
    let (name, version) = package_selector(package).context("invalid package selector")?;
    let root = repo.join("Cargo.toml");
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(&root)?)?;
    let mut manifests = vec![root];
    if let Some(members) = manifest
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(toml::Value::as_array)
    {
        for member in members.iter().filter_map(toml::Value::as_str) {
            manifests.extend(
                glob::glob(&repo.join(member).join("Cargo.toml").to_string_lossy())?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            );
        }
    }
    for path in manifests {
        let value: toml::Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
        if value
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
            == Some(name)
        {
            if let Some(version) = version {
                let declared = value.get("package").and_then(|p| p.get("version"));
                let resolved = declared.and_then(toml::Value::as_str).or_else(|| {
                    declared
                        .and_then(|v| v.get("workspace"))
                        .and_then(toml::Value::as_bool)
                        .filter(|inherited| *inherited)
                        .and_then(|_| manifest.get("workspace")?.get("package")?.get("version")?.as_str())
                });
                if resolved != Some(version) {
                    continue;
                }
            }
            return Ok((path, value));
        }
    }
    bail!("Worker test refused: package {package} not found in workspace")
}

/// Record an existing supervisor-run selection under the assigned worker's
/// normal receipt key. The authority, task binding and provenance are checked
/// here so MCP and CLI use exactly the same path.
pub(crate) fn record_supervisor_test(
    core: &crate::mcp::CasCore,
    task_id: &str,
    payload: &str,
) -> Result<String> {
    let supervisor = core.resolve_live_supervisor_authority().map_err(|_| {
        anyhow::anyhow!("record_test_receipt requires a live registered supervisor")
    })?;
    if payload.len() > 512 * 1024 {
        bail!("supervisor evidence payload exceeds 512 KiB");
    }
    let evidence: SupervisorTestEvidence = serde_json::from_str(payload)
        .context("payload must contain head, repo, cargo_args, log_path and exit_code")?;
    if !valid_git_oid(&evidence.head) {
        bail!("head must be the full tested commit SHA");
    }
    if !evidence.repo.is_absolute() || !evidence.log_path.is_absolute() {
        bail!("repo and log_path must be absolute paths");
    }
    let test = targeted_test(&evidence.cargo_args).context(
        "cargo_args must be nextest run -p <crate> [--lib|--test <harness>] -E 'test(name)'",
    )?;
    let root = core.cas_root.canonicalize()?;
    let tasks = crate::store::open_task_store(&root)?;
    let task = tasks.get(task_id)?;
    if task.status == cas_types::TaskStatus::Closed {
        bail!("cannot record new test evidence for a closed task");
    }
    let agents = crate::store::open_agent_store(&root)?;
    let worker = task
        .assignee
        .as_deref()
        .and_then(|assignee| {
            crate::mcp::tools::core::task::resolve_agent_identity(agents.as_ref(), assignee)
        })
        .filter(|agent| agent.role == cas_types::AgentRole::Worker)
        .context("task must be assigned to a registered worker")?;
    // Same metadata/convention precedence as the factory worker roster.
    let repo = worker
        .metadata
        .get("clone_path")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| root.join("worktrees").join(&worker.name))
        .canonicalize()
        .context("assigned worker checkout is unavailable")?;
    let tested_repo = evidence
        .repo
        .canonicalize()
        .context("tested checkout is unavailable")?;
    for checkout in [&repo, &tested_repo] {
        if PathBuf::from(git(checkout, &["rev-parse", "--show-toplevel"])?).canonicalize()?
            != *checkout
            || clean_head(checkout)? != evidence.head
        {
            bail!(
                "task worker checkout and tested checkout must both be clean at {}",
                evidence.head
            );
        }
    }
    package_manifest(&repo, &test.package)?;
    validate_harness(&repo, &test)?;
    let _lane = try_lock_lane(&root, &repo)?
        .context("worker has an active capped run; retry recording later")?;
    let path = test_receipt_path(&root, &repo, &evidence.head, &test);
    // Just like an executed retry, failed evidence invalidates only this
    // exact command's earlier receipt, never another selection's proof.
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if evidence.exit_code != 0 {
        bail!(
            "supervisor test exited {}; no PASS receipt recorded",
            evidence.exit_code
        );
    }
    let log_path = evidence.log_path.canonicalize()?;
    if !std::fs::metadata(&log_path)?.is_file() {
        bail!("supervisor log must be a regular file");
    }
    let mut log_file = File::open(&log_path)?;
    let mut bytes = Vec::new();
    use std::io::Read;
    (&mut log_file)
        .take(25 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > 25 * 1024 * 1024 {
        bail!("supervisor log must be non-empty and at most 25 MiB");
    }
    let log = std::str::from_utf8(&bytes).context("supervisor log must be UTF-8")?;
    let ansi = regex::Regex::new(r"\x1b\[[0-9;]*[A-Za-z]")?;
    let log = ansi.replace_all(log, "");
    let summary = regex::Regex::new(
        r"(?m)^ *Summary +\[[^\r\n]*\] +([0-9]+) tests? run: ([0-9]+) passed(?:, [^\r\n]*)?$",
    )?;
    let summaries = summary.captures_iter(&log).collect::<Vec<_>>();
    if summaries.len() != 1 {
        bail!("supervisor log must contain exactly one selected nextest summary");
    }
    let run: u64 = summaries[0][1].parse()?;
    let count: u64 = summaries[0][2].parse()?;
    if count == 0 || count != run {
        bail!("supervisor log must show a nonzero selection with every executed test passed");
    }
    // A receipt is not transferable across ownership or source mutations
    // while the log was read. The worker lane lock also fences capped writers.
    if tasks.get(task_id)?.assignee != task.assignee
        || clean_head(&repo)? != evidence.head
        || clean_head(&tested_repo)? != evidence.head
    {
        bail!("task assignment or checkout changed while recording; no PASS receipt recorded");
    }
    let provenance = SupervisorRecordedProof {
        supervisor_id: supervisor.id,
        task_id: task_id.to_string(),
        tested_repo,
        cargo_args: evidence.cargo_args,
        log_path,
        log_sha256: hex::encode(Sha256::digest(&bytes)),
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };
    let record = CheckReceipt {
        head: evidence.head,
        repo,
        packages: vec![test.package],
        test: Some(TestReceipt {
            filter: test.filter,
            harness: test.harness,
            count,
            host_path_opt_out: None,
            supervisor_recorded: Some(provenance),
        }),
    };
    let proof = record.test.as_ref().expect("test receipt");
    let receipt = format!(
        "test: PASS {} {} {} {}{}",
        record.head,
        record.packages[0],
        proof.filter,
        proof.count,
        proof.path_note()
    );
    tasks.append_note(
        task_id,
        &format!(
            "[{}] SUPERVISOR-RECORDED TEST: {}; provenance={}",
            chrono::Utc::now().to_rfc3339(),
            receipt,
            serde_json::to_string(&proof.supervisor_recorded)?
        ),
    )?;
    std::fs::create_dir_all(path.parent().context("receipt directory")?)?;
    std::fs::write(path, serde_json::to_vec(&record)?)?;
    Ok(receipt)
}

fn lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A compiler-cache daemon may outlive Cargo. Keep all admission,
        // lane and slot descriptors in this runner, never in descendants.
        options.custom_flags(libc::O_CLOEXEC);
    }
    Ok(options.open(path)?)
}

fn waiting_for_admission(reason: &str) -> String {
    format!("Waiting for build admission: {reason}.\nCapacity wait: keep the task in_progress and retain its lease. Do not set blocked or page the supervisor for a routine capacity retry.\nRetry the same capped Cargo command after 120 seconds; the daemon can queue one supervisor notice per recorded wait after 20 minutes, subject to normal delivery readiness. Report an actual need for assistance immediately.")
}

#[derive(Debug)]
struct CapacityRefusal(String);
impl std::fmt::Display for CapacityRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&waiting_for_admission(&self.0))
    }
}
impl std::error::Error for CapacityRefusal {}

fn recorded_refusal(root: &Path, repo: &Path, cause: &str) -> anyhow::Error {
    match crate::factory_capacity_wait::record_refusal(root, repo, cause, chrono::Utc::now()) {
        Ok(Some(wait)) => {
            let mut message = waiting_for_admission(&wait.cause);
            message.push_str(&format!("\nCapacity wait recorded for {}; next retry: {}.", wait.task_id, wait.next_retry.to_rfc3339()));
            anyhow::anyhow!(message)
        }
        Ok(None) => anyhow::anyhow!(waiting_for_admission(cause)),
        Err(error) => {
            let mut message = waiting_for_admission(cause);
            message.push_str(&format!("\nCapacity wait could not be recorded: {error}. Report storage/ownership assistance if needed."));
            anyhow::anyhow!(message)
        }
    }
}

#[derive(Debug)]
struct BuilderSlot {
    file: Option<File>,
    index: usize,
    root: PathBuf,
}

impl BuilderSlot {
    fn audit(&self, event: &str) {
        use std::io::Write;
        // Never panic in Drop if a caller closes its log stream.
        let _ = writeln!(std::io::stdout().lock(), "worker-check slot: {}", serde_json::json!({
            "event": event,
            "utc": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "slot": self.index,
            "pid": std::process::id(),
            "root": self.root,
        }));
    }
}

impl Drop for BuilderSlot {
    fn drop(&mut self) {
        // Record while still locked: another holder's admission must follow
        // this release record, including on early error returns.
        self.audit("released");
        drop(self.file.take());
    }
}

fn acquire_slot(root: &Path, cap: usize) -> Result<BuilderSlot> {
    std::fs::create_dir_all(root)?;
    for index in 0..cap {
        let slot = lock_file(&root.join(format!("slot-{index}.lock")))?;
        match slot.try_lock_exclusive() {
            Ok(()) => {
                let slot = BuilderSlot { file: Some(slot), index, root: root.to_path_buf() };
                slot.audit("admitted");
                return Ok(slot);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(CapacityRefusal(format!("max_concurrent_builders={cap} slots are occupied")).into())
}

fn available_slot_count(root: &Path, cap: usize) -> Result<usize> {
    std::fs::create_dir_all(root)?;
    let mut available = 0;
    for index in 0..cap {
        let slot = lock_file(&root.join(format!("slot-{index}.lock")))?;
        match slot.try_lock_exclusive() {
            Ok(()) => available += 1,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(available)
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    // cas-39f3: the runner only reads git state. `git status` otherwise
    // rewrites a stat-dirty index under the worker's `index.lock`, and a git
    // killed during that write strands a zero-byte lock that blocks commits.
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(repo)
        .output()?;
    if !output.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

const HOST_PATH_OPT_OUT: &str = "CAS_WORKER_TEST_ALLOW_HOST_PATH";
const HEADFUL_FILTER: &str = "test(=e2e::factory_tui_headful::factory_tui_headful_reuse_window)";

/// Compose the existing explicit opt-ins rather than weakening default test isolation.
fn live_headful_mode(test: Option<&TargetedTest>, isolated_test_env: Option<&Path>, host_reason: Option<&str>) -> Result<bool> {
    if std::env::var_os("CAS_HEADFUL_E2E").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Ok(false);
    }
    if !test.is_some_and(|test| test.package == "cas"
        && test.harness.as_deref() == Some("e2e_test") && test.filter == HEADFUL_FILTER)
    {
        bail!("Live headful requires the exact headful e2e_test selection documented in CONTRIBUTING");
    }
    if host_reason.is_none() {
        bail!("Live headful requires CAS_WORKER_TEST_ALLOW_HOST_PATH with a single-line reason; run from the operator's own terminal");
    }
    if isolated_test_env.is_some() {
        bail!("Live headful cannot use the credential-free isolated-test-env profile; retain the selected Claude account environment");
    }
    if !cfg!(target_os = "macos") {
        bail!("Live headful requires macOS with Terminal.app; no live test was executed");
    }
    Ok(true)
}

fn host_path_opt_out() -> Result<Option<String>> {
    let Some(reason) = std::env::var_os(HOST_PATH_OPT_OUT) else {
        return Ok(None);
    };
    let reason = reason
        .to_str()
        .context("host PATH opt-out reason must be Unicode")?
        .trim();
    if reason.is_empty() || reason.chars().any(char::is_control) {
        bail!("{HOST_PATH_OPT_OUT} requires a nonempty, single-line reason");
    }
    Ok(Some(reason.to_string()))
}

#[cfg(unix)]
fn isolated_test_path(
    cargo: &Path,
    cwd: &Path,
    slots: &Path,
) -> Result<(tempfile::TempDir, std::ffi::OsString)> {
    let started = std::time::Instant::now();
    let bin = tempfile::tempdir_in(slots)?;
    let search = std::env::var_os("PATH").unwrap_or_default();
    let resolve = |name: &Path| {
        let candidates = if name.is_absolute() || name.components().count() > 1 {
            vec![name.to_path_buf()]
        } else {
            std::env::split_paths(&search)
                .map(|dir| dir.join(name))
                .collect()
        };
        candidates
            .into_iter()
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                }
            })
            .find(|path| executable(path))
    };
    let resolved_cargo =
        resolve(cargo).context("cannot resolve Cargo before isolating test PATH")?;
    let rustup = resolve(Path::new("rustup")).or_else(|| {
        resolved_cargo.canonicalize().ok().filter(|path| path.file_name().is_some_and(|name| name == "rustup"))
    });
    let concrete = |name: &str, tool: PathBuf| -> Result<PathBuf> {
        use std::os::unix::fs::MetadataExt;
        let tool = tool.canonicalize()?;
        let proxy = rustup.as_ref().is_some_and(|rustup| {
            tool.metadata().ok().zip(rustup.metadata().ok())
                .is_some_and(|(tool, rustup)| tool.dev() == rustup.dev() && tool.ino() == rustup.ino())
        });
        if !proxy || name == "rustup" {
            return Ok(tool);
        }
        // Rustup proxies prepend CARGO_HOME/bin when dispatching. Resolve their
        // actual toolchain binary before constructing the isolated environment.
        use crate::bounded_process::{Deadline, run_command};
        let budget = std::time::Duration::from_secs(2);
        let mut command = Command::new(rustup.as_ref().unwrap());
        command.args(["which", name]).current_dir(cwd);
        let output = run_command(&mut command, Deadline::after(budget), budget)
            .map_err(|_| anyhow::anyhow!("cannot resolve concrete {name} through rustup"))?;
        if !output.status.success() {
            bail!("rustup cannot resolve concrete {name}: {}", String::from_utf8_lossy(&output.stderr));
        }
        let direct = PathBuf::from(String::from_utf8(output.stdout)?.trim());
        if !direct.is_absolute() || !executable(&direct) || direct.canonicalize()? == tool {
            bail!("rustup did not resolve a concrete executable for {name}");
        }
        Ok(direct)
    };
    std::os::unix::fs::symlink(concrete("cargo", resolved_cargo)?, bin.path().join("cargo"))?;
    for name in ["rustc", "rustdoc", "rustup", "cargo-nextest"] {
        if let Some(tool) = resolve(Path::new(name)) {
            std::os::unix::fs::symlink(concrete(name, tool)?, bin.path().join(name))?;
        }
    }
    // cas-1c4b: hide the host harness binaries, not their directories.
    // Dropping a whole directory (e.g. /opt/homebrew/bin, which holds codex
    // and claude) silently swapped in Apple's /usr/bin/git. A directory that
    // holds a harness binary is replaced in place by a private mirror of its
    // other entries, minus rustup proxies: dispatching one would prepend
    // $CARGO_HOME/bin and restore the hidden binaries.
    const HARNESS: [&str; 3] = ["cas", "codex", "claude"];
    let rustup_proxy = |path: &Path| {
        use std::os::unix::fs::MetadataExt;
        rustup.as_ref().is_some_and(|rustup| {
            path.metadata().ok().zip(rustup.metadata().ok())
                .is_some_and(|(tool, rustup)| tool.dev() == rustup.dev() && tool.ino() == rustup.ino())
        })
    };
    let mirrors = bin.path().join("harness-free");
    let mut retained = Vec::new();
    for (index, dir) in std::env::split_paths(&search)
        .map(|dir| if dir.is_absolute() { dir } else { cwd.join(dir) })
        .enumerate()
    {
        if !HARNESS.iter().any(|name| dir.join(name).is_file()) {
            retained.push(dir);
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mirror = mirrors.join(index.to_string());
        std::fs::create_dir_all(&mirror)?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let source = entry.path();
            if HARNESS.iter().any(|harness| name == *harness) || rustup_proxy(&source) {
                continue;
            }
            std::os::unix::fs::symlink(&source, mirror.join(&name))?;
        }
        retained.push(mirror);
    }
    let path = std::env::join_paths(std::iter::once(bin.path()).chain(retained.iter().map(PathBuf::as_path)))?;
    println!(
        "Worker test PATH: concrete Rust tools + PATH with host harness binaries hidden; setup={:.3}ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok((bin, path))
}

fn clean_head(repo: &Path) -> Result<String> {
    if !git(repo, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
        bail!("Commit the worker change before checking; worker PASS must name a clean commit");
    }
    git(repo, &["rev-parse", "HEAD"])
}

/// cas-f616: set by a caller continuing a multi-step proof at one commit
/// (`scripts/check-lane-compile.py --prove` runs `--lib`, then `--tests`).
pub(crate) const CONTINUATION_ENV: &str = "CAS_WORKER_CHECK_CONTINUES_PASS";
/// How recent the preceding PASS must be to carry its load admission.
const CONTINUATION_WINDOW: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// cas-f616: whether this check continues a proof whose previous capped step
/// just passed, so it inherits that step's load admission instead of being
/// refused for the load the step itself raised. Requires the caller's
/// explicit [`CONTINUATION_ENV`] opt-in and a fresh PASS receipt for the
/// same worktree, commit and packages; an independent check never qualifies.
fn continuation_admits_load(
    receipt: &Path,
    repo: &Path,
    head: &str,
    packages: &[String],
    now: std::time::SystemTime,
) -> bool {
    let Ok(metadata) = std::fs::metadata(receipt) else {
        return false;
    };
    // A receipt stamped up to a minute ahead (filesystem/clock skew) is fresh.
    let fresh = metadata
        .modified()
        .ok()
        .is_some_and(|written| match now.duration_since(written) {
            Ok(age) => age <= CONTINUATION_WINDOW,
            Err(ahead) => ahead.duration() <= std::time::Duration::from_secs(60),
        });
    let Some(record) = std::fs::read(receipt)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CheckReceipt>(&bytes).ok())
    else {
        return false;
    };
    fresh
        && record.test.is_none()
        && record.repo == repo
        && record.head == head
        && record.packages == packages
}

fn receipt_path(cas_root: &Path, repo: &Path, head: &str) -> PathBuf {
    let key = hex::encode(Sha256::digest(repo.as_os_str().as_encoded_bytes()));
    cas_root
        .join("worker-checks")
        .join(key)
        .join(format!("{head}.json"))
}

pub(crate) fn passing_receipt(cas_root: &Path, repo: &Path, head: &str) -> Option<String> {
    let repo = repo.canonicalize().ok()?;
    let receipt: CheckReceipt =
        serde_json::from_slice(&std::fs::read(receipt_path(cas_root, &repo, head)).ok()?).ok()?;
    (receipt.test.is_none()
        && receipt.repo == repo
        && receipt.head == head
        && !receipt.packages.is_empty()
        && receipt
            .packages
            .iter()
            .all(|package| valid_package(package)))
    .then(|| format!("check: PASS {head} packages={}", receipt.packages.join(",")))
}

fn test_receipt_path(cas_root: &Path, repo: &Path, head: &str, test: &TargetedTest) -> PathBuf {
    let key = hex::encode(Sha256::digest(
        serde_json::to_vec(&(&test.package, &test.harness, &test.filter))
            .expect("serialize test identity"),
    ));
    receipt_path(cas_root, repo, head).with_file_name(format!("{head}-test-{key}.json"))
}

/// One close's evidence at an immutable delivery. Construction does no I/O;
/// package coverage and audit lines share the first read, including its Git
/// classification work. A new close gets a fresh view of receipt files.
pub(crate) struct PassingTests<'a> {
    cas_root: &'a Path,
    repo: &'a Path,
    head: &'a str,
    records: std::sync::OnceLock<Vec<(String, String, TestReceipt)>>,
}

impl<'a> PassingTests<'a> {
    pub(crate) fn new(cas_root: &'a Path, repo: &'a Path, head: &'a str) -> Self {
        Self {
            cas_root,
            repo,
            head,
            records: std::sync::OnceLock::new(),
        }
    }

    fn records(&self) -> &[(String, String, TestReceipt)] {
        self.records
            .get_or_init(|| passing_test_records(self.cas_root, self.repo, self.head))
    }

    pub(crate) fn receipts(&self) -> Vec<String> {
        let mut receipts = self.records().iter()
            .map(|(receipt_head, package, test)| {
                let head = self.head;
                let rebind = (receipt_head != head)
                    .then(|| format!(" rebound from {receipt_head} to {head}"))
                    .unwrap_or_default();
                format!(
                    "test: PASS {receipt_head} {package} {} {}{}{rebind}",
                    test.filter, test.count, test.path_note()
                )
            }).collect::<Vec<_>>();
        receipts.sort();
        receipts.dedup();
        receipts
    }

    pub(crate) fn packages(&self) -> Vec<String> {
        let mut packages = self.records().iter()
            .filter_map(|(_, package, _)| {
                let (name, version) = package_selector(package)?;
                // A dependency's other version must never cover this workspace
                // crate. The receipt/audit identity keeps the full selector.
                if version.is_some() && package_manifest(self.repo, package).is_err() {
                    return None;
                }
                Some(name.to_string())
            }).collect::<Vec<_>>();
        packages.sort();
        packages.dedup();
        packages
    }
}

#[cfg(test)]
pub(crate) fn passing_test_receipts(cas_root: &Path, repo: &Path, head: &str) -> Vec<String> {
    PassingTests::new(cas_root, repo, head).receipts()
}

/// cas-b38a: the packages with at least one passing targeted test at `head`
/// in this worktree, sorted and once each. The close gate counts a delivered
/// crate as tested when its package is listed here.
pub(crate) fn passing_test_packages(cas_root: &Path, repo: &Path, head: &str) -> Vec<String> {
    PassingTests::new(cas_root, repo, head).packages()
}

/// Exact test receipts plus ancestor receipts whose full commit range contains
/// no Rust, test or build inputs. The latter saves an identical rerun after a
/// documentation-only delivery commit while keeping proof tied to its tested
/// tree.
fn passing_test_records(
    cas_root: &Path,
    repo: &Path,
    head: &str,
) -> Vec<(String, String, TestReceipt)> {
    let Ok(repo) = repo.canonicalize() else {
        return Vec::new();
    };
    let path = receipt_path(cas_root, &repo, head);
    let Some(directory) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut exact = BTreeMap::new();
    let mut ancestors = Vec::new();
    for entry in entries {
        let Some((receipt_head, package, test)) =
            read_passing_test_record(cas_root, &repo, &entry)
        else {
            continue;
        };
        if receipt_head == head {
            exact.insert(
                (package, test.harness.clone(), test.filter.clone()),
                (receipt_head, test),
            );
        } else {
            ancestors.push((receipt_head, package, test));
        }
    }

    let mut ranges = BTreeMap::new();
    for (receipt_head, package, test) in &ancestors {
        let key = (package.clone(), test.harness.clone(), test.filter.clone());
        if exact.contains_key(&key) {
            continue;
        }
        ranges.entry(receipt_head.clone()).or_insert_with(|| {
            receipt_range_changed_paths(&repo, receipt_head, head)
        });
    }

    let changed_paths = ranges
        .values()
        .filter_map(Option::as_ref)
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    let proof_inputs = delivered_tree_proof_inputs(&repo, head, &changed_paths)
        .unwrap_or_else(|| changed_paths.clone());

    for (receipt_head, package, test) in ancestors {
        let key = (package.clone(), test.harness.clone(), test.filter.clone());
        let Some(changes) = ranges.get(&receipt_head).and_then(Option::as_ref) else {
            continue;
        };
        if exact.contains_key(&key) || changes.iter().any(|path| proof_inputs.contains(path)) {
            continue;
        }
        match exact.entry(key) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert((receipt_head, test));
            }
            std::collections::btree_map::Entry::Occupied(mut slot)
                if receipt_head_is_ancestor(&repo, &slot.get().0, &receipt_head) =>
            {
                slot.insert((receipt_head, test));
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }

    exact
        .into_iter()
        .map(|((package, _, _), (receipt_head, test))| (receipt_head, package, test))
        .collect()
}

fn read_passing_test_record(
    cas_root: &Path,
    repo: &Path,
    entry: &std::fs::DirEntry,
) -> Option<(String, String, TestReceipt)> {
    if !entry.file_type().ok()?.is_file() {
        return None;
    }
    let mut record = serde_json::from_slice::<CheckReceipt>(&std::fs::read(entry.path()).ok()?).ok()?;
    let test = record.test.take()?;
    if record.repo != repo
        || record.packages.len() != 1
        || package_selector(&record.packages[0]).is_none()
        || !named_filter(&test.filter)
        || test.count == 0
        || test
            .harness
            .as_ref()
            .is_some_and(|harness| !valid_package(harness))
    {
        return None;
    }
    let package = record.packages.remove(0);
    let targeted_test = TargetedTest {
        package: package.clone(),
        harness: test.harness.clone(),
        filter: test.filter.clone(),
    };
    (entry.path() == test_receipt_path(cas_root, repo, &record.head, &targeted_test))
        .then_some((record.head, package, test))
}

fn receipt_range_changed_paths(
    repo: &Path,
    receipt_head: &str,
    delivered_head: &str,
) -> Option<Vec<String>> {
    if !valid_git_oid(receipt_head)
        || !valid_git_oid(delivered_head)
        || !receipt_head_is_ancestor(repo, receipt_head, delivered_head)
    {
        return None;
    }
    let range = format!("{receipt_head}..{delivered_head}");
    let Ok(changes) = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "log",
            "--format=",
            "--name-only",
            "--no-renames",
            "-m",
            "-z",
            &range,
        ])
        .current_dir(repo)
        .output()
    else {
        return None;
    };
    if !changes.status.success() {
        return None;
    }
    changes
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| std::str::from_utf8(path).map(str::to_owned).ok())
        .collect()
}

fn receipt_head_is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    valid_git_oid(ancestor)
        && valid_git_oid(descendant)
        && Command::new("git")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .current_dir(repo)
            .status()
            .is_ok_and(|status| status.success())
}

fn valid_git_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_test_or_build_input_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower == ".env"
        || lower.ends_with(".capture.txt")
        || lower == "hub-web/dist"
        || lower.starts_with("hub-web/dist/")
    {
        return true;
    }
    let parts = lower.split('/').collect::<Vec<_>>();
    let name = parts.last().copied().unwrap_or_default();
    if parts.iter().any(|part| {
        matches!(
            *part,
            "test" | "tests" | "__tests__" | "spec" | "specs" | "fixture" | "fixtures"
                | "testdata" | "snapshots" | "benches" | "build" | "scripts" | "ci"
                | ".cargo" | ".github" | "xtask"
        )
    }) {
        return true;
    }
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    if name.starts_with("test_")
        || name.starts_with("test-")
        || [".test.", "_test.", "-test.", ".spec.", "_spec.", "-spec."]
            .iter()
            .any(|marker| name.contains(marker))
        || matches!(stem, "test" | "spec")
        || matches!(
            name,
            "cargo.toml"
                | "cargo.lock"
                | "build.rs"
                | "makefile"
                | "gnumakefile"
                | "justfile"
                | "taskfile"
                | "cmakelists.txt"
                | "build.ninja"
                | "dockerfile"
                | "rust-toolchain"
                | "rust-toolchain.toml"
                | ".rust-toolchain"
                | "nextest.toml"
                | "clippy.toml"
                | "rustfmt.toml"
                | ".rustfmt.toml"
        )
    {
        return true;
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension).unwrap_or_default();
    matches!(
        extension,
        "rs" | "c" | "cc" | "cpp" | "h" | "hpp" | "go" | "py" | "pyi" | "js"
            | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "sh" | "bash" | "java" | "kt"
            | "kts" | "cs" | "php" | "swift" | "lua" | "sql" | "pl" | "r" | "ex"
            | "exs" | "scala" | "clj" | "vue" | "svelte" | "toml" | "lock" | "json"
            | "json5" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "properties" | "xml"
            | "proto" | "graphql" | "nix"
    )
}

/// Read the delivered tree once per close and batch its Rust sources once.
fn delivered_tree_proof_inputs(repo: &Path, head: &str, changed: &BTreeSet<String>) -> Option<BTreeSet<String>> {
    if changed.is_empty() { return Some(BTreeSet::new()); }
    let tree = delivered_tree(repo, head)?;
    let mut inputs = BTreeSet::new();
    let external = changed.iter().filter(|path| {
        if is_test_or_build_input_path(path) || tree.is_workspace_crate_path(path) {
            inputs.insert((*path).clone()); false
        } else { true }
    }).cloned().collect::<Vec<_>>();
    if external.is_empty() { return Some(inputs); }
    for (source_path, source) in tree.rust_sources(repo)? {
        let manifest = tree.manifest_dir_for_source(source_path);
        for path in &external {
            if source_references_path(source, source_path, manifest.as_deref(), path) {
                inputs.insert(path.clone());
            }
        }
    }
    Some(inputs)
}

struct DeliveredTree {
    manifests: BTreeSet<String>,
    sources: Vec<(String, String)>,
    source_bodies: std::sync::OnceLock<Option<Vec<(String, String)>>>,
}

/// Share the immutable delivered-tree scan among proof reads for the same tip.
fn delivered_tree(repo: &Path, head: &str) -> Option<std::sync::Arc<DeliveredTree>> {
    type CachedTree = (String, std::sync::Arc<DeliveredTree>);
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<CachedTree>>> = std::sync::OnceLock::new();
    let mut cache = CACHE.get_or_init(|| std::sync::Mutex::new(None)).lock().ok()?;
    if let Some((cached_head, tree)) = cache.as_ref()
        && cached_head == head
    {
        return Some(std::sync::Arc::clone(tree));
    }
    let tree = std::sync::Arc::new(DeliveredTree::load(repo, head)?);
    *cache = Some((head.to_owned(), std::sync::Arc::clone(&tree)));
    Some(tree)
}

impl DeliveredTree {
    fn load(repo: &Path, head: &str) -> Option<Self> {
        let tree = Command::new("git").env("GIT_OPTIONAL_LOCKS", "0")
            .args(["ls-tree", "-r", "-z", "--full-tree", head]).current_dir(repo).output().ok()?;
        if !tree.status.success() { return None; }
        let (mut manifests, mut sources) = (BTreeSet::new(), Vec::new());
        for row in tree.stdout.split(|b| *b == 0).filter(|row| !row.is_empty()) {
            let tab = row.iter().position(|b| *b == b'\t')?;
            let mut fields = row[..tab].split(|b| b.is_ascii_whitespace());
            fields.next()?;
            if fields.next()? != b"blob" { continue; }
            let oid = std::str::from_utf8(fields.next()?).ok()?.to_owned();
            let path = std::str::from_utf8(&row[tab + 1..]).ok()?.to_owned();
            if path == "Cargo.toml" { manifests.insert(String::new()); }
            if let Some(dir) = path.strip_suffix("/Cargo.toml") { manifests.insert(dir.to_owned()); }
            if path.ends_with(".rs") { sources.push((path, oid)); }
        }
        Some(Self { manifests, sources, source_bodies: std::sync::OnceLock::new() })
    }

    fn is_workspace_crate_path(&self, path: &str) -> bool {
        let mut dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        while !dir.is_empty() {
            if self.manifests.contains(dir) { return true; }
            dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
        false
    }

    fn manifest_dir_for_source(&self, path: &str) -> Option<String> {
        let mut dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        loop {
            if self.manifests.contains(dir) { return Some(dir.to_owned()); }
            match dir.rsplit_once('/') {
                Some((parent, _)) => dir = parent,
                None => return self.manifests.contains("").then(String::new),
            }
        }
    }

    fn rust_sources(&self, repo: &Path) -> Option<&[(String, String)]> {
        self.source_bodies.get_or_init(|| {
            if self.sources.is_empty() { return Some(Vec::new()); }
            let mut child = Command::new("git").env("GIT_OPTIONAL_LOCKS", "0")
                .args(["cat-file", "--batch"]).current_dir(repo)
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().ok()?;
            use std::io::{Read, Write};
            let mut input = child.stdin.take()?;
            let oids = self.sources.iter().map(|(_, oid)| oid.clone()).collect::<Vec<_>>();
            let writer = std::thread::spawn(move || {
                for oid in oids { writeln!(input, "{oid}")?; }
                Ok::<_, std::io::Error>(())
            });
            let mut stdout = child.stdout.take()?;
            let mut output = Vec::new();
            let read_result = stdout.read_to_end(&mut output);
            drop(stdout);
            let status = child.wait().ok()?;
            let write_result = writer.join().ok()?;
            if !status.success() || read_result.is_err() || write_result.is_err() { return None; }
            let mut cursor = 0;
            let bodies = self.sources.iter().map(|(path, oid)| {
                let end = cursor + output.get(cursor..)?.iter().position(|b| *b == b'\n')?;
                let mut fields = output[cursor..end].split(|b| b.is_ascii_whitespace());
                if fields.next()? != oid.as_bytes() || fields.next()? != b"blob" { return None; }
                let size = std::str::from_utf8(fields.next()?).ok()?.parse::<usize>().ok()?;
                cursor = end + 1;
                let body_end = cursor.checked_add(size)?;
                let body = std::str::from_utf8(output.get(cursor..body_end)?).ok()?.to_owned();
                if output.get(body_end) != Some(&b'\n') { return None; }
                cursor = body_end + 1;
                Some((path.clone(), body))
            }).collect::<Option<Vec<_>>>()?;
            (cursor == output.len()).then_some(bodies)
        }).as_deref()
    }
}

fn source_references_path(source: &str, source_path: &str, manifest: Option<&str>, target: &str) -> bool {
    let dir = source_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    if let Some(name) = target.rsplit('/').next()
        && source.contains(name)
        && (source.contains(target)
            || relative_repo_path(dir, target).is_some_and(|relative| source.contains(&relative)))
    {
        return true;
    }
    [Some(dir), manifest].into_iter().flatten().any(|base| {
        let Some(relative) = relative_repo_path(base, target) else { return false; };
        if normalize_repo_path(base, &relative).as_deref() != Some(target) { return false; }
        match relative.rsplit_once('/') {
            Some((parent, name)) => source.contains(&relative)
                || (source.contains(&format!("{parent}/")) && source.contains(name)),
            None => source.contains(&relative),
        }
    })
}

fn relative_repo_path(base: &str, target: &str) -> Option<String> {
    let from = normalize_repo_path("", base)?;
    let to = normalize_repo_path("", target)?;
    let from = from.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    let to = to.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    Some(std::iter::repeat("..").take(from.len() - common)
        .chain(to[common..].iter().copied()).collect::<Vec<_>>().join("/"))
}

fn normalize_repo_path(base: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\0') { return None; }
    let mut parts = Vec::new();
    for part in base.split('/').chain(path.split('/')) {
        match part { "" | "." => {}, ".." => { parts.pop()?; }, name => parts.push(name) }
    }
    Some(parts.join("/"))
}

/// Write the receipt a passing targeted run leaves, for tests of the gates
/// that read it (cas-b38a).
#[cfg(test)]
pub(crate) fn record_passing_test_for_tests(
    cas_root: &Path,
    repo: &Path,
    head: &str,
    package: &str,
    filter: &str,
) {
    let repo = repo.canonicalize().expect("canonical repo");
    let test = TargetedTest {
        package: package.to_string(),
        harness: None,
        filter: filter.to_string(),
    };
    let path = test_receipt_path(cas_root, &repo, head, &test);
    std::fs::create_dir_all(path.parent().expect("receipt directory"))
        .expect("create receipt directory");
    let record = CheckReceipt {
        head: head.to_string(),
        repo,
        packages: vec![package.to_string()],
        test: Some(TestReceipt {
            filter: filter.to_string(),
            harness: None,
            count: 1,
            host_path_opt_out: None,
            supervisor_recorded: None,
        }),
    };
    std::fs::write(
        path,
        serde_json::to_vec(&record).expect("serialize receipt"),
    )
    .expect("write receipt");
}

// Shared with parked-cache eviction: a cache cannot be evicted while its
// capped builder owns the lane, even between process inspection and spawn.
pub(crate) struct LaneLock {
    file: File,
    stale_holder: Option<LaneLockHolder>,
}
impl Drop for LaneLock {
    fn drop(&mut self) {
        // The file itself remains as the stable flock inode, but owner details
        // should survive only a crashed holder, not a clean release.
        let _ = self.file.set_len(0);
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct LaneLockHolder {
    pid: u32,
    command: String,
    started_at: String,
}

pub(crate) fn try_lock_lane(cas_root: &Path, repo: &Path) -> std::io::Result<Option<LaneLock>> {
    try_lock_lane_with_command(cas_root, repo, "factory lane operation")
}

fn try_lock_lane_with_command(
    cas_root: &Path,
    repo: &Path,
    command: &str,
) -> std::io::Result<Option<LaneLock>> {
    let slots = cas_root.join("worker-check-slots");
    std::fs::create_dir_all(&slots)?;
    let mut lock = lock_file(&lane_lock_path(cas_root, repo)?).map_err(std::io::Error::other)?;
    match lock.try_lock_exclusive() {
        Ok(()) => {
            use std::io::{Read, Seek, SeekFrom, Write};
            lock.seek(SeekFrom::Start(0))?;
            let mut old_contents = String::new();
            lock.read_to_string(&mut old_contents)?;
            let stale_holder = serde_json::from_str::<LaneLockHolder>(&old_contents)
                .ok()
                .filter(|holder| lane_holder_is_alive(holder.pid) == Some(false));
            let holder = LaneLockHolder {
                pid: std::process::id(),
                command: sanitize_lane_command(command),
                started_at: chrono::Utc::now()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            };
            lock.set_len(0)?;
            lock.seek(SeekFrom::Start(0))?;
            serde_json::to_writer(&mut lock, &holder).map_err(std::io::Error::other)?;
            lock.flush()?;
            Ok(Some(LaneLock {
                file: lock,
                stale_holder,
            }))
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error),
    }
}

fn sanitize_lane_command(command: &str) -> String {
    command
        .chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .take(240)
        .collect::<String>()
        .trim()
        .to_string()
}

fn worker_check_command(args: &[String]) -> String {
    let suffix = args.join(" ");
    // The executable is stable even when a private Cargo path is used for
    // isolated tests; the suffix is exactly the admitted worker command.
    let command = if suffix.is_empty() {
        "cargo".to_string()
    } else {
        format!("cargo {suffix}")
    };
    sanitize_lane_command(&command)
}

fn lane_lock_refusal(path: &Path) -> anyhow::Error {
    let details = std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<LaneLockHolder>(&contents).ok())
        .map(|holder| lane_holder_description(&holder, chrono::Utc::now()))
        .unwrap_or_else(|| {
            "holder details are unavailable; wait for it or retry later".to_string()
        });
    anyhow::anyhow!("This worktree already has a worker check; {details}")
}

fn stale_lane_holder_notice(holder: &LaneLockHolder, now: chrono::DateTime<chrono::Utc>) -> String {
    let age = lane_holder_age(holder, now);
    let command = sanitize_lane_command(&holder.command);
    format!(
        "worker-check lane: stale previous holder metadata for pid {} ({command}), started {age} ago; continuing with the lane",
        holder.pid
    )
}

fn lane_holder_description(holder: &LaneLockHolder, now: chrono::DateTime<chrono::Utc>) -> String {
    let age = lane_holder_age(holder, now);
    let start = sanitize_lane_command(&holder.started_at);
    let command = sanitize_lane_command(&holder.command);
    let when = format!("started {age} ago ({start})");
    match lane_holder_is_alive(holder.pid) {
        Some(true) => format!(
            "held by pid {} ({command}), {when}; wait for it or stop that check",
            holder.pid
        ),
        Some(false) => format!(
            "stale holder metadata for pid {} ({command}), {when}; an OS lock is still held, so inspect its owner",
            holder.pid
        ),
        None => format!(
            "holder pid {} ({command}) has unknown liveness, {when}; wait for it or retry later",
            holder.pid
        ),
    }
}

fn lane_holder_age(holder: &LaneLockHolder, now: chrono::DateTime<chrono::Utc>) -> String {
    chrono::DateTime::parse_from_rfc3339(&holder.started_at)
        .ok()
        .map(|started| {
            let seconds = (now - started.with_timezone(&chrono::Utc)).num_seconds().max(0);
            if seconds >= 3600 {
                format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
            } else if seconds >= 60 {
                format!("{}m", seconds / 60)
            } else {
                format!("{seconds}s")
            }
        })
        .unwrap_or_else(|| "an unknown time".to_string())
}

#[cfg(unix)]
fn lane_holder_is_alive(pid: u32) -> Option<bool> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Some(false);
    }
    // SAFETY: signal zero only checks whether this recorded PID exists.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        Some(true)
    } else {
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => Some(false),
            Some(libc::EPERM) => Some(true),
            _ => None,
        }
    }
}

#[cfg(not(unix))]
fn lane_holder_is_alive(_pid: u32) -> Option<bool> {
    None
}

fn lane_lock_path(cas_root: &Path, repo: &Path) -> std::io::Result<PathBuf> {
    // GC must lock the same lane after its checkout has been removed. Never
    // resolve arbitrary missing paths or dangling symlinks as worker lanes.
    let repo = repo.canonicalize().or_else(|error| {
        if crate::factory_scratch::removed_worker(cas_root, repo) {
            Ok(repo.to_path_buf())
        } else { Err(error) }
    })?;
    let key = hex::encode(Sha256::digest(repo.as_os_str().as_encoded_bytes()));
    Ok(cas_root.join("worker-check-slots").join(format!("lane-{key}.lock")))
}

/// Positive, read-only evidence of a capped check waiting for admission or
/// executing in this worktree. A leftover lock file is not a running build.
/// Never take even a shared lock here: it can refuse a concurrent builder.
#[cfg(target_os = "linux")]
pub(crate) fn lane_is_busy(cas_root: &Path, repo: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let metadata = match std::fs::metadata(lane_lock_path(cas_root, repo)?) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(flock_held_on_inode(&std::fs::read_to_string("/proc/locks")?, metadata.ino()))
}

/// Whether a `/proc/locks` table shows a held FLOCK on this inode. The
/// device field is not compared: on btrfs subvolumes and overlayfs it can
/// differ from the `stat` device, which hid a running build (cas-c53a).
#[cfg(target_os = "linux")]
fn flock_held_on_inode(locks: &str, inode: u64) -> bool {
    locks.lines().any(|line| {
        let mut fields = line.split_whitespace();
        // Waiters have `->` before FLOCK; only actual holders count.
        fields.next().is_some()
            && fields.next() == Some("FLOCK")
            && fields
                .nth(3)
                .and_then(|identity| identity.rsplit(':').next())
                .and_then(|ino| ino.parse::<u64>().ok())
                == Some(inode)
    })
}

/// No passive lock table is available on these platforms. Unknown evidence
/// leaves the idle signal alone rather than competing for build admission.
#[cfg(not(target_os = "linux"))]
pub(crate) fn lane_is_busy(_cas_root: &Path, _repo: &Path) -> std::io::Result<bool> {
    Ok(false)
}

#[cfg(unix)]
fn discard_shared_fingerprints(target: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if !target.exists() {
        return Ok(());
    }
    // Old seeders shared mutable freshness records. Their contents may already
    // describe another lane's rebuild, so copying them now would preserve a
    // poisoned cache. Drop only affected fingerprint directories; Cargo will
    // rebuild those units and keep unrelated private metadata/artifacts.
    // Profiles live under target/<profile> or target/<triple>/<profile>.
    let mut roots = Vec::new();
    for entry in walkdir::WalkDir::new(target)
        .max_depth(3)
        .follow_links(false)
    {
        let entry = entry?;
        if entry.file_type().is_dir() && entry.file_name() == ".fingerprint" {
            roots.push(entry.into_path());
        }
    }
    for root in roots {
        for unit in std::fs::read_dir(root)? {
            let unit = unit?;
            if !unit.file_type()?.is_dir() {
                continue;
            }
            let mut shared = false;
            for record in std::fs::read_dir(unit.path())? {
                let record = record?;
                if record.file_type()?.is_file() && record.metadata()?.nlink() > 1 {
                    shared = true;
                    break;
                }
            }
            if shared {
                std::fs::remove_dir_all(unit.path())?;
            }
        }
    }
    Ok(())
}

/// The fixed provider credential-free test profile; never an arbitrary env override.
pub(crate) const ISOLATED_TEST_CREDENTIALS: [&str; 6] = [
    "VIKTOR_API_KEY", "VIKTOR_KEY", "NEON_API_KEY", "NEON_TOKEN", "VERCEL_TOKEN", "VERCEL_API_TOKEN",
];

pub(crate) fn execute(cas_root: &Path, args: &[String], isolated_test_env: Option<&Path>) -> Result<()> {
    execute_at_with_scope(
        cas_root,
        args,
        &std::env::current_dir()?,
        Path::new("cargo"),
        cfg!(target_os = "linux").then_some(Path::new("systemd-run")),
        isolated_test_env,
    )
}

/// Run the capped runner in its own session and return once it has started
/// (cas-3d04). The worker guard routes a backgrounded `... > log 2>&1 &` here
/// instead of keeping the shell's `&`: Codex `exec_command` SIGKILLs the
/// shell's whole process group as soon as the shell exits
/// (codex-rs/utils/pty `kill_process_group` via `UnifiedExecProcess::drop`),
/// so a `&` job died before writing a byte and the call still exited 0. A new
/// session is outside that group. Its stdout and stderr both go to this
/// process's stdout (the shell's log redirect), never the harness pipe.
#[cfg(unix)]
pub(crate) fn execute_detached(cas_root: &Path, args: &[String], isolated_test_env: Option<&Path>) -> Result<()> {
    if !allowed_args(args) {
        bail!("Only package-scoped cargo check or nextest run -p <crate> [--lib|--test <harness>] -E 'test(name)' is allowed");
    }
    let command = detached_runner_command(&std::env::current_exe()?, cas_root, args, isolated_test_env);
    let child = spawn_detached(command).context("starting the detached worker-check runner")?;
    println!(
        "worker-check: detached pid {} in its own session; this log receives its output and receipt",
        child.id()
    );
    Ok(())
}

/// The detached runner, ending its log with `worker-check: EXIT <code>` so a
/// refused or failed run is distinguishable from one still running (cas-6591).
#[cfg(unix)]
fn detached_runner_command(executable: &Path, cas_root: &Path, args: &[String], isolated_test_env: Option<&Path>) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", r#""$0" "$@"; echo "worker-check: EXIT $?""#])
        .arg(executable)
        .args(["factory", "worker-check", "--cas-root"])
        .arg(cas_root);
    if let Some(directory) = isolated_test_env {
        command.arg("--isolated-test-env").arg(directory);
    }
    command.arg("--").args(args);
    command
}

/// Spawn `command` as a new session leader with stdin closed and both output
/// streams on this process's stdout.
#[cfg(unix)]
fn spawn_detached(mut command: Command) -> std::io::Result<std::process::Child> {
    use std::os::fd::AsFd;
    use std::os::unix::process::CommandExt;
    let log = std::io::stdout().as_fd().try_clone_to_owned()?;
    command
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe and touches no parent state.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}

#[cfg(not(unix))]
pub(crate) fn execute_detached(_cas_root: &Path, _args: &[String], _isolated_test_env: Option<&Path>) -> Result<()> {
    bail!("worker-check --detach needs a Unix session; run the check in the foreground")
}

#[cfg(test)]
fn execute_at(cas_root: &Path, args: &[String], cwd: &Path, cargo: &Path) -> Result<()> {
    execute_at_with_scope(cas_root, args, cwd, cargo, None, None)
}

/// cas-d95c: run a task's named tests at a verification's bound head under the
/// same admission as a worker check (PSI and builder cap, a counted builder
/// slot, a private target, the zero-test guard). A verifier otherwise has no
/// clean checkout of that head under the worktrees root, and fell back to
/// plain Cargo that no admission saw.
pub(crate) fn execute_verifier(
    cas_root: &Path,
    task_id: &str,
    dispatch_id: Option<&str>,
    head: &str,
    args: &[String],
) -> Result<()> {
    execute_verifier_at(
        cas_root,
        task_id,
        dispatch_id,
        head,
        args,
        &std::env::current_dir()?,
        Path::new("cargo"),
        cfg!(target_os = "linux").then_some(Path::new("systemd-run")),
    )
}

fn execute_verifier_at(
    cas_root: &Path,
    task_id: &str,
    dispatch_id: Option<&str>,
    head: &str,
    args: &[String],
    cwd: &Path,
    cargo: &Path,
    scope_runner: Option<&Path>,
) -> Result<()> {
    if targeted_test(args).is_none() {
        bail!(
            "Verifier tests run named tests only: `nextest run -p <crate> [--lib|--test <harness>] -E 'test(module::name)'`"
        );
    }
    let checkout =
        crate::factory_review_scratch::create(cas_root, cwd, task_id, dispatch_id, head)?;
    execute_at_with_scope(cas_root, args, &checkout.path, cargo, scope_runner, None)
}

/// Prefer the active task's declared integration target, never the pushed
/// worker branch's upstream (which would erase the task diff after a push).
fn proof_diff_base(cas_root: &Path, repo: &Path, configured: Option<&str>) -> Result<String> {
    let mut target = configured.map(str::to_string);
    if let Ok(session) = std::env::var("CAS_SESSION_ID") {
        let agents = crate::store::open_agent_store(cas_root)?;
        if let Ok(agent) = agents.get(&session) {
            let tasks = crate::store::open_task_store(cas_root)?;
            for lease in agents.list_agent_leases(&agent.id)? {
                let task = tasks.get(&lease.task_id)?;
                if task.status == crate::types::TaskStatus::InProgress {
                    target = task.deliverables.work_target.map(|work| work.target_branch).or(target);
                    break;
                }
            }
        }
    }
    if let Some(target) = target {
        let remote = format!("refs/remotes/origin/{target}");
        return git(repo, &["merge-base", &remote, "HEAD"])
            .or_else(|_| git(repo, &["merge-base", &format!("refs/heads/{target}"), "HEAD"]));
    }
    // Standalone checkouts and hermetic fixtures have no factory assignment.
    git(repo, &["merge-base", "refs/remotes/origin/HEAD", "HEAD"])
        .or_else(|_| git(repo, &["rev-parse", "HEAD^"]))
        .or_else(|_| git(repo, &["rev-parse", "HEAD"]))
}

fn execute_at_with_scope(cas_root: &Path, args: &[String], cwd: &Path, cargo: &Path, scope_runner: Option<&Path>, isolated_test_env: Option<&Path>) -> Result<()> {
    let test = targeted_test(args);
    if let Some(directory) = isolated_test_env {
        if test.is_none() || !directory.is_absolute() || !directory.is_dir() {
            bail!("Isolated worker test environment needs a named targeted test and an existing absolute private XDG_CONFIG_HOME directory");
        }
    }
    let packages = match &test {
        Some(test) => vec![test.package.clone()],
        None => check_packages(args).context("Only package-scoped cargo check or nextest run -p <crate> [--lib|--test <harness>] -E 'test(name)' is allowed")?,
    };
    let repo = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let cas_root = cas_root.canonicalize()?;
    // Check only isolated, seeded worker caches; never use a shared target.
    if !repo.starts_with(cas_root.join("worktrees"))
        && !crate::factory_review_scratch::is_managed_checkout(&cas_root, &repo) {
        bail!(
            "Worker check requires an isolated worktree under {}",
            cas_root.join("worktrees").display()
        );
    }
    let head = clean_head(&repo)?;
    if let Some(test) = &test {
        validate_harness(&repo, test)?;
    }
    let config = crate::config::Config::load(&cas_root)?.factory();
    let slots = cas_root.join("worker-check-slots");
    std::fs::create_dir_all(&slots)?;
    // One check per lane also prevents concurrent PASS/FAIL receipts at the
    // same commit from overwriting one another.
    let lane_key = hex::encode(Sha256::digest(repo.as_os_str().as_encoded_bytes()));
    let lane_path = lane_lock_path(&cas_root, &repo)?;
    let _lane = try_lock_lane_with_command(&cas_root, &repo, &worker_check_command(args))?
        .ok_or_else(|| lane_lock_refusal(&lane_path))?;
    if let Some(stale) = &_lane.stale_holder {
        eprintln!("{}", stale_lane_holder_notice(stale, chrono::Utc::now()));
    }
    crate::factory_target_cache::parked::resume(&cas_root, &repo)?;
    let receipt = match &test {
        Some(test) => test_receipt_path(&cas_root, &repo, &head, test),
        None => receipt_path(&cas_root, &repo, &head),
    };
    // cas-f616: decided before the earlier PASS is removed below.
    let load_admitted = test.is_none()
        && std::env::var_os(CONTINUATION_ENV).is_some_and(|value| value == "1")
        && continuation_admits_load(&receipt, &repo, &head, &packages, std::time::SystemTime::now());
    // A failed retry must not leave an earlier PASS at this SHA.
    match std::fs::remove_file(&receipt) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let host_path_opt_out = if test.is_some() { host_path_opt_out()? } else { None };
    let live_headful = live_headful_mode(test.as_ref(), isolated_test_env, host_path_opt_out.as_deref())?;
    if live_headful {
        println!("Live headful: enabled; host PATH and Claude credentials inherited");
        println!("Factory launch remains operator-only; normal build caps apply");
    } else if test.as_ref().is_some_and(|test| test.harness.as_deref() == Some("e2e_test")
        && test.filter.contains("factory_tui_headful")) {
        println!("Live headful: NOT EXERCISED (CAS_HEADFUL_E2E=1 not set)");
    }
    let admission = lock_file(&slots.join("admission.lock"))?;
    admission.lock_exclusive()?;
    let snapshot = crate::factory_build_guard::inspect(&cas_root, &config, 1);
    let violations = snapshot.admission_violations(load_admitted);
    if !violations.is_empty() {
        let refusal = recorded_refusal(&cas_root, &repo, &violations.join("; "));
        drop(admission);
        return Err(refusal);
    }
    if load_admitted && !snapshot.violations().is_empty() {
        println!(
            "Worker check: continuing the proof that just passed at {head}; its CPU admission carries over (load_1m={:?}, cpu_count={})",
            snapshot.load_1m, snapshot.cpu_count
        );
    }
    // OS locks close the snapshot-to-spawn race, even when the soft guard's
    // environment override is set. No force/disable option bypasses the cap.
    let available_slots = available_slot_count(&slots, config.max_concurrent_builders)?;
    match crate::factory_capacity_wait::priority_refusal(
        &cas_root,
        &repo,
        chrono::Utc::now(),
        available_slots,
    ) {
        Ok(Some(cause)) => {
            let refusal = recorded_refusal(&cas_root, &repo, &cause);
            drop(admission);
            return Err(refusal);
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                %error,
                checkout = %repo.display(),
                "capacity priority lookup failed; continuing build admission"
            );
        }
    }
    let slot = match acquire_slot(&slots, config.max_concurrent_builders) {
        Ok(slot) => slot,
        Err(error) => {
            if let Some(refusal) = error.downcast_ref::<CapacityRefusal>() {
                let refusal = recorded_refusal(&cas_root, &repo, &refusal.0);
                drop(admission);
                return Err(refusal);
            }
            drop(admission);
            return Err(error);
        }
    };
    if let Err(error) = crate::factory_capacity_wait::admitted(&cas_root, &repo) {
        tracing::warn!(%error, "capacity wait clear failed; continuing admitted build");
    }
    // A parked target may contain proof logs but no debug artifacts. Seed only
    // missing children from the published immutable baseline under the lane lock.
    if let Err(error) = crate::ui::factory::seed_worker_target_from_baseline(&cas_root, &repo) {
        tracing::warn!(%error, "worker target re-seed skipped; Cargo will rebuild privately");
    }
    if let Err(error) = crate::factory_review_scratch::seed_target_from_worker(&cas_root, &repo) {
        tracing::warn!(%error, "review worker seed skipped; Cargo will rebuild privately");
    }
    #[cfg(unix)]
    let target_lease = crate::factory_target_cache::owner::acquire(&cas_root, &repo)?;
    // cas-3f93: the private target, relocated when `[factory] scratch_root` is set.
    let target_dir = crate::factory_scratch::worker_target_dir(&repo);
    #[cfg(unix)]
    discard_shared_fingerprints(&target_dir)?;
    let count_file = slots.join(format!("count-{lane_key}"));
    if let Some(reason) = &host_path_opt_out {
        println!("Worker test PATH: host opt-out: {reason}");
    }
    #[cfg(unix)]
    let test_path = if test.is_some() && host_path_opt_out.is_none() {
        Some(isolated_test_path(cargo, cwd, &slots)?)
    } else { None };
    #[cfg(unix)]
    let private_cargo = test_path.as_ref().map(|(bin, _)| bin.path().join("cargo"));
    #[cfg(unix)]
    let cargo = private_cargo.as_deref().unwrap_or(cargo);
    let mut command = if test.is_some() {
        // Embed the shared zero-test guard so projects need no cas-src scripts.
        let verified = include_str!("../../scripts/run-verified-tests.sh");
        let mut command = if repo.join("cas-cli/Cargo.toml").is_file()
            && repo.join("scripts/check-test-shape.py").is_file()
        {
            let mut guarded = Command::new("python3");
            guarded.env("PYTHONDONTWRITEBYTECODE", "1");
            guarded.arg("-c").arg(include_str!("../../scripts/worker-proof-guards.py"))
                .arg("--base").arg(proof_diff_base(&cas_root, &repo, config.epic_base_branch.as_deref())?)
                .arg("--verified-script").arg(verified)
                .arg("--cargo").arg(cargo)
                .arg("--count-file").arg(&count_file)
                .arg("--");
            guarded
        } else {
            let mut plain = Command::new("bash");
            plain.arg("-c").arg(verified).arg("worker-verified-tests");
            plain
        };
        command
            .env("VERIFIED_TEST_REPO_ROOT", &repo)
            .env("VERIFIED_TEST_COUNT_FILE", &count_file)
            .env_remove("VERIFIED_TEST_LOG")
            .env("CARGO", cargo);
        command.env("CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS", "120");
        command
    } else {
        let mut command = Command::new(cargo);
        command.arg("check");
        // cas-edeb: a nested check must not reuse an outer test's lease runner.
        if let Some(variable) = crate::factory_target_cache::owner::host_runner_env() {
            command.env_remove(variable);
        }
        command
    };
    #[cfg(unix)]
    if test.is_some()
        && target_lease.is_none()
        && let Some(variable) = crate::factory_target_cache::owner::host_runner_env()
    {
        // Managed review checkouts have no lease-aware runner to replace this value.
        command.env_remove(variable);
    }
    #[cfg(unix)]
    if let Some((_, path)) = &test_path {
        command.env("PATH", path);
    }
    let mut cargo_args = args.to_vec();
    if live_headful {
        cargo_args.extend(["--success-output".into(), "immediate".into()]);
    }
    // An omitted harness means library tests; do not build every test binary.
    if test.as_ref().is_some_and(|test| test.harness.is_none())
        && cargo_args.get(4).is_some_and(|arg| arg == "-E")
    {
        cargo_args.insert(4, "--lib".into());
    }
    command
        .args(&cargo_args)
        .current_dir(&repo)
        .env("CARGO_TARGET_DIR", &target_dir)
        // Never let a long-lived compiler cache retain the target lease.
        .env("RUSTC_WRAPPER", "")
        .env("RUSTC_WORKSPACE_WRAPPER", "");
    // Bootstrap/admission keep their inherited identity and credentials. Only
    // the final Cargo/test child receives this fixed isolation profile.
    if let Some(directory) = isolated_test_env {
        for key in ISOLATED_TEST_CREDENTIALS { command.env_remove(key); }
        command.env("XDG_CONFIG_HOME", directory);
        println!("Worker test environment: provider credentials removed; private XDG_CONFIG_HOME");
    }
    let scope_directory = tempfile::tempdir_in(&slots)?;
    let high = crate::factory_build_memory::memory_high(
        config.worker_memory_high_gib,
        config.max_concurrent_builders,
        &std::fs::read_to_string("/proc/meminfo").unwrap_or_default(),
    );
    let mut command = crate::factory_build_memory::scope_command(
        command, scope_runner, high, &scope_directory.path().join("started"),
    );
    #[cfg(unix)]
    if let Some(lease) = &target_lease {
        lease.inherit(&mut command);
        // cas-4ad0: test binaries start through a runner that closes the
        // inherited lease, so a test's leaked child cannot hold the worktree.
        if test.is_some()
            && let Some(variable) = crate::factory_target_cache::owner::host_runner_env()
        {
            let runner = crate::factory_target_cache::owner::write_release_runner(&slots)?;
            command.env(variable, runner);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(unix)]
    let worker_signals = WorkerWaitSignals::install()?;
    let mut child = command
        .spawn()
        .context("start capped worker Cargo command")?;
    FileExt::unlock(&admission)?;
    drop(admission);
    #[cfg(unix)]
    let status = wait_for_worker_command(&mut child)?;
    #[cfg(not(unix))]
    let status = child.wait()?;
    #[cfg(unix)]
    drop(worker_signals);
    drop(slot);
    if !status.success() {
        bail!(
            "{}: FAIL {head} ({status})",
            if test.is_some() { "test" } else { "check" }
        );
    }
    if clean_head(&repo)? != head {
        bail!("Worker tree changed during check; no PASS receipt recorded");
    }
    let test_record = match test {
        Some(test) => {
            let count: u64 = std::fs::read_to_string(&count_file)
                .context("missing verified-test count")?
                .trim()
                .parse()?;
            std::fs::remove_file(&count_file)?;
            if count == 0 {
                bail!("Worker test executed zero tests; no PASS receipt recorded");
            }
            Some(TestReceipt {
                filter: test.filter,
                harness: test.harness,
                count,
                host_path_opt_out,
                supervisor_recorded: None,
            })
        }
        None => None,
    };
    let record = CheckReceipt {
        head: head.clone(),
        repo,
        packages,
        test: test_record,
    };
    std::fs::create_dir_all(receipt.parent().context("receipt directory")?)?;
    std::fs::write(&receipt, serde_json::to_vec(&record)?)?;
    if let Some(test) = record.test {
        println!(
            "test: PASS {head} {} {} {}{}",
            record.packages[0], test.filter, test.count, test.path_note()
        );
    } else {
        println!("check: PASS {head} packages={}", record.packages.join(","));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn term_to_worker_check_parent_cleans_cargo_descendants() {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        use std::time::{Duration, Instant};

        const NAME: &str = "factory_worker_check::tests::term_to_worker_check_parent_cleans_cargo_descendants";
        const FIXTURE: &str = "CAS_WORKER_TERM_PARENT_FIXTURE";
        if let Some(root) = std::env::var_os(FIXTURE) {
            let root = PathBuf::from(root);
            let mut command = Command::new("python3");
            command.arg("-c").arg("import pathlib, runpy, sys; guard = runpy.run_path(sys.argv[1], run_name='signal_fixture'); guard['verified'](pathlib.Path(sys.argv[2]).read_text(), sys.argv[3], ['nextest', 'run', '-p', 'cas', '--lib'], pathlib.Path(sys.argv[4]))")
                .arg(root.join("worker-proof-guards.py"))
                .arg(root.join("verified.sh"))
                .arg(root.join("fake-cargo.py"))
                .arg(root.join("count"))
                .env("VERIFIED_TEST_REPO_ROOT", &root)
                .env("CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS", "60")
                .env("PID_FILE", root.join("pids"))
                .env("CARGO", root.join("fake-cargo.py"))
                .env("PYTHONPATH", &root)
                // CI runs the suite under run-verified-tests.sh with a
                // relative VERIFIED_TEST_LOG; the nested runner must not
                // inherit it, or it cannot open the log and never starts Cargo.
                .env_remove("VERIFIED_TEST_LOG")
                .stdout(Stdio::null()).stderr(Stdio::null());
            command.process_group(0);
            let signals = WorkerWaitSignals::install().unwrap();
            let mut child = command.spawn().unwrap();
            let status = wait_for_worker_command(&mut child).unwrap();
            drop(signals);
            assert!(!status.success(), "TERM must stop the worker command");
            return;
        }

        let root = tempfile::tempdir().unwrap();
        let fake = root.path().join("fake-cargo.py");
        std::fs::write(
            root.path().join("worker-proof-guards.py"),
            include_str!("../../scripts/worker-proof-guards.py"),
        )
        .unwrap();
        std::fs::write(root.path().join("verified.sh"), include_str!("../../scripts/run-verified-tests.sh")).unwrap();
        std::fs::write(root.path().join("sitecustomize.py"), r#"import os, signal, time
real_killpg = os.killpg
def slow_group_term(group, signum):
    if signum == signal.SIGTERM:
        time.sleep(1.2)
    return real_killpg(group, signum)
os.killpg = slow_group_term
"#).unwrap();
        std::fs::write(&fake, r#"#!/usr/bin/env python3
import os, pathlib, subprocess, sys
marker = pathlib.Path(os.environ['PID_FILE'])
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)', str(marker)])
ready = marker.with_suffix('.ready')
ready.write_text(f'{os.getpid()} {child.pid}')
ready.replace(marker)
print('    Finished `test` profile in 0.01s', flush=True)
child.wait()
"#).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut parent = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
            .env(FIXTURE, root.path())
            .stdout(Stdio::null()).stderr(Stdio::inherit())
            .process_group(0)
            .spawn().unwrap();
        let marker = root.path().join("pids");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let pids: Vec<i32> = marker.exists()
            .then(|| std::fs::read_to_string(&marker).unwrap())
            .unwrap_or_default().split_whitespace()
            .map(|pid| pid.parse().unwrap()).collect();
        if pids.len() == 2 {
            unsafe { libc::kill(parent.id() as i32, libc::SIGTERM) };
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut status = None;
        while status.is_none() && Instant::now() < deadline {
            status = parent.try_wait().unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        if status.is_none() {
            // The fixture's leader is still unreaped; only its owned group.
            unsafe { libc::killpg(parent.id() as i32, libc::SIGKILL) };
            status = Some(parent.wait().unwrap());
        }
        let survivors: Vec<_> = pids.into_iter().enumerate().filter(|(_, pid)| {
            let output = Command::new("ps").args(["-p", &pid.to_string(), "-o", "stat="])
                .output().unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            !state.trim().is_empty() && !state.trim().starts_with('Z')
        }).collect();
        for (index, pid) in &survivors {
            let output = Command::new("ps").args(["-ww", "-p", &pid.to_string(), "-o", "command="])
                .output().unwrap();
            let command = String::from_utf8_lossy(&output.stdout);
            let owned = if *index == 0 { command.contains(fake.to_str().unwrap()) }
                        else { command.contains(root.path().to_str().unwrap()) && command.contains("time.sleep(60)") };
            if owned {
                unsafe { libc::kill(*pid, libc::SIGKILL) };
            }
        }
        assert!(marker.exists(), "fake Cargo never reached discovery");
        assert_eq!(status.unwrap().code(), Some(0), "parent signal fixture failed");
        assert!(survivors.is_empty(), "Cargo descendants survived: {survivors:?}");
    }

    #[cfg(unix)]
    fn capacity_test_root(base: &Path, cap: usize) -> PathBuf {
        let root = base.join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            format!("[factory]\nmax_concurrent_builders={cap}\n"),
        )
        .unwrap();
        crate::store::open_agent_store(&root).unwrap().init().unwrap();
        crate::store::open_task_store(&root).unwrap().init().unwrap();
        root
    }

    #[cfg(unix)]
    fn register_capacity_worker(
        root: &Path,
        worker_id: &str,
        worker_name: &str,
        task_id: &str,
        priority: cas_types::Priority,
        checkout: &Path,
    ) {
        use crate::store::{open_agent_store, open_task_store};
        use cas_types::{Agent, AgentRole, Task, TaskStatus};

        let agents = open_agent_store(root).unwrap();
        let tasks = open_task_store(root).unwrap();
        let mut agent = Agent::new_with_role(
            worker_id.into(),
            worker_name.into(),
            AgentRole::Worker,
        );
        agent.factory_session = Some(format!("session-{worker_name}"));
        agent
            .metadata
            .insert("clone_path".into(), checkout.display().to_string());
        agents.register(&agent).unwrap();
        let mut task = Task::new(task_id.into(), "wait for a builder slot".into());
        task.status = TaskStatus::InProgress;
        task.assignee = Some(worker_name.into());
        task.priority = priority;
        tasks.add(&task).unwrap();
        agents
            .try_claim(task_id, worker_id, 3600, Some("working"))
            .unwrap();
    }

    /// cas-c53a: on btrfs and overlayfs `/proc/locks` can name a different
    /// device than `stat`; a held FLOCK on the lane file's inode is still busy.
    #[cfg(target_os = "linux")]
    #[test]
    fn lane_lock_on_another_device_number_is_still_busy_cas_c53a() {
        let locks = "1: FLOCK  ADVISORY  WRITE 4242 00:3e:918273 0 EOF\n\
                     2: -> FLOCK  ADVISORY  WRITE 4343 00:2f:555 0 EOF\n";
        assert!(super::flock_held_on_inode(locks, 918_273));
        assert!(!super::flock_held_on_inode(locks, 555), "a waiter does not hold the lock");
    }

    #[cfg(unix)]
    #[test]
    fn worker_check_lane_refusal_reports_live_and_stale_owner() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_BUILD_GUARD",
            "off",
        )]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[factory]\nmax_concurrent_builders=1\n",
        )
        .unwrap();
        let fake = dir.path().join("fake-cargo");
        fake_cargo(&fake, "exit 0");
        let args = ["nextest", "run", "-p", "cas", "--lib", "-E", "test(waiting)"]
            .map(str::to_string);

        let path = lane_lock_path(&root, &repo).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lock = lock_file(&path).unwrap();
        lock.lock_exclusive().unwrap();
        let started = chrono::Utc::now() - chrono::Duration::seconds(210);
        std::fs::write(
            &path,
            serde_json::json!({
                "pid": std::process::id(),
                "command": "cargo nextest run -p cas --lib -E test(waiting)",
                "started_at": started.to_rfc3339(),
            })
            .to_string(),
        )
        .unwrap();

        let live = execute_at(&root, &args, &repo, &fake).unwrap_err().to_string();
        assert!(live.contains(&format!("pid {}", std::process::id())), "{live}");
        assert!(
            live.contains("cargo nextest run -p cas --lib -E test(waiting)"),
            "{live}"
        );
        assert!(live.contains("started 3m ago"), "{live}");

        std::fs::write(
            &path,
            serde_json::json!({
                "pid": u32::MAX,
                "command": "cargo nextest run -p cas --lib -E test(waiting)",
                "started_at": started.to_rfc3339(),
            })
            .to_string(),
        )
        .unwrap();
        let stale = execute_at(&root, &args, &repo, &fake).unwrap_err().to_string();
        assert!(stale.contains("stale"), "{stale}");
        assert!(
            stale.contains("cargo nextest run -p cas --lib -E test(waiting)"),
            "{stale}"
        );
        FileExt::unlock(&lock).unwrap();
        let acquired = try_lock_lane_with_command(
            &root,
            &repo,
            "cargo nextest run -p cas --lib -E test(waiting)",
        )
        .unwrap()
        .unwrap();
        let stale = acquired.stale_holder.as_ref().unwrap();
        assert_eq!(stale.pid, u32::MAX);
        assert!(stale_lane_holder_notice(stale, chrono::Utc::now()).contains("stale previous holder"));
        drop(acquired);
        drop(lock);
    }

    /// cas-3d04: Codex SIGKILLs the calling shell's process group when an
    /// exec_command shell exits, so the detached runner must lead its own
    /// session (and therefore its own process group).
    #[cfg(unix)]
    #[test]
    fn detached_runner_leads_its_own_session_outside_the_callers_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 5"]);
        let mut child = spawn_detached(command).unwrap();
        let pid = child.id() as libc::pid_t;
        // SAFETY: plain process-id queries on our own live child.
        let (sid, pgid, caller_pgid) = unsafe { (libc::getsid(pid), libc::getpgid(pid), libc::getpgrp()) };
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(sid, pid, "detached runner must be a session leader");
        assert_eq!(pgid, pid, "detached runner must lead its own process group");
        assert_ne!(pgid, caller_pgid, "a kill of the caller's group must not reach it");
    }

    /// cas-6591: the detached log ends with the runner's exit status, so a
    /// refused run cannot pass for one still in progress.
    #[cfg(unix)]
    #[test]
    fn detached_runner_log_ends_with_the_runner_exit_code() {
        // `false` stands in for a runner that refuses: it exits 1.
        let args = vec!["-p".to_string(), "cas".into(), "--lib".into()];
        let output = detached_runner_command(Path::new("false"), Path::new("/cas-root"), &args, None)
            .output()
            .unwrap();
        let log = String::from_utf8(output.stdout).unwrap();
        assert_eq!(log, "worker-check: EXIT 1\n");
    }

    #[cfg(unix)]
    #[test]
    fn worker_proofs_include_fixed_guards_before_an_unrelated_filter() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        // Keep the real lease above the wrapper's log FD 3, so that log
        // descriptor cannot accidentally satisfy the inherited-lease check.
        let _reserved_fds: Vec<_> = (0..16).map(|_| std::fs::File::open("/dev/null").unwrap()).collect();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        std::fs::create_dir(repo.join("scripts")).unwrap();
        std::fs::create_dir(repo.join("cas-cli")).unwrap();
        std::fs::write(repo.join("cas-cli/Cargo.toml"), "[package]\nname = \"cas\"\n").unwrap();
        for (name, source) in [
            ("check-test-shape.py", include_str!("../../scripts/check-test-shape.py")),
            ("rust_test_source.py", include_str!("../../scripts/rust_test_source.py")),
        ] {
            std::fs::write(repo.join("scripts").join(name), source).unwrap();
        }
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", "guard fixture"]).unwrap();
        let fake = dir.path().join("cargo");
        fake_cargo(&fake, r#"if [ -d /proc/self/fd ]; then
    [ -e "/proc/self/fd/$CAS_TARGET_LEASE_FD" ] || exit 11
fi
printf '%s\n' "$*" >> target/invocations
printf '     Summary [ 0.01s] 2 tests run: 2 passed, 0 skipped\n'"#);
        let args = ["nextest", "run", "-p", "cas", "--lib", "-E", "test(unrelated)"].map(str::to_string);
        execute_at(&root, &args, &repo, &fake).unwrap();
        let invocations = std::fs::read_to_string(repo.join("target/invocations")).unwrap();
        assert!(invocations.contains("test_support::lib_test_process_env_mutation_is_isolated"), "{invocations}");
        assert!(invocations.contains("serialized_factory_fields_have_registry_entries_cas_f7b9"), "{invocations}");
        assert!(invocations.lines().last().unwrap().ends_with("test(unrelated)"), "{invocations}");
        assert_eq!(invocations.lines().count(), 2);
        assert!(passing_test_receipts(&root, &repo, &fixture_head(&repo))[0].ends_with("test(unrelated) 2"));
        fake_cargo(&fake, r#"case "$*" in
    *test_support::*) printf '     Summary [ 0.01s] 2 tests run: 2 passed, 0 skipped\n' ;;
    *) printf '     Summary [ 0.01s] 0 tests run: 0 passed, 0 skipped\n' ;;
esac"#);
        assert!(execute_at(&root, &args, &repo, &fake).is_err(), "passing guards cannot hide an empty requested selection");
        assert!(passing_test_receipts(&root, &repo, &fixture_head(&repo)).is_empty());
        fake_cargo(&fake, r#"printf '%s\n' "$*" > target/failed-guard
printf '     Summary [ 0.01s] 0 tests run: 0 passed, 0 skipped\n'"#);
        assert!(execute_at(&root, &args, &repo, &fake).is_err(), "guard execution itself must be nonzero");
        // pin: the failing external invocation must be the fixed guard selection.
        assert!(std::fs::read_to_string(repo.join("target/failed-guard")).unwrap().contains("test_support::"));
    }

    #[test]
    fn ancestor_test_receipt_uses_delivered_manifest_after_head_switch() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        std::fs::write(
            repo.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/widget\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(repo.join("crates/widget/assets")).unwrap();
        std::fs::write(
            repo.join("crates/widget/Cargo.toml"),
            "[package]\nname = \"cas\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(repo.join("crates/widget/assets/theme.brief.md"), "old\n").unwrap();
        fixture_commit_as(&repo, "workspace fixture");
        let receipt_head = fixture_head(&repo);
        record_passing_test_for_tests(&root, &repo, &receipt_head, "cas", "test(cached)");

        std::fs::write(repo.join("crates/widget/assets/theme.brief.md"), "new\n").unwrap();
        fixture_commit_as(&repo, "delivered asset");
        let delivered_head = fixture_head(&repo);
        std::fs::remove_file(repo.join("crates/widget/Cargo.toml")).unwrap();
        fixture_commit_as(&repo, "later manifest removal");
        let later_head = fixture_head(&repo);
        git(&repo, &["checkout", "--detach", &later_head]).unwrap();
        assert_ne!(fixture_head(&repo), delivered_head, "the worktree must be on a later HEAD");

        assert!(
            passing_test_receipts(&root, &repo, &delivered_head).is_empty(),
            "the delivered tree still has a crate manifest, so its changed asset invalidates the old test receipt"
        );
    }

    #[test]
    fn ancestor_test_receipt_ignores_same_basename_outside_delivered_include_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        std::fs::create_dir_all(repo.join("crates/widget/src")).unwrap();
        std::fs::write(
            repo.join("crates/widget/Cargo.toml"),
            "[package]\nname = \"cas\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(repo.join("docs/maintenance")).unwrap();
        std::fs::create_dir_all(repo.join("docs/release")).unwrap();
        std::fs::write(
            repo.join("docs/maintenance/update-managed-files.brief.md"),
            "maintenance\n",
        )
        .unwrap();
        std::fs::write(
            repo.join("docs/release/update-managed-files.brief.md"),
            "release v1\n",
        )
        .unwrap();
        std::fs::write(
            repo.join("crates/widget/src/lib.rs"),
            "const MAINTENANCE: &str = include_str!(\"../../../docs/maintenance/update-managed-files.brief.md\");\n",
        )
        .unwrap();
        fixture_commit_as(&repo, "include fixture");
        let receipt_head = fixture_head(&repo);
        record_passing_test_for_tests(&root, &repo, &receipt_head, "cas", "test(cached)");

        std::fs::write(
            repo.join("docs/release/update-managed-files.brief.md"),
            "release v2\n",
        )
        .unwrap();
        fixture_commit_as(&repo, "unreferenced same-name asset");
        let delivered_head = fixture_head(&repo);
        std::fs::write(
            repo.join("docs/maintenance/update-managed-files.brief.md"),
            "maintenance v2\n",
        )
        .unwrap();
        fixture_commit_as(&repo, "referenced asset change");
        let referenced_head = fixture_head(&repo);
        assert!(
            passing_test_receipts(&root, &repo, &referenced_head).is_empty(),
            "a change to the exact included repo-relative path invalidates the old test receipt"
        );
        std::fs::write(
            repo.join("crates/widget/src/lib.rs"),
            "const MAINTENANCE: &str = include_str!(\"../../../docs/maintenance/update-managed-files.brief.md\");\nconst RELEASE: &str = include_str!(\"../../../docs/release/update-managed-files.brief.md\");\n",
        )
        .unwrap();
        fixture_commit_as(&repo, "later include change");
        let later_head = fixture_head(&repo);
        git(&repo, &["checkout", "--detach", &later_head]).unwrap();
        assert_ne!(fixture_head(&repo), delivered_head, "the worktree must be on a later HEAD");

        assert_eq!(
            passing_test_receipts(&root, &repo, &delivered_head),
            [format!(
                "test: PASS {receipt_head} cas test(cached) 1 rebound from {receipt_head} to {delivered_head}"
            )],
            "the delivered source refers to a different repo-relative path with the same basename"
        );
    }

    #[test]
    fn ancestor_test_receipt_git_processes_are_bounded_by_close_cas_27f8() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        std::fs::create_dir_all(repo.join("src")).unwrap();
        for index in 0..12 {
            std::fs::write(
                repo.join(format!("src/source-{index}.rs")),
                "const GUIDE: &str = include_str!(\"guide.data\");\n",
            )
            .unwrap();
        }
        fixture_commit_as(&repo, "Rust source candidates");
        let receipt_head = fixture_head(&repo);
        record_passing_test_for_tests(&root, &repo, &receipt_head, "cas", "test(cached)");

        for index in 0..12 {
            let asset = repo.join(format!("docs/deep/section-{index}/guide.data"));
            std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
            std::fs::write(asset, "guide\n").unwrap();
        }
        fixture_commit_as(&repo, "unreferenced assets");
        let delivered_head = fixture_head(&repo);

        let trace = dir.path().join("git-trace2.jsonl");
        let mut env = crate::test_env_guard::TestEnvGuard::new();
        env.set("GIT_TRACE2_EVENT", &trace);
        let receipts = passing_test_receipts(&root, &repo, &delivered_head);
        drop(env);

        assert_eq!(receipts.len(), 1, "unreferenced files should retain the receipt");
        let starts = std::fs::read_to_string(trace)
            .unwrap()
            .lines()
            .filter(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .is_ok_and(|event| event["event"] == "start")
            })
            .count();
        assert!(
            starts <= 5,
            "one close should use a bounded Git process count for many directories and source candidates; observed {starts}"
        );
    }

    #[test]
    fn ancestor_test_receipt_matches_nested_manifest_relative_include_cas_27f8() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        std::fs::write(
            repo.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/nested/widget\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(repo.join("crates/nested/widget/src")).unwrap();
        std::fs::write(
            repo.join("crates/nested/widget/Cargo.toml"),
            "[package]\nname = \"widget\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(repo.join("docs/assets")).unwrap();
        std::fs::write(repo.join("docs/assets/guide.md"), "old guide\n").unwrap();
        std::fs::write(
            repo.join("crates/nested/widget/src/lib.rs"),
            "const GUIDE: &str = include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../../docs/assets/\", \"guide.md\"));\n",
        )
        .unwrap();
        fixture_commit_as(&repo, "nested include fixture");
        let receipt_head = fixture_head(&repo);
        record_passing_test_for_tests(&root, &repo, &receipt_head, "widget", "test(cached)");

        std::fs::write(repo.join("docs/assets/guide.md"), "updated guide\n").unwrap();
        fixture_commit_as(&repo, "included manifest-relative asset");
        let delivered_head = fixture_head(&repo);

        assert!(
            passing_test_receipts(&root, &repo, &delivered_head).is_empty(),
            "a nested CARGO_MANIFEST_DIR-relative include resolves to its repo-relative asset"
        );
    }

    #[test]
    fn ancestor_test_receipt_runtime_read_keeps_the_old_path_match_floor_cas_27f8() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        std::fs::create_dir_all(repo.join("cas-cli/tests")).unwrap();
        std::fs::write(
            repo.join("cas-cli/Cargo.toml"),
            "[package]\nname = \"cas\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(repo.join(".markdownlint-cli2.jsonc"), "old rules\n").unwrap();
        std::fs::write(
            repo.join("cas-cli/tests/release_report_test.rs"),
            "fn read_rules() { let _ = std::fs::read_to_string(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../.markdownlint-cli2.jsonc\")); }\n",
        )
        .unwrap();
        fixture_commit_as(&repo, "runtime-read fixture");
        let receipt_head = fixture_head(&repo);
        record_passing_test_for_tests(&root, &repo, &receipt_head, "cas", "test(cached)");

        std::fs::write(repo.join(".markdownlint-cli2.jsonc"), "updated rules\n").unwrap();
        fixture_commit_as(&repo, "runtime-read config change");
        let delivered_head = fixture_head(&repo);

        assert!(
            passing_test_receipts(&root, &repo, &delivered_head).is_empty(),
            "a runtime read of a nonstandard extension still invalidates the ancestor test receipt"
        );
    }

    #[test]
    fn ancestor_test_receipt_reads_cat_file_while_writing_large_batches_cas_27f8() {
        use std::process::Stdio;
        use std::time::{Duration, Instant};

        const TEST: &str = "factory_worker_check::tests::ancestor_test_receipt_reads_cat_file_while_writing_large_batches_cas_27f8";
        const REPO: &str = "CAS_27F8_BATCH_REPO";
        const HEAD: &str = "CAS_27F8_BATCH_HEAD";
        if let (Some(repo), Some(head)) = (std::env::var_os(REPO), std::env::var_os(HEAD)) {
            let repo = PathBuf::from(repo);
            let tree = DeliveredTree::load(&repo, head.to_str().unwrap()).unwrap();
            assert_eq!(tree.rust_sources(&repo).unwrap().len(), 3_500);
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fixture_commit(&repo);
        std::fs::create_dir_all(repo.join("src")).unwrap();
        for index in 0..3_500 {
            std::fs::write(
                repo.join(format!("src/source-{index}.rs")),
                "pub const SOURCE: u8 = 1;\n",
            )
            .unwrap();
        }
        fixture_commit_as(&repo, "large source batch");
        let head = fixture_head(&repo);

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(REPO, &repo)
            .env(HEAD, head)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                let output = child.wait_with_output().unwrap();
                assert!(
                    output.status.success(),
                    "large cat-file batch child failed:\n{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(stdout.contains("1 passed; 0 failed"), "{stdout}");
                break;
            }
            if started.elapsed() >= Duration::from_secs(15) {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "cat-file batch blocked before completing 3,500 sources:\n{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(unix)]
    fn path_test_child(name: &str, path: &OsStr, bin: &Path) {
        let test = format!("factory_worker_check::tests::{name}");
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--nocapture"])
            .env("PATH", path)
            .env("CAS_A9FC_PATH_CHILD", bin)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        let stdout = String::from_utf8(child.stdout).unwrap();
        // pin: Rust's external harness must prove one exact selected child passed.
        assert!(
            stdout.contains(&format!("test {test} ... ok")) && stdout.contains("1 passed; 0 failed"),
            "{stdout}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn named_test_path_hides_host_tools_beside_cargo_cas_a9fc() {
        let Some(bin) = std::env::var_os("CAS_A9FC_PATH_CHILD") else {
            let bin = tempfile::tempdir().unwrap();
            let tools = tempfile::tempdir().unwrap();
            fake_cargo(&tools.path().join("cas-a9fc-downstream-tool"), "exit 0");
            for name in ["cas", "codex", "claude"] {
                fake_cargo(&bin.path().join(name), "exit 0");
            }
            // cas-1c4b: a tool sharing a directory with the harness binaries
            // (git beside codex/claude in /opt/homebrew/bin) must stay visible.
            fake_cargo(&bin.path().join("cas-1c4b-beside-harness-tool"), "exit 0");
            let original = std::env::var_os("PATH").unwrap();
            let path = std::env::join_paths(
                [bin.path().to_path_buf(), tools.path().to_path_buf()]
                    .into_iter().chain(std::env::split_paths(&original)),
            )
            .unwrap();
            path_test_child(
                "named_test_path_hides_host_tools_beside_cargo_cas_a9fc",
                &path,
                bin.path(),
            );
            return;
        };
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let bin = PathBuf::from(bin);
        let cargo = bin.join("cargo");
        fake_cargo(
            &cargo,
            r#"for tool in cas codex claude; do
    if command -v "$tool" > /dev/null; then
        printf 'host tool leaked: %s\n' "$tool" >&2
        exit 23
    fi
    done
    command -v rustc > /dev/null || exit 24
    command -v git > /dev/null || exit 25
    command -v cas-a9fc-downstream-tool > /dev/null || exit 26
    command -v cas-1c4b-beside-harness-tool > /dev/null || exit 27
    printf '%s\n' "$PATH" > target/selected-path
    printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'"#,
        );
        let args = [
            "nextest",
            "run",
            "-p",
            "cas",
            "--lib",
            "-E",
            "test(selected)",
        ]
        .map(str::to_string);
        execute_at(&root, &args, &repo, &cargo)
            .expect("selected tests must not see tools beside host cargo");
        let selected_path = std::fs::read_to_string(repo.join("target/selected-path")).unwrap();
        assert!(
            !selected_path.contains(bin.to_str().unwrap()),
            "{selected_path}"
        );
        assert!(
            passing_test_receipts(&root, &repo, &fixture_head(&repo))[0].ends_with("test(selected) 1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn real_rustup_cargo_cannot_restore_host_harness_path_cas_a9fc() {
        let Some(bin) = std::env::var_os("CAS_A9FC_PATH_CHILD") else {
            let _env = crate::test_support::TestEnvGuard::new();
            let original = std::env::var_os("PATH").unwrap_or_default();
            let Some(rustup) = std::env::split_paths(&original)
                .map(|dir| dir.join("rustup")).find(|path| executable(path)) else {
                eprintln!("SKIP real rustup control: rustup is unavailable");
                return;
            };
            let available = Command::new(&rustup).args(["which", "cargo"]).output().unwrap();
            if !available.status.success() {
                eprintln!("SKIP real rustup control: no active toolchain");
                return;
            }
            let home = tempfile::tempdir().unwrap();
            let bin = home.path().join("bin");
            std::fs::create_dir(&bin).unwrap();
            for name in ["cargo", "rustc", "rustdoc", "rustup"] {
                std::os::unix::fs::symlink(&rustup, bin.join(name)).unwrap();
            }
            for name in ["cas", "codex", "claude"] {
                fake_cargo(&bin.join(name), "exit 0");
            }
            fake_cargo(&bin.join("cargo-nextest"), r#"for tool in cas codex claude; do
    if command -v "$tool" > /dev/null; then
        printf 'rustup restored host tool: %s\n' "$tool" >&2
        exit 23
    fi
done
printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'"#);
            let path = std::env::join_paths(
                std::iter::once(bin.clone()).chain(std::env::split_paths(&original)),
            ).unwrap();
            path_test_child("real_rustup_cargo_cannot_restore_host_harness_path_cas_a9fc", &path, &bin);
            return;
        };
        let bin = PathBuf::from(bin);
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CARGO_HOME", bin.parent().unwrap());
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let args = ["nextest", "run", "-p", "cas", "--lib", "-E", "test(selected)"].map(str::to_string);
        execute_at(&root, &args, &repo, &bin.join("cargo"))
            .expect("the real rustup proxy must not restore host harness binaries");
        assert!(!passing_test_receipts(&root, &repo, &fixture_head(&repo)).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn host_path_opt_out_is_reasoned_and_receipted_cas_a9fc() {
        let Some(bin) = std::env::var_os("CAS_A9FC_PATH_CHILD") else {
            let cas = crate::test_support::fake_cas_on_path();
            path_test_child(
                "host_path_opt_out_is_reasoned_and_receipted_cas_a9fc",
                cas.path(),
                cas.bin(),
            );
            return;
        };
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let cargo = dir.path().join("cargo");
        fake_cargo(
            &cargo,
            r#"command -v cas > target/selected-cas || exit 23
    printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'"#,
        );
        let args = [
            "nextest",
            "run",
            "-p",
            "cas",
            "--lib",
            "-E",
            "test(selected)",
        ]
        .map(str::to_string);
        env.set(
            HOST_PATH_OPT_OUT,
            "exercise operator-installed harness integration",
        );
        execute_at(&root, &args, &repo, &cargo).unwrap();
        let receipts = passing_test_receipts(&root, &repo, &fixture_head(&repo));
        assert!(
            receipts[0]
                .contains("[host PATH opt-out: exercise operator-installed harness integration]")
        );
        let selected = std::fs::read_to_string(repo.join("target/selected-cas")).unwrap();
        assert_eq!(Path::new(selected.trim()), PathBuf::from(bin).join("cas"));
        env.set(HOST_PATH_OPT_OUT, " ");
        assert!(
            execute_at(&root, &args, &repo, &cargo)
                .unwrap_err()
                .to_string()
                .contains("nonempty, single-line reason")
        );
        assert!(passing_test_receipts(&root, &repo, &fixture_head(&repo)).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn live_headful_profile_requires_reason_and_preserves_credentials_cas_7e12() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        env.set("CAS_HEADFUL_E2E", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]\nname='cas'\nversion='0.1.0'\n[[test]]\nname='e2e_test'\npath='tests/e2e.rs'\n").unwrap();
        std::fs::write(repo.join("tests/e2e.rs"), "").unwrap();
        fixture_commit_as(&repo, "explicit e2e harness");
        let cargo = dir.path().join("cargo");
        fake_cargo(&cargo, "printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\\n'");
        let args = ["nextest", "run", "-p", "cas", "--test", "e2e_test", "-E",
            "test(=e2e::factory_tui_headful::factory_tui_headful_reuse_window)"].map(str::to_string);
        let error = execute_at(&root, &args, &repo, &cargo).unwrap_err().to_string();
        assert!(error.contains("CAS_WORKER_TEST_ALLOW_HOST_PATH"), "{error}");
        env.set(HOST_PATH_OPT_OUT, "operator live headful credential proof");
        let xdg = dir.path().join("private-config");
        std::fs::create_dir(&xdg).unwrap();
        let error = execute_at_with_scope(&root, &args, &repo, &cargo, None, Some(&xdg)).unwrap_err().to_string();
        assert!(error.contains("credential-free"), "{error}");
        let lib_args = ["nextest", "run", "-p", "cas", "--lib", "-E", "test(selected)"].map(str::to_string);
        let error = execute_at(&root, &lib_args, &repo, &cargo).unwrap_err().to_string();
        assert!(error.contains("exact headful"), "{error}");
        if !cfg!(target_os = "macos") { return; }
        env.set("CLAUDE_CONFIG_DIR", "/fixture/selected-account");
        env.set("CLAUDE_CODE_OAUTH_TOKEN", "fixture-token");
        env.set("XDG_CONFIG_HOME", &xdg);
        fake_cargo(&cargo, r#"[ "$CAS_HEADFUL_E2E" = 1 ] || exit 21
[ "$CLAUDE_CONFIG_DIR" = /fixture/selected-account ] || exit 22
[ "$CLAUDE_CODE_OAUTH_TOKEN" = fixture-token ] || exit 23
[ -d "$XDG_CONFIG_HOME" ] || exit 24
[ "$CARGO_TARGET_DIR" = "$PWD/target" ] || exit 25
case "$*" in *'--success-output immediate'*) ;; *) exit 26 ;; esac
printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'"#);
        execute_at(&root, &args, &repo, &cargo).unwrap();
        assert!(passing_test_receipts(&root, &repo, &fixture_head(&repo))[0].contains("operator live headful credential proof"));
    }

    #[cfg(unix)]
    #[test]
    fn private_fake_cas_fixture_preserves_process_path_cas_a9fc() {
        let _env = crate::test_support::TestEnvGuard::new();
        let original = std::env::var_os("PATH");
        let cas = crate::test_support::fake_cas_on_path();
        let found = Command::new("sh")
            .env("PATH", cas.path())
            .args(["-c", "command -v cas; cas"])
            .output()
            .unwrap();
        assert!(found.status.success());
        assert_eq!(
            String::from_utf8(found.stdout).unwrap().trim(),
            cas.bin().join("cas").to_str().unwrap()
        );
        assert_eq!(std::env::var_os("PATH"), original);
    }

    #[test]
    fn worker_proof_diff_uses_integration_target_after_worker_push() {
        let _env = crate::test_support::TestEnvGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let base = fixture_head(&repo);
        git(&repo, &["update-ref", "refs/remotes/origin/epic/v42", &base]).unwrap();
        std::fs::write(repo.join("delivery.rs"), "fn changed() {}\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", "delivery"]).unwrap();
        git(&repo, &["update-ref", "refs/remotes/origin/factory/worker", "HEAD"]).unwrap();
        assert_eq!(proof_diff_base(&root, &repo, Some("epic/v42")).unwrap(), base);
    }

    #[cfg(unix)]
    #[test]
    fn worker_memory_scope_runs_cargo_with_limit_and_preserves_context() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker with spaces");
        fixture_commit(&repo);
        std::fs::write(root.join("config.toml"), "[factory]\nworker_memory_high_gib = 3\n").unwrap();
        let scope = dir.path().join("scope runner");
        fake_scope(&scope, r#"printf '%s\n' "$@" > target/scope-args
while [ "$1" != -- ]; do shift; done
shift
exec "$@""#);
        let cargo = dir.path().join("cargo stand-in");
        fake_cargo(&cargo, r#"[ "$CARGO_TARGET_DIR" = "$PWD/target" ] || exit 4
[ -z "$RUSTC_WRAPPER" ] || exit 5
if [ -d /proc/self/fd ]; then
    [ -e "/proc/self/fd/$CAS_TARGET_LEASE_FD" ] || exit 6
fi
printf '%s\n' "$@" > target/cargo-args"#);
        execute_at_with_scope(&root, &["-p".into(), "cas".into(), "--lib".into()], &repo, &cargo, Some(&scope), None).unwrap();
        let scope_args = std::fs::read_to_string(repo.join("target/scope-args")).expect("build must enter a memory-limited scope");
        assert!(scope_args.contains("--user\n--scope\n"), "{scope_args}");
        assert!(scope_args.contains("MemoryHigh=3221225472\n"), "{scope_args}");
        // pin: the external build receives exactly the admitted Cargo argv.
        assert_eq!(std::fs::read_to_string(repo.join("target/cargo-args")).unwrap(), "check\n-p\ncas\n--lib\n");
        assert!(passing_receipt(&root, &repo, &fixture_head(&repo)).is_some());
    }

    /// cas-3f93: with `[factory] scratch_root` set the capped check builds
    /// into the worker's relocated private target, under its lifetime lease,
    /// and never creates in-tree build output.
    #[cfg(unix)]
    #[test]
    fn scratch_root_worker_check_builds_into_relocated_target() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let scratch = dir.path().join("rewind");
        std::fs::write(
            root.join("config.toml"),
            format!("[factory]\nscratch_root = {:?}\n", scratch.display().to_string()),
        )
        .unwrap();
        let expected = scratch
            .join(cas_types::factory_scratch::project_key(&root))
            .join("targets/worker");
        let cargo = dir.path().join("cargo");
        fake_cargo(&cargo, &format!(
            "[ \"$CARGO_TARGET_DIR\" = {:?} ] || exit 4\n[ -f \"$CARGO_TARGET_DIR/{}\" ] || exit 5\necho run >> \"$CARGO_TARGET_DIR/runs\"",
            expected.display().to_string(),
            crate::factory_target_cache::owner::MARKER,
        ));
        execute_at(&root, &["-p".into(), "cas".into(), "--lib".into()], &repo, &cargo).unwrap();
        assert_eq!(std::fs::read_to_string(expected.join("runs")).unwrap(), "run\n");
        assert!(!repo.join("target/debug").exists());
        assert!(passing_receipt(&root, &repo, &fixture_head(&repo)).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn worker_memory_scope_falls_back_without_a_user_session_or_systemd() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let rejected = dir.path().join("no-user-session");
        fake_cargo(&rejected, "exit 1");
        let missing = dir.path().join("missing-systemd-run");
        let cargo = dir.path().join("cargo");
        fake_cargo(&cargo, "echo run >> target/runs");
        for (name, runner) in [("no-session", Some(rejected.as_path())), ("missing", Some(missing.as_path())), ("other-platform", None)] {
            let repo = root.join("worktrees").join(name);
            fixture_commit(&repo);
            execute_at_with_scope(&root, &["-p".into(), "cas".into(), "--lib".into()], &repo, &cargo, runner, None).unwrap();
            // pin: unavailable scopes must execute the external build once.
            assert_eq!(std::fs::read_to_string(repo.join("target/runs")).unwrap().lines().count(), 1, "fallback must execute Cargo once: {name}");
            assert!(passing_receipt(&root, &repo, &fixture_head(&repo)).is_some(), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn worker_memory_scope_does_not_retry_a_failed_build_unscoped() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let scope = dir.path().join("scope");
        fake_scope(&scope, "while [ \"$1\" != -- ]; do shift; done; shift; exec \"$@\"");
        let cargo = dir.path().join("cargo");
        fake_cargo(&cargo, "echo run >> target/runs\nexit 7");
        let error = execute_at_with_scope(&root, &["-p".into(), "cas".into(), "--lib".into()], &repo, &cargo, Some(&scope), None).unwrap_err();
        assert!(error.to_string().contains("FAIL"), "{error}");
        assert_eq!(std::fs::read_to_string(repo.join("target/runs")).unwrap().lines().count(), 1);
        assert!(passing_receipt(&root, &repo, &fixture_head(&repo)).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_memory_scope_keeps_targeted_test_receipts_and_lease() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let _reserved_fds: Vec<_> = (0..16).map(|_| std::fs::File::open("/dev/null").unwrap()).collect();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let scope = dir.path().join("scope");
        fake_scope(&scope, "while [ \"$1\" != -- ]; do shift; done; shift; exec \"$@\"");
        let cargo = dir.path().join("cargo");
        fake_cargo(&cargo, r#"[ "$1" = nextest ] || exit 3
[ -e "/proc/self/fd/$CAS_TARGET_LEASE_FD" ] || exit 4
[ -n "$CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER$CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER" ] || exit 5
printf '     Summary [ 0.01s] 2 tests run: 2 passed, 0 skipped\n'"#);
        let args = ["nextest", "run", "-p", "cas", "-E", "test(worker)"].map(str::to_string);
        execute_at_with_scope(&root, &args, &repo, &cargo, Some(&scope), None).unwrap();
        let receipts = passing_test_receipts(&root, &repo, &fixture_head(&repo));
        assert_eq!(receipts.len(), 1);
        assert!(receipts[0].ends_with("test(worker) 2"), "{receipts:?}");
        assert!(crate::factory_target_cache::owner::for_retirement(&root, &repo).unwrap().is_some());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn capped_check_then_test_excludes_compiler_cache_and_keeps_target_lease_cas_4b15() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        // A Cargo stand-in observes the effective wrapper settings and the
        // real inherited lock. It can invoke a cache stand-in, but must not.
        let cache = dir.path().join("cache with spaces/sccache");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fake_cargo(&cache, "touch target/cache-was-invoked");
        env.set("CARGO_BUILD_RUSTC_WRAPPER", &cache);
        env.set("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", &cache);
        let fake = dir.path().join("fake-cargo");
        fake_cargo(&fake, r#"python3 - <<'PY'
import json, os, subprocess
from pathlib import Path
repo = Path.cwd()
record, = [json.loads(p.read_text()) for p in (repo.parent.parent / 'worker-target-owners').glob('*.json')
           if json.loads(p.read_text())['worktree'] == str(repo)]
assert record['active'], 'Cargo must run under target ownership'
fds = []
for fd in os.listdir('/proc/self/fd'):
    try:
        stat = os.fstat(int(fd))
        fds.append((stat.st_dev, stat.st_ino))
    except OSError:
        pass
assert (record['lease_dev'], record['lease_ino']) in fds, 'real builders must still inherit the lease'
for key in ('RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER'):
    wrapper = os.environ.get(key, os.environ.get('CARGO_BUILD_' + key, ''))
    if wrapper:
        subprocess.run([wrapper], check=True)
(repo / 'target/observed').write_text('owned')
PY
if [ "$1" = nextest ]; then
    printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'
fi"#);
        for direct in [true, false] {
            let repo = root.join(format!("worktrees/worker-{direct}"));
            fixture_commit(&repo);
            for key in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"] {
                if direct { env.set(key, &cache); } else { env.remove(key); }
            }
            execute_at(&root, &["-p".into(), "cas".into(), "--tests".into()], &repo, &fake).unwrap();
            let test_args = ["nextest", "run", "-p", "cas", "--lib", "-E", "test(worker)"].map(str::to_string);
            execute_at(&root, &test_args, &repo, &fake).unwrap();
            assert!(!repo.join("target/cache-was-invoked").exists(), "capped Cargo must not invoke a compiler cache (direct={direct})");
            assert_eq!(std::fs::read_to_string(repo.join("target/observed")).unwrap(), "owned");
            assert_eq!(passing_test_receipts(&root, &repo, &fixture_head(&repo)).len(), 1);
            assert!(crate::factory_target_cache::owner::for_retirement(&root, &repo).unwrap().is_some());
            assert_eq!(std::env::var_os("CARGO_BUILD_RUSTC_WRAPPER").unwrap(), cache.as_os_str(), "parent cache configuration is unchanged");
        }
    }

    /// cas-4ad0: through the real capped runner, a test binary started by
    /// nextest runs through the release runner. It does not hold the target
    /// lease, so a child it leaks cannot lock the worktree out. A plain
    /// `cargo check` sets no runner.
    #[cfg(target_os = "linux")]
    #[test]
    fn leaked_test_child_does_not_lock_the_worktree_out_cas_4ad0() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let variable = crate::factory_target_cache::owner::host_runner_env().unwrap();
        // cas-edeb: nested checks can inherit a capped outer test's runner.
        // Check must clear it; test mode must install its own release runner.
        env.set(&variable, "/inherited/outer-test-runner");
        let fake = dir.path().join("fake-cargo");
        fake_cargo(
            &fake,
            &format!(
                r#"if [ "$1" != nextest ]; then
    [ -z "${{{variable}:-}}" ] || {{ echo 'check must not set a test runner' >&2; exit 3; }}
    exit 0
fi
# The "test binary": runs through the runner, must not see the lease, leaks a child.
"${{{variable}}}" python3 - <<'PY'
import json, os, subprocess
from pathlib import Path
repo = Path.cwd()
record, = [json.loads(p.read_text()) for p in (repo.parent.parent / 'worker-target-owners').glob('*.json')
           if json.loads(p.read_text())['worktree'] == str(repo)]
held = []
for fd in os.listdir('/proc/self/fd'):
    try:
        stat = os.fstat(int(fd))
    except OSError:
        continue
    if (stat.st_dev, stat.st_ino) == (record['lease_dev'], record['lease_ino']):
        held.append(fd)
assert not held, f'test binary inherited the lease on fd {{held}}'
leak = subprocess.Popen(['sleep', '30'], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL, start_new_session=True)
(repo / 'target/leaked-pid').write_text(str(leak.pid))
PY
printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped
'"#
            ),
        );
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        execute_at(&root, &["-p".into(), "cas".into(), "--lib".into()], &repo, &fake).unwrap();
        let test_args =
            ["nextest", "run", "-p", "cas", "--lib", "-E", "test(worker)"].map(str::to_string);
        execute_at(&root, &test_args, &repo, &fake).unwrap();
        let leaked: i32 = std::fs::read_to_string(repo.join("target/leaked-pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: signal 0 only probes the leaked child this test caused.
        let alive = unsafe { libc::kill(leaked, 0) } == 0;
        let free = crate::factory_target_cache::owner::for_retirement(&root, &repo);
        // SAFETY: kill only the leaked child this test caused.
        unsafe { libc::kill(leaked, libc::SIGKILL) };
        assert!(alive, "the leaked child outlives the run");
        assert!(
            free.is_ok_and(|lease| lease.is_some()),
            "a leaked test child must not hold the worktree's target lease"
        );
    }

    /// cas-edeb: managed review checkouts have no target lease, so nested
    /// tests must not inherit the outer host's test-binary runner.
    #[cfg(unix)]
    #[test]
    fn managed_review_test_does_not_inherit_host_runner_cas_edeb() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_BUILD_GUARD", "off");
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let variable = crate::factory_target_cache::owner::host_runner_env().unwrap();
        env.set(&variable, "/inherited/outer-test-runner");

        let dir = tempfile::tempdir().unwrap();
        // Match production admission's canonical paths, including macOS's
        // /var -> /private/var temporary-directory alias.
        let repo = dir.path().canonicalize().unwrap().join("project");
        fixture_commit(&repo);
        let head = fixture_head(&repo);
        let root = repo.join(".cas");
        crate::store::init_cas_dir(&repo).unwrap();
        crate::store::open_task_store(&root)
            .unwrap()
            .add(&crate::types::Task::new("cas-proof".into(), "proof".into()))
            .unwrap();
        let checkout =
            crate::factory_review_scratch::create(&root, &repo, "cas-proof", None, &head).unwrap();
        let alias = dir.path().join("cas-alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        for cas_root in [&root, &alias] {
            assert!(
                crate::factory_target_cache::owner::acquire(cas_root, &checkout.path)
                    .unwrap()
                    .is_none(),
                "managed review checkouts do not own worker target leases: {}",
                cas_root.display()
            );
        }

        let fake = dir.path().join("fake-cargo");
        fake_cargo(
            &fake,
            &format!(
                r#"[ "$1" = nextest ] || exit 2
[ -z "${{{variable}:-}}" ] || {{ echo 'test command inherited host runner' >&2; exit 3; }}
python3 -c 'import os, sys; sys.exit(0 if not os.environ.get("{variable}") else 4)'
printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'"#
            ),
        );
        let args = ["nextest", "run", "-p", "cas", "--lib", "-E", "test(worker)"]
            .map(str::to_string);
        execute_at_with_scope(&root, &args, &checkout.path, &fake, None, None).unwrap();
    }

    /// cas-f616: a lane proof's `--tests` step continues its `--lib` PASS at
    /// the same commit; anything else is an independent check and stays gated.
    #[test]
    fn continuation_carries_load_admission_only_for_the_same_fresh_pass_cas_f616() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("preview");
        let receipt = dir.path().join("head.json");
        let packages = vec!["cas".to_string()];
        let write = |record: &CheckReceipt| std::fs::write(&receipt, serde_json::to_vec(record).unwrap()).unwrap();
        let pass = |head: &str, packages: &[String], test: Option<TestReceipt>| CheckReceipt {
            head: head.into(),
            repo: repo.clone(),
            packages: packages.to_vec(),
            test,
        };
        let now = std::time::SystemTime::now();

        assert!(!continuation_admits_load(&receipt, &repo, "abc", &packages, now), "no earlier PASS");
        write(&pass("abc", &packages, None));
        assert!(continuation_admits_load(&receipt, &repo, "abc", &packages, now));
        assert!(!continuation_admits_load(&receipt, &repo, "def", &packages, now), "other commit");
        assert!(
            !continuation_admits_load(&receipt, &repo, "abc", &["cas".into(), "cas-pty".into()], now),
            "other packages"
        );
        assert!(
            !continuation_admits_load(&receipt, &dir.path().join("other"), "abc", &packages, now),
            "other worktree"
        );
        let later = now + CONTINUATION_WINDOW + std::time::Duration::from_secs(60);
        assert!(!continuation_admits_load(&receipt, &repo, "abc", &packages, later), "stale PASS");
        write(&pass("abc", &packages, Some(TestReceipt { filter: "test(x)".into(), harness: None, count: 1, host_path_opt_out: None, supervisor_recorded: None })));
        assert!(!continuation_admits_load(&receipt, &repo, "abc", &packages, now), "a test receipt");
    }

    /// cas-f616: a load reading above the cap between the proof's steps does
    /// not refuse the continuation, but the builder cap still does, and an
    /// independent check is still refused on load.
    #[test]
    fn admitted_continuation_waives_only_the_load_reading_cas_f616() {
        let snapshot = crate::factory_build_guard::BuildGuardSnapshot {
            cpu_count: 32,
            load_1m: Some(41.5),
            live_cargo_workers: 0,
            requested_workers: 1,
            requested_builders: 1,
            expected_build: "yes",
            max_concurrent_builders: 2,
            disabled: false,
            pressure: Default::default(),
            limits: Default::default(),
            disk: None,
        };
        assert_eq!(snapshot.admission_violations(false).len(), 1, "independent checks stay load-gated");
        assert!(snapshot.violations()[0].contains("exceeds 32 CPUs"));
        assert!(snapshot.admission_violations(true).is_empty(), "the continuation proceeds");
        let crowded = crate::factory_build_guard::BuildGuardSnapshot { live_cargo_workers: 2, ..snapshot };
        let violations = crowded.admission_violations(true);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("max_concurrent_builders"), "{violations:?}");
    }

    #[cfg(unix)]
    #[test]
    fn cas_c0ec_documented_redirect_passes_clean_gate_without_compilation() {
        use cas_core::hooks::types::HookInput;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        std::fs::create_dir_all(repo.join("target")).unwrap();
        git(&repo, &["init", "-q"]).unwrap();
        std::fs::write(repo.join(".gitignore"), "/target/\n").unwrap();
        std::fs::write(repo.join("source.rs"), "// committed fixture\n").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CAS_FACTORY_BUILD_GUARD", "off"),
            ("CAS_HOOK_HARNESS", "claude"),
            ("CAS_CLONE_PATH", repo.to_str().unwrap()),
        ]);
        let request = HookInput {
            session_id: "worker-log-fixture".into(),
            cwd: repo.to_string_lossy().into_owned(),
            hook_event_name: "PreToolUse".into(),
            tool_name: Some("Bash".into()),
            tool_input: Some(
                serde_json::json!({"command": "cargo check -p cas --tests > target/worker-check.log 2>&1 &"}),
            ),
            agent_role: Some("worker".into()),
            ..Default::default()
        };
        let prior_refusal = "Waiting for build admission: max_concurrent_builders=2 slots are occupied.\n";
        std::fs::write(repo.join("target/worker-check.log"), prior_refusal).unwrap();
        let output = crate::hooks::handlers::handle_pre_tool_use(&request, Some(&root)).unwrap();
        let value = serde_json::to_value(output).unwrap();
        let rewritten = value
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|value| value.as_str())
            .unwrap();
        assert!(rewritten.contains("factory worker-check"));
        // cas-3d04: backgrounded runs detach instead of keeping the shell `&`.
        assert!(rewritten.contains("--detach --"), "{rewritten}");
        assert!(rewritten.ends_with(">> target/worker-check.log 2>&1"), "{rewritten}");
        let head = fixture_head(&repo);
        // Execute the admitted shell suffix before invoking the real runner
        // boundary. The Cargo stand-in exits zero and compiles nothing.
        let suffix = rewritten.split_once('>').unwrap().1;
        assert!(
            Command::new("sh")
                .args(["-c", &format!(": >{suffix}")])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
        // pin: a retry must preserve the complete prior refusal receipt byte-for-byte.
        assert_eq!(std::fs::read_to_string(repo.join("target/worker-check.log")).unwrap(), prior_refusal,
            "a retry's admitted shell redirect must preserve the earlier refusal receipt");
        assert_eq!(clean_head(&repo).unwrap(), head);
        let fake = dir.path().join("fake-cargo");
        std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let args = vec!["-p".into(), "cas".into(), "--tests".into()];
        execute_at(&root, &args, &repo, &fake).unwrap();
        assert!(passing_receipt(&root, &repo, &head).is_some());
        for path in ["untracked.rs", "source.rs"] {
            std::fs::write(repo.join(path), "// dirt\n").unwrap();
            assert!(
                execute_at(&root, &args, &repo, &fake)
                    .unwrap_err()
                    .to_string()
                    .contains("Commit the worker change")
            );
            if path == "untracked.rs" {
                std::fs::remove_file(repo.join(path)).unwrap();
            }
        }
    }

    #[test]
    fn only_package_scoped_lib_or_tests_checks_are_accepted() {
        let args = |text: &str| {
            text.split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        for target in ["--lib", "--tests"] {
            assert_eq!(
                check_packages(&args(&format!("-p cas -p cas-pty {target}"))).unwrap(),
                ["cas", "cas-pty"]
            );
            assert!(check_packages(&args(&format!("{target} -p cas"))).is_some());
        }
        for invalid in [
            "--lib",
            "--tests",
            "-p cas",
            "-p '*' --tests",
            "-p cas --tests --lib",
            "-p cas --lib --tests",
            "-p cas --lib --lib",
            "-p cas --lib --all-targets",
            "-p cas --lib --workspace",
            "-p cas --lib --config x=y",
            "-p cas --tests --workspace",
            "-p cas --tests --config x=y",
            "-p cas --tests --tests",
            "-p --tests",
            "-p ../cas --tests",
        ] {
            assert!(check_packages(&args(invalid)).is_none(), "{invalid}");
        }
    }

    #[test]
    fn targeted_nextest_allow_deny_matrix() {
        let words = |text: &str| {
            text.split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        for accepted in [
            "nextest run -p cas -E test(hooks::handlers)",
            "nextest run -p cas --lib -E test(=module::name)",
            "nextest run -p cas --test integration_factory -E test(worker)",
            "nextest run -p crossterm@0.25.0 --lib -E test(take_buffered)",
            "nextest run -p crate-name@1.0.0-rc.1+build.5 --test harness -E test(worker)",
        ] {
            assert!(allowed_args(&words(accepted)), "{accepted}");
        }
        let mut union = words("nextest run -p cas --lib -E");
        union.push("test(one) | test(two) & test(three)".into());
        assert!(allowed_args(&union));
        for refused in [
            "nextest run -p cas",
            "nextest run -p cas -E",
            "nextest run -p cas -E all()",
            "nextest run -p cas -E test()",
            "nextest run -p cas -E test(*)",
            "nextest run -p cas -E test(/.*/)",
            "nextest run -p cas -E !test(name)",
            "nextest run -p cas -E package(cas)",
            "nextest run -p cas -p cas --lib -E test(name)",
            "nextest run -p cas -p cas-pty --lib -E test(name)",
            "nextest run --workspace -E test(name)",
            "nextest run -p cas --release -E test(name)",
            "nextest run -p cas --tests -E test(name)",
            "nextest run -p cas --lib --test integration_factory -E test(name)",
            "nextest run -p cas --test one --test two -E test(name)",
            "nextest run -p cas --no-fail-fast",
            "nextest run -p cas --no-fail-fast -E test(name)",
            "nextest run -p cas -E test(name) --workspace",
            "test -p cas --lib name",
            "build -p cas",
            "nextest run -p '*' -E test(name)",
        ] {
            assert!(!allowed_args(&words(refused)), "{refused}");
        }
        for filter in [
            "",
            " ",
            "test(one) | all()",
            "test(one) & !test(two)",
            "test(one); echo bad",
            "test(one)\n",
        ] {
            let mut args = words("nextest run -p cas -E");
            args.push(filter.into());
            assert!(!allowed_args(&args), "{filter}");
        }
        for package in [
            "@0.25.0", "crossterm@", "crossterm@0.25", "crossterm@01.0.0",
            "crossterm@0.25.0@0.29.0", "crossterm@*", "crossterm@^0.25.0",
            "crossterm@0.25.0-", "crossterm@0.25.0+", "crossterm@0.25.0-01",
            "crossterm@0.25.0 other", "crossterm@0.25.0\n", "crossterm@0.25.0\0",
            "crossterm@0.25.0;echo", "crossterm@0.25.0$(echo)",
            "https://example.test/#crossterm@0.25.0",
        ] {
            let mut args = words("nextest run -p cas --lib -E test(name)");
            args[3] = package.into();
            assert!(!allowed_args(&args), "{package:?}");
        }
    }

    #[test]
    fn harnesses_use_explicit_manifest_inventory() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(
            repo.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"package\"]\n[workspace.package]\nversion = \"0.25.0\"\n",
        )
        .unwrap();
        let package = repo.path().join("package");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("Cargo.toml"), "[package]\nname = \"cas\"\n[[test]]\nname = \"integration_factory\"\npath = \"harness.rs\"\n").unwrap();
        std::fs::write(package.join("harness.rs"), "// fixture").unwrap();
        let mut test = TargetedTest {
            package: "cas".into(),
            harness: Some("integration_factory".into()),
            filter: "test(worker)".into(),
        };
        validate_harness(repo.path(), &test).unwrap();
        for version in ["version = \"0.25.0\"", "version.workspace = true"] {
            std::fs::write(package.join("Cargo.toml"), format!(
                "[package]\nname = \"cas\"\n{version}\n[[test]]\nname = \"integration_factory\"\npath = \"harness.rs\"\n"
            )).unwrap();
            test.package = "cas@0.25.0".into();
            validate_harness(repo.path(), &test).unwrap();
            test.package = "cas@0.29.0".into();
            assert!(validate_harness(repo.path(), &test).is_err());
        }
        test.package = "cas".into();
        test.harness = Some("source_suite_alias".into());
        assert!(validate_harness(repo.path(), &test).is_err());
        test.harness = Some("integration_factory".into());
        std::fs::remove_file(package.join("harness.rs")).unwrap();
        assert!(validate_harness(repo.path(), &test).is_err());
    }

    #[cfg(unix)]
    /// cas-84b3: the fixture's HEAD, measured without the runner's own
    /// `clean_head`. Expectations built with `clean_head` agreed with any
    /// receipt it wrote, even one naming a short or wrong commit.
    fn fixture_head(repo: &Path) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "git rev-parse HEAD failed");
        let head = String::from_utf8(out.stdout).unwrap().trim().to_string();
        assert!(
            head.len() == 40 && head.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "fixture HEAD must be a full commit id: {head}"
        );
        head
    }

    fn fixture_commit(repo: &Path) {
        std::fs::create_dir_all(repo).unwrap();
        git(repo, &["init", "-q"]).unwrap();
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        git(repo, &["add", "."]).unwrap();
        git(
            repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
    }

    fn fixture_commit_as(repo: &Path, message: &str) {
        git(repo, &["add", "-A"]).unwrap();
        git(
            repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                message,
            ],
        )
        .unwrap();
    }

    #[cfg(unix)]
    fn fake_scope(path: &Path, body: &str) {
        fake_cargo(path, &format!(
            "if [ \"$1\" = --version ]; then printf 'systemd 259\\n'; exit 0; fi\n{body}"
        ));
    }

    #[cfg(unix)]
    fn fake_cargo(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn runner_leaves_zig_discovery_to_the_build_script() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fixture_commit(&source);
        std::fs::write(source.join(".gitignore"), ".cas/\n.context/\ntarget/\n").unwrap();
        let consumer = source.join("crates/ghostty_vt_sys/build.rs");
        std::fs::create_dir_all(consumer.parent().unwrap()).unwrap();
        std::fs::write(consumer, "// Zig consumer").unwrap();
        git(&source, &["add", "."]).unwrap();
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "consumer",
            ],
        )
        .unwrap();
        let root = source.join(".cas");
        let repo = root.join("worktrees/preview");
        std::fs::create_dir_all(repo.parent().unwrap()).unwrap();
        git(
            &source,
            &[
                "worktree",
                "add",
                "--detach",
                repo.to_str().unwrap(),
                "HEAD",
            ],
        )
        .unwrap();
        assert!(!repo.join(".context/zig/zig").exists());
        // Preserve the tools needed by Git and the zero-test guard, including
        // Python for nextest discovery; keep Zig absent from this PATH.
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let host_path = std::env::var_os("PATH").unwrap();
        for tool in ["git", "bash", "python3", "rm", "mktemp", "tee", "sed", "grep", "awk"] {
            let path = std::env::split_paths(&host_path)
                .map(|dir| dir.join(tool))
                .find(|path| executable(path))
                .unwrap();
            std::os::unix::fs::symlink(path, bin.join(tool)).unwrap();
        }
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CAS_FACTORY_BUILD_GUARD", "off"),
            ("ZIG", ""),
            ("PATH", bin.to_str().unwrap()),
        ]);
        let fake = dir.path().join("fake-cargo");
        fake_cargo(
            &fake,
            r#"[ -z "${ZIG:-}" ] || exit 9
printf 'called\n' >> "$PWD/target/called"
printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'"#,
        );
        std::fs::create_dir(repo.join("target")).unwrap();
        let args = vec!["-p".into(), "cas".into(), "--lib".into()];
        execute_at(&root, &args, &repo, &fake).unwrap();
        let test_args = vec![
            "nextest".into(),
            "run".into(),
            "-p".into(),
            "cas".into(),
            "--lib".into(),
            "-E".into(),
            "test(worker)".into(),
        ];
        execute_at(&root, &test_args, &repo, &fake).unwrap();
        let head = fixture_head(&repo);
        assert!(passing_receipt(&root, &repo, &head).is_some());
        assert_eq!(
            std::fs::read_to_string(repo.join("target/called")).unwrap(),
            "called\ncalled\n"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_runner_records_ownership_before_fake_build_and_inherits_lease_cas_f96d() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CAS_FACTORY_BUILD_GUARD", "off"), ("CAS_FACTORY_DISABLE_TARGET_SEED", "1"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let fake = dir.path().join("fake-cargo");
        fake_cargo(&fake, r#"python3 - <<'PY'
import json, os
from pathlib import Path
repo = Path.cwd()
records = [json.loads(p.read_text()) for p in (repo.parent.parent / 'worker-target-owners').glob('*.json')]
record, = [r for r in records if r['worktree'] == str(repo)]
assert record['active'] and record['start'] > 0
assert [p.name for p in (repo / 'target').iterdir()] == ['.cas-worker-target-owner'], 'provenance must precede data'
inherited = []
for fd in os.listdir('/proc/self/fd'):
    try:
        stat = os.fstat(int(fd))
        inherited.append((stat.st_dev, stat.st_ino))
    except OSError:
        pass
assert (record['lease_dev'], record['lease_ino']) in inherited, 'output lease must reach real command'
(repo / 'target' / 'build-observed').write_text('lease protected')
PY"#);
        execute_at(&root, &["-p".into(), "cas".into(), "--lib".into()], &repo, &fake).unwrap();
        assert_eq!(std::fs::read_to_string(repo.join("target/build-observed")).unwrap(), "lease protected");
        assert!(crate::factory_target_cache::owner::for_retirement(&root, &repo).unwrap().is_some());
        assert!(passing_receipt(&root, &repo, &fixture_head(&repo)).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn runner_discards_legacy_shared_fingerprints_cas_a7cf() {
        use std::os::unix::fs::MetadataExt;

        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let snapshot = dir.path().join("snapshot");
        std::fs::create_dir(&snapshot).unwrap();
        let source = snapshot.join("invoked.timestamp");
        std::fs::write(&source, b"another lane's freshness").unwrap();
        for profile in ["debug", "aarch64-apple-darwin/debug"] {
            let shared = repo
                .join("target")
                .join(profile)
                .join(".fingerprint/cas-types-abc");
            std::fs::create_dir_all(&shared).unwrap();
            std::fs::hard_link(&source, shared.join("invoked.timestamp")).unwrap();
            // Even records Cargo already replaced privately are suspect when
            // this unit still has a shared invocation timestamp.
            std::fs::write(shared.join("lib-cas_types"), b"possibly poisoned").unwrap();
        }
        let private = repo.join("target/debug/.fingerprint/warm-abc/lib-warm");
        std::fs::create_dir_all(private.parent().unwrap()).unwrap();
        std::fs::write(&private, b"private freshness").unwrap();
        let artifact = repo.join("target/debug/deps/libwarm.rmeta");
        std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        std::fs::hard_link(&source, &artifact).unwrap();
        let fake = dir.path().join("fake-cargo");
        fake_cargo(
            &fake,
            r#"
[ ! -e target/debug/.fingerprint/cas-types-abc ] || exit 3
[ ! -e target/aarch64-apple-darwin/debug/.fingerprint/cas-types-abc ] || exit 4
[ -f target/debug/.fingerprint/warm-abc/lib-warm ] || exit 5
[ -f target/debug/deps/libwarm.rmeta ] || exit 6
"#,
        );
        let args = vec!["-p".into(), "cas".into(), "--lib".into()];
        execute_at(&root, &args, &repo, &fake).unwrap();
        execute_at(&root, &args, &repo, &fake).unwrap();
        assert_eq!(std::fs::read(&private).unwrap(), b"private freshness");
        assert_eq!(std::fs::read(&source).unwrap(), b"another lane's freshness");
        assert_eq!(
            std::fs::metadata(&artifact).unwrap().ino(),
            std::fs::metadata(&source).unwrap().ino()
        );
        assert!(passing_receipt(&root, &repo, &fixture_head(&repo)).is_some());
    }

    /// Capacity is waiting for local resources, not a yielded task.
    #[cfg(unix)]
    #[tokio::test]
    async fn capacity_refusals_keep_lease_and_escalate_once_cas_dd28() {
        use crate::mcp::{CasCore, CasService};
        use crate::store::{open_agent_store, open_supervisor_queue_store, open_task_store};
        use cas_types::{Agent, AgentRole, Task, TaskStatus};
        use rmcp::handler::server::wrapper::Parameters;
        use rmcp::model::RawContent;

        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_BUILD_GUARD", "off");
        env.set("CAS_FACTORY_MODE", "1");
        env.set("CAS_FACTORY_SESSION", "capacity-fixture");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        env.set("CAS_ROOT", &root);
        std::fs::write(root.join("config.toml"), "[factory]\nmax_concurrent_builders=1\n").unwrap();
        let agents = open_agent_store(&root).unwrap();
        agents.init().unwrap();
        for (id, name, role) in [
            ("capacity-worker", "worker", AgentRole::Worker),
            ("capacity-supervisor", "capacity-lead", AgentRole::Supervisor),
        ] {
            let mut agent = Agent::new_with_role(id.into(), name.into(), role);
            agent.factory_session = Some("capacity-fixture".into());
            agents.register(&agent).unwrap();
        }
        let tasks = open_task_store(&root).unwrap();
        tasks.init().unwrap();
        let mut task = Task::new("cas-dd28".into(), "wait for build capacity".into());
        task.status = TaskStatus::InProgress;
        task.assignee = Some("worker".into());
        tasks.add(&task).unwrap();
        agents.try_claim(&task.id, "capacity-worker", 3600, Some("working")).unwrap();
        let lease = agents.get_lease(&task.id).unwrap().unwrap();
        let notices = open_supervisor_queue_store(&root).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        core.set_agent_id_for_testing("capacity-supervisor".into());
        let service = CasService::new(core, None);
        let fake = dir.path().join("fake-cargo");
        fake_cargo(&fake, "exit 0");
        let args = ["-p", "cas", "--lib"].map(str::to_string);
        let held = acquire_slot(&root.join("worker-check-slots"), 1).unwrap();
        for _ in 0..3 {
            let refused = execute_at(&root, &args, &repo, &fake).unwrap_err().to_string();
            assert!(refused.contains("Waiting for build admission"), "{refused}");
            assert_eq!(tasks.get(&task.id).unwrap().status, TaskStatus::InProgress);
            assert_eq!(agents.get_lease(&task.id).unwrap().unwrap().epoch, lease.epoch);
            assert_eq!(notices.pending_count("capacity-supervisor").unwrap(), 0);
        }
        let result = service.factory(Parameters(serde_json::from_value(serde_json::json!({
            "action": "worker_status", "summary_mode": true,
        })).unwrap())).await.unwrap();
        let status = result.content.into_iter().filter_map(|content| match content.raw {
            RawContent::Text(text) => Some(text.text), _ => None,
        }).collect::<Vec<_>>().join("\n");
        // pin: capacity wait and next retry identify resource waiting to workers and supervisors.
        assert!(status.to_ascii_lowercase().contains("capacity wait"), "{status}");
        assert!(status.contains("next retry"), "{status}");
        assert!(status.contains("max_concurrent_builders=1"), "{status}");
        assert_eq!(status.matches("Capacity wait:").count(), 1);
        let wait = crate::factory_capacity_wait::view(&root, "capacity-worker", chrono::Utc::now()).unwrap().unwrap();
        let retry_at = wait.since + chrono::Duration::minutes(2);
        let repeated = crate::factory_capacity_wait::record_refusal(&root, &repo,
            "memory PSI some avg10 25% exceeds 10%", retry_at).unwrap().unwrap();
        assert_eq!(repeated.since, wait.since, "retry cannot reset the escalation deadline");
        assert_eq!(repeated.next_retry, retry_at + chrono::Duration::seconds(120));
        for (minutes, expected_pages) in [(19, 0), (20, 1), (21, 1), (30, 1)] {
            // The daemon uses this same reconciliation entry with wall time;
            // passing time here needs neither a sleep nor a fake store.
            crate::factory_capacity_wait::reconcile(&root, Some("capacity-fixture"),
                wait.since + chrono::Duration::minutes(minutes)).unwrap();
            assert_eq!(notices.pending_count("capacity-supervisor").unwrap(), expected_pages);
            assert_eq!(tasks.get(&task.id).unwrap().status, TaskStatus::InProgress);
            assert_eq!(agents.get_lease(&task.id).unwrap().unwrap().epoch, lease.epoch);
        }
        // The actual deadline row must reach a quiet supervisor, not merely
        // exist in the outbox. Use its stored origin and the daemon's real
        // pane-wake policy; queue acceptance alone cannot prove AC3.
        let rows = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(100).unwrap();
        let wake_rows: Vec<_> = rows.iter().filter(|row| row.source.starts_with("supervisor-notice:")).collect();
        assert_eq!(wake_rows.len(), 1, "one episode queues one supervisor wake row");
        let row = wake_rows[0];
        use crate::ui::factory::daemon::{FactoryDaemon, runtime::queue_and_events::{PaneWakeState, ToolCallEvidence}};
        let data = cas_factory::DirectorData::load_fast(&root).unwrap();
        let sender = FactoryDaemon::wake_sender_from_origin(row.origin.as_ref(), None);
        let wake = FactoryDaemon::supervisor_wake_decision(
            &data, "capacity-lead", "capacity-lead", &sender, &row.source, &row.prompt,
            PaneWakeState { composer_dirty: false, ready_for_injection: true,
                silent_for: Some(std::time::Duration::from_secs(45)), tool_call: ToolCallEvidence::Idle },
            wait.since + chrono::Duration::minutes(30));
        assert!(wake.allowed, "deadline notice must wake a quiet supervisor: {}", wake.reason);
        drop(held);
        execute_at(&root, &args, &repo, &fake).unwrap();
        assert!(crate::factory_capacity_wait::view(&root, "capacity-worker", chrono::Utc::now()).unwrap().is_none(), "admission clears the episode");
        assert_eq!(tasks.get(&task.id).unwrap().status, TaskStatus::InProgress);
        assert_eq!(agents.get_lease(&task.id).unwrap().unwrap().epoch, lease.epoch);

        // Explicit need uses the existing durable worker message immediately,
        // and consumes the same episode's later automatic escalation.
        let _held = acquire_slot(&root.join("worker-check-slots"), 1).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        core.set_agent_id_for_testing("capacity-worker".into());
        let worker = CasService::new(core, None);
        let mut other = Agent::new_with_role(
            "other-supervisor".into(), "other-lead".into(), AgentRole::Supervisor,
        );
        other.factory_session = Some("other-session".into());
        agents.register(&other).unwrap();
        let mut last_need = None;
        for (target, consumes_notice) in [
            ("supervisor", true), ("other-lead", false), ("capacity-lead", true),
        ] {
            crate::factory_capacity_wait::admitted(&root, &repo).unwrap();
            assert!(execute_at(&root, &args, &repo, &fake).is_err());
            let mut explicit =
                crate::factory_capacity_wait::view(&root, "capacity-worker", chrono::Utc::now())
                    .unwrap()
                    .unwrap();
            // A stale admission reservation still belongs to the live episode.
            explicit.since -= chrono::Duration::minutes(6);
            explicit.next_retry = explicit.since + chrono::Duration::seconds(120);
            let explicit_path = std::fs::read_dir(root.join("capacity-waits"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.extension().is_some_and(|ext| ext == "json"))
                .unwrap();
            std::fs::write(explicit_path, serde_json::to_vec(&explicit).unwrap()).unwrap();
            assert!(
                crate::factory_capacity_wait::view(&root, "capacity-worker", chrono::Utc::now())
                    .unwrap()
                    .is_none(),
                "gap exceeds admission freshness"
            );
            let need = format!("Need storage assistance to repair the scratch volume ({target})");
            let delivered = worker
                .coordination(Parameters(
                    serde_json::from_value(serde_json::json!({
                        "action": "message", "target": target, "task_id": task.id,
                        "blocker": true, "summary": "Storage repair needed", "message": need,
                    }))
                    .unwrap(),
                ))
                .await
                .unwrap();
            assert_ne!(delivered.is_error, Some(true), "{delivered:?}");
            let resolved_target = if target == "supervisor" { "capacity-lead" } else { target };
            assert_eq!(
                crate::store::open_prompt_queue_store(&root)
                    .unwrap()
                    .peek_all(100)
                    .unwrap()
                    .iter()
                    .filter(|prompt| prompt.target == resolved_target && prompt.prompt.contains(&need))
                    .count(),
                1,
                "exactly one explicit assistance page is durably queued"
            );
            let after_need = notices.pending_count("capacity-supervisor").unwrap();
            crate::factory_capacity_wait::reconcile(
                &root,
                Some("capacity-fixture"),
                explicit.since + chrono::Duration::minutes(25),
            )
            .unwrap();
            assert_eq!(
                notices.pending_count("capacity-supervisor").unwrap(),
                after_need + usize::from(!consumes_notice),
                "a blocker sent to {target} only consumes its owner's automatic notice"
            );
            last_need = Some((
                explicit.since, notices.pending_count("capacity-supervisor").unwrap(),
            ));
        }
        let (explicit_since, after_need) = last_need.unwrap();

        // A replacement lease cannot inherit an earlier wait or its deadline.
        assert!(agents.release_lease_if_owner_epoch(&task.id, "capacity-worker", lease.epoch, "new ownership generation").unwrap());
        agents.try_claim(&task.id, "capacity-worker", 3600, Some("resumed")).unwrap();
        assert!(crate::factory_capacity_wait::view(&root, "capacity-worker", chrono::Utc::now()).unwrap().is_none());
        crate::factory_capacity_wait::reconcile(&root, Some("capacity-fixture"), explicit_since + chrono::Duration::minutes(30)).unwrap();
        assert_eq!(notices.pending_count("capacity-supervisor").unwrap(), after_need);
    }

    #[cfg(unix)]
    #[test]
    fn higher_priority_capacity_waiter_gets_next_slot_and_lower_priority_progresses() {
        use crate::store::{open_agent_store, open_task_store};
        use cas_types::{Agent, AgentRole, Priority, Task, TaskStatus};

        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_BUILD_GUARD", "off");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[factory]\nmax_concurrent_builders=1\n",
        )
        .unwrap();
        let agents = open_agent_store(&root).unwrap();
        agents.init().unwrap();
        let tasks = open_task_store(&root).unwrap();
        tasks.init().unwrap();

        let mut repos = Vec::new();
        for (worker_id, worker_name, task_id, priority) in [
            (
                "priority-p2-worker",
                "priority-p2",
                "cas-priority-p2",
                Priority::MEDIUM,
            ),
            (
                "priority-p0-worker",
                "priority-p0",
                "cas-priority-p0",
                Priority::CRITICAL,
            ),
        ] {
            let mut agent = Agent::new_with_role(
                worker_id.into(),
                worker_name.into(),
                AgentRole::Worker,
            );
            agent.factory_session = Some(format!("session-{worker_name}"));
            agents.register(&agent).unwrap();
            let mut task = Task::new(task_id.into(), "wait for a builder slot".into());
            task.status = TaskStatus::InProgress;
            task.assignee = Some(worker_name.into());
            task.priority = priority;
            tasks.add(&task).unwrap();
            agents
                .try_claim(task_id, worker_id, 3600, Some("working"))
                .unwrap();
            let repo = root.join("worktrees").join(worker_name);
            fixture_commit(&repo);
            repos.push(repo);
        }

        let fake = dir.path().join("fake-cargo");
        fake_cargo(&fake, "exit 0");
        let args = ["-p", "cas", "--lib"].map(str::to_string);
        let held = acquire_slot(&root.join("worker-check-slots"), 1).unwrap();
        let p2_wait = execute_at(&root, &args, &repos[0], &fake)
            .unwrap_err()
            .to_string();
        let p0_wait = execute_at(&root, &args, &repos[1], &fake)
            .unwrap_err()
            .to_string();
        drop(held);

        let p2_retried = execute_at(&root, &args, &repos[0], &fake)
            .err()
            .map(|error| error.to_string());
        let p0_admitted_first = execute_at(&root, &args, &repos[1], &fake).is_ok();
        let p2_admitted_after = execute_at(&root, &args, &repos[0], &fake).is_ok();

        // A long-waiting P2 ages into P0 and breaks ties by its earlier wait
        // time, so a stream of newly queued P0 work cannot starve it forever.
        let held = acquire_slot(&root.join("worker-check-slots"), 1).unwrap();
        execute_at(&root, &args, &repos[0], &fake).unwrap_err();
        execute_at(&root, &args, &repos[1], &fake).unwrap_err();
        let p2_wait_path = std::fs::read_dir(root.join("capacity-waits"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                std::fs::read(path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .and_then(|wait| wait["worker_id"].as_str().map(str::to_owned))
                    .as_deref()
                    == Some("priority-p2-worker")
            })
            .unwrap();
        let mut p2_wait_record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&p2_wait_path).unwrap()).unwrap();
        p2_wait_record["since"] = serde_json::json!(
            (chrono::Utc::now() - chrono::Duration::minutes(45)).to_rfc3339()
        );
        p2_wait_record["next_retry"] = serde_json::json!(
            (chrono::Utc::now() - chrono::Duration::minutes(43)).to_rfc3339()
        );
        std::fs::write(&p2_wait_path, serde_json::to_vec(&p2_wait_record).unwrap()).unwrap();
        assert!(crate::factory_capacity_wait::view(&root, "priority-p2-worker", chrono::Utc::now()).unwrap().is_none());
        let aged = crate::factory_capacity_wait::record_refusal(
            &root, &repos[0], "retry after a gap longer than four minutes", chrono::Utc::now(),
        ).unwrap().unwrap();
        assert_eq!(aged.since, serde_json::from_value::<chrono::DateTime<chrono::Utc>>(p2_wait_record["since"].clone()).unwrap(), "retry preserves aging across the gap");
        assert!(aged.cause.starts_with("priority=P0"), "{}", aged.cause);
        drop(held);

        let p0_aged_p2_block = execute_at(&root, &args, &repos[1], &fake)
            .err()
            .map(|error| error.to_string());
        let p2_aged_admitted = execute_at(&root, &args, &repos[0], &fake).is_ok();
        let p0_admitted_after_aged_p2 = execute_at(&root, &args, &repos[1], &fake).is_ok();

        let mut failures = Vec::new();
        if !p2_wait.contains("priority=P2") {
            failures.push(format!("P2 refusal omitted its priority: {p2_wait}"));
        }
        if !p0_wait.contains("priority=P0") {
            failures.push(format!("P0 refusal omitted its priority: {p0_wait}"));
        }
        if !p2_retried
            .as_deref()
            .is_some_and(|message| message.contains("queued behind priority=P0"))
        {
            failures.push(format!("P2 retry was not queued behind P0: {p2_retried:?}"));
        }
        if !p0_admitted_first || !p2_admitted_after {
            failures.push("P0 did not go first while P2 later progressed".to_string());
        }
        if !p0_aged_p2_block
            .as_deref()
            .is_some_and(|message| message.contains("queued behind priority=P0"))
        {
            failures.push(format!("aged P2 did not precede fresh P0: {p0_aged_p2_block:?}"));
        }
        if !p2_aged_admitted || !p0_admitted_after_aged_p2 {
            failures.push("aged P2 or the following P0 failed to make progress".to_string());
        }
        assert!(
            failures.is_empty(),
            "priority admission failed: {}",
            failures.join("; ")
        );
    }

    #[cfg(unix)]
    #[test]
    fn one_higher_priority_waiter_does_not_reserve_multiple_free_slots() {
        use cas_types::Priority;

        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_BUILD_GUARD",
            "off",
        )]);
        let dir = tempfile::tempdir().unwrap();
        let root = capacity_test_root(dir.path(), 3);
        let lower_repo = root.join("worktrees/priority-lower");
        let higher_repo = root.join("worktrees/priority-higher");
        fixture_commit(&lower_repo);
        fixture_commit(&higher_repo);
        register_capacity_worker(
            &root,
            "lower-worker",
            "priority-lower",
            "cas-lower",
            Priority::MEDIUM,
            &lower_repo,
        );
        register_capacity_worker(
            &root,
            "higher-worker",
            "priority-higher",
            "cas-higher",
            Priority::CRITICAL,
            &higher_repo,
        );

        let fake = dir.path().join("fake-cargo");
        fake_cargo(&fake, "exit 0");
        let args = ["-p", "cas", "--lib"].map(str::to_string);
        let slots = root.join("worker-check-slots");
        let occupied = (0..3).map(|_| acquire_slot(&slots, 3).unwrap()).collect::<Vec<_>>();
        assert!(execute_at(&root, &args, &higher_repo, &fake).is_err());
        drop(occupied);

        // One occupied slot leaves two available; one higher-ranked waiter
        // consumes one of them, so the lower-ranked build can use the other.
        let occupied = acquire_slot(&slots, 3).unwrap();
        let lower_result = execute_at(&root, &args, &lower_repo, &fake);
        drop(occupied);
        assert!(
            lower_result.is_ok(),
            "one free slot remains beyond the higher-priority waiter's demand: {lower_result:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn expired_capacity_wait_does_not_block_lower_priority_build() {
        use cas_types::Priority;

        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_BUILD_GUARD",
            "off",
        )]);
        let dir = tempfile::tempdir().unwrap();
        let root = capacity_test_root(dir.path(), 1);
        let lower_repo = root.join("worktrees/priority-lower");
        let higher_repo = root.join("worktrees/priority-higher");
        fixture_commit(&lower_repo);
        fixture_commit(&higher_repo);
        register_capacity_worker(
            &root,
            "lower-worker",
            "priority-lower",
            "cas-lower",
            Priority::MEDIUM,
            &lower_repo,
        );
        register_capacity_worker(
            &root,
            "higher-worker",
            "priority-higher",
            "cas-higher",
            Priority::CRITICAL,
            &higher_repo,
        );

        let fake = dir.path().join("fake-cargo");
        fake_cargo(&fake, "exit 0");
        let args = ["-p", "cas", "--lib"].map(str::to_string);
        let held = acquire_slot(&root.join("worker-check-slots"), 1).unwrap();
        assert!(execute_at(&root, &args, &higher_repo, &fake).is_err());
        drop(held);

        let wait_path = std::fs::read_dir(root.join("capacity-waits"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                std::fs::read(path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .and_then(|wait| wait["worker_id"].as_str().map(str::to_owned))
                    .as_deref()
                    == Some("higher-worker")
            })
            .unwrap();
        let expired = chrono::Utc::now() - chrono::Duration::minutes(5);
        let mut wait: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&wait_path).unwrap()).unwrap();
        wait["next_retry"] = serde_json::json!(expired.to_rfc3339());
        std::fs::write(&wait_path, serde_json::to_vec(&wait).unwrap()).unwrap();

        assert!(
            execute_at(&root, &args, &lower_repo, &fake).is_ok(),
            "an expired higher-priority wait must not refuse the lower build"
        );
        assert!(
            crate::factory_capacity_wait::view(&root, "higher-worker", chrono::Utc::now())
                .unwrap()
                .is_none(),
            "an admission-expired wait is no longer shown as queued"
        );
        crate::factory_capacity_wait::reconcile(
            &root,
            Some("session-priority-higher"),
            chrono::Utc::now(),
        )
        .unwrap();
        assert!(
            wait_path.exists(),
            "reconciliation must retain escalation history for a live task lease"
        );
    }

    #[cfg(unix)]
    #[test]
    fn capacity_binding_skips_missing_task_lease_cas_2083() {
        use crate::store::{open_agent_store, open_task_store};
        use cas_types::{Priority, TaskStatus};

        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = capacity_test_root(dir.path(), 1);
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        register_capacity_worker(
            &root,
            "worker-id",
            "worker",
            "cas-live",
            Priority::MEDIUM,
            &repo,
        );
        let agents = open_agent_store(&root).unwrap();
        agents
            .try_claim("cas-missing", "worker-id", 3600, Some("orphaned lease"))
            .unwrap();
        let wait = crate::factory_capacity_wait::record_refusal(
            &root,
            &repo,
            "fixture capacity refusal",
            chrono::Utc::now(),
        )
        .expect("missing task leases are skipped")
        .unwrap();
        assert_eq!(wait.task_id, "cas-live");
        assert!(
            agents.get_lease("cas-missing").unwrap().is_some(),
            "scanning does not mutate leases"
        );
        assert_eq!(
            open_task_store(&root)
                .unwrap()
                .get("cas-live")
                .unwrap()
                .status,
            TaskStatus::InProgress
        );
    }

    #[cfg(unix)]
    #[test]
    fn unresolved_capacity_wait_binding_does_not_abort_build() {
        use cas_types::Priority;

        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let root = capacity_test_root(dir.path(), 1);
        let shared_repo = root.join("worktrees/shared");
        fixture_commit(&shared_repo);
        register_capacity_worker(
            &root,
            "worker-one",
            "worker-one",
            "cas-one",
            Priority::MEDIUM,
            &shared_repo,
        );
        register_capacity_worker(
            &root,
            "worker-two",
            "worker-two",
            "cas-two",
            Priority::MEDIUM,
            &shared_repo,
        );

        let fake = dir.path().join("fake-cargo");
        fake_cargo(&fake, "printf called >> \"$PWD/target/cargo-ran\"\nexit 0");
        std::fs::create_dir_all(shared_repo.join("target")).unwrap();
        let args = ["-p", "cas", "--lib"].map(str::to_string);

        let result = execute_at(&root, &args, &shared_repo, &fake);
        assert!(
            result.is_ok(),
            "ambiguous capacity-wait binding should warn and continue: {result:?}"
        );
        assert!(
            shared_repo.join("target/cargo-ran").exists(),
            "Cargo still runs when task ownership cannot be resolved"
        );
        std::fs::remove_file(shared_repo.join("target/cargo-ran")).unwrap();
        std::fs::create_dir_all(root.join("capacity-waits")).unwrap();
        std::fs::write(root.join("capacity-waits/broken.json"), b"{not json").unwrap();
        let result = execute_at(&root, &args, &shared_repo, &fake);
        assert!(
            result.is_ok(),
            "episode-clear failure must not abort an admitted build: {result:?}"
        );
        assert!(
            shared_repo.join("target/cargo-ran").exists(),
            "admitted Cargo still runs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn targeted_runner_guards_counts_dirty_trees_and_failed_retries() {
        let mut env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        fixture_commit(&repo);
        let fake = dir.path().join("fake-cargo");
        fake_cargo(
            &fake,
            r#"[ "$CARGO_TARGET_DIR" = "$PWD/target" ] || exit 3
[ "$1 $2 $3 $4 $5 $6" = 'nextest run -p cas --lib -E' ] || exit 4
printf '     Summary [ 0.01s] 2 tests run: 2 passed, 0 skipped\n'"#,
        );
        let args = vec![
            "nextest".into(),
            "run".into(),
            "-p".into(),
            "cas".into(),
            "-E".into(),
            "test(worker)".into(),
        ];
        let head = fixture_head(&repo);
        // Real hook/CLI transport plus the normal capped runner, after the
        // runtime has already inherited provider credentials.
        let xdg = dir.path().join("private-xdg");
        std::fs::create_dir(&xdg).unwrap();
        for key in ISOLATED_TEST_CREDENTIALS { env.set(key, "fixture-provider-sentinel"); }
        env.set("XDG_CONFIG_HOME", dir.path().join("inherited-xdg"));
        env.set("CAS_SESSION_ID", "worker-env-fixture");
        env.set("CAS_CLONE_PATH", &repo);
        env.set("CAS_HOOK_HARNESS", "claude");
        let request = cas_core::hooks::types::HookInput {
            cwd: repo.display().to_string(),
            hook_event_name: "PreToolUse".into(),
            tool_name: Some("Bash".into()), agent_role: Some("worker".into()),
            tool_input: Some(serde_json::json!({"command": format!(
                "env -u VIKTOR_API_KEY -u VIKTOR_KEY -u NEON_API_KEY -u NEON_TOKEN -u VERCEL_TOKEN -u VERCEL_API_TOKEN XDG_CONFIG_HOME={} cargo nextest run -p cas -E 'test(worker)' > target/tests.log 2>&1 &", xdg.display())})),
            ..Default::default()
        };
        let output = crate::hooks::handlers::handle_pre_tool_use(&request, Some(&root)).unwrap();
        let value = serde_json::to_value(output).unwrap();
        let rewritten = value.pointer("/hookSpecificOutput/updatedInput/command").and_then(|v| v.as_str()).expect("isolated request is rewritten");
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(shell_words::split(rewritten.split_once(" >>").unwrap().0).unwrap()).unwrap();
        let Some(crate::cli::Commands::Factory(factory)) = cli.command else { panic!("factory command") };
        let Some(crate::cli::factory::FactoryCommands::WorkerCheck { cas_root, cargo_args, isolated_test_env, detach }) = factory.command else { panic!("worker-check command") };
        assert!(detach);
        assert_eq!(cas_root, root);
        assert_eq!(cargo_args, args);
        assert_eq!(isolated_test_env.as_deref(), Some(xdg.as_path()));
        let detached = detached_runner_command(Path::new("cas"), &root, &cargo_args, isolated_test_env.as_deref());
        let detached_args = detached.get_args().collect::<Vec<_>>();
        assert!(detached_args.windows(2).any(|pair| pair == [std::ffi::OsStr::new("--isolated-test-env"), xdg.as_os_str()]));
        fake_cargo(&fake, &format!(r#"set -e
python3 - <<'PYENV'
import os
from pathlib import Path
for key in ['VIKTOR_API_KEY', 'VIKTOR_KEY', 'NEON_API_KEY', 'NEON_TOKEN', 'VERCEL_TOKEN', 'VERCEL_API_TOKEN']:
    assert key not in os.environ, key + ' leaked into test child'
assert os.environ.get('XDG_CONFIG_HOME') == {xdg:?}, 'private config directory lost'
assert os.environ.get('CAS_SESSION_ID') == 'worker-env-fixture', 'runtime identity lost'
assert os.environ.get('CARGO_TARGET_DIR') == str(Path.cwd() / 'target'), 'private target lost'
assert os.environ.get('CAS_WORKER_NEXTEST_DISCOVERY_TIMEOUT_SECS') == '120', 'discovery bound lost'
(Path.cwd() / 'target' / 'environment-observed').write_text('credential-free private config')
PYENV
printf '     Summary [ 0.01s] 2 tests run: 2 passed, 0 skipped\n'
"#, xdg=xdg.to_str().unwrap()));
        let scope = dir.path().join("scope");
        fake_scope(&scope, "while [ \"$1\" != -- ]; do shift; done; shift; exec \"$@\"");
        for scope in [None, Some(scope.as_path())] {
            execute_at_with_scope(&root, &cargo_args, &repo, &fake, scope, isolated_test_env.as_deref()).unwrap();
        }
        assert!(repo.join("target/environment-observed").is_file(), "environment-checking child must execute");
        for key in ISOLATED_TEST_CREDENTIALS { assert!(std::env::var_os(key).is_some(), "parent {key} changed"); }
        assert_eq!(std::env::var_os("XDG_CONFIG_HOME"), Some(dir.path().join("inherited-xdg").into_os_string()));
        fake_cargo(&fake, "printf '     Summary [ 0.01s] 2 tests run: 2 passed, 0 skipped\\n'");
        assert_eq!(
            passing_test_receipts(&root, &repo, &head),
            [format!("test: PASS {head} cas test(worker) 2")]
        );
        assert!(passing_receipt(&root, &repo, &head).is_none());
        assert!(passing_test_receipts(&root, &repo, &"b".repeat(40)).is_empty());
        assert!(passing_test_receipts(&root, root.as_path(), &head).is_empty());
        let mut other = args.clone();
        other[5] = "test(other)".into();
        execute_at(&root, &other, &repo, &fake).unwrap();
        assert_eq!(passing_test_receipts(&root, &repo, &head).len(), 2);
        // cas-b38a: the close gate reads the tested packages, once each.
        assert_eq!(passing_test_packages(&root, &repo, &head), ["cas"]);
        assert!(passing_test_packages(&root, &repo, &"b".repeat(40)).is_empty());
        fake_cargo(
            &fake,
            "printf '     Summary [ 0.01s] 0 tests run: 0 passed, 2 skipped\\n'",
        );
        assert!(execute_at(&root, &args, &repo, &fake).is_err());
        assert_eq!(
            passing_test_receipts(&root, &repo, &head),
            [format!("test: PASS {head} cas test(other) 2")]
        );
        fake_cargo(&fake, "printf 'no summary\\n'");
        assert!(execute_at(&root, &args, &repo, &fake).is_err());
        fake_cargo(
            &fake,
            "printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\\n'\nexit 12",
        );
        assert!(execute_at(&root, &args, &repo, &fake).is_err());
        std::fs::write(repo.join("dirty.rs"), "// dirty").unwrap();
        assert!(
            execute_at(&root, &args, &repo, &fake)
                .unwrap_err()
                .to_string()
                .contains("Commit the worker change")
        );
        assert!(execute_at(&root, &args, dir.path(), &fake).is_err());
        std::fs::remove_file(repo.join("dirty.rs")).unwrap();
        fake_cargo(
            &fake,
            "printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\\n'\nprintf dirty > changed.rs",
        );
        assert!(execute_at(&root, &args, &repo, &fake).is_err());
        assert_eq!(passing_test_receipts(&root, &repo, &head).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_processes_respect_cap_and_audit_slot_lifetime() {
        use std::io::{BufRead, Write};
        use std::process::Stdio;
        const CHILD: &str = "CAS_WORKER_CAP_PROCESS_FIXTURE";
        const NAME: &str = "factory_worker_check::tests::concurrent_processes_respect_cap_and_audit_slot_lifetime";
        if let Some(input) = std::env::var_os(CHILD) {
            println!(); // Separate machine-readable lines from libtest's test-name prefix.
            let paths: Vec<PathBuf> = serde_json::from_str(&input.to_string_lossy()).unwrap();
            let args = ["nextest", "run", "-p", "cas", "--lib", "-E", "test(worker)"]
                .map(str::to_string);
            match execute_at(&paths[0], &args, &paths[1], &paths[2]) {
                Ok(()) => println!("RUNNER_RESULT admitted"),
                Err(error) => println!("RUNNER_RESULT refused: {error:#}"),
            }
            return;
        }
        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.toml"), "[factory]\nmax_concurrent_builders = 2\n").unwrap();
        let fake = dir.path().join("fake-cargo");
        // A per-payload FIFO holds admitted slots until all N+1 processes
        // answer. Cargo's verified wrapper deliberately gives it null stdin.
        fake_cargo(&fake, r#"printf 'PAYLOAD_STARTED\n'
IFS= read -r release < target/release.fifo
printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'"#);
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut children = Vec::new();
        let mut readers = Vec::new();
        let mut releases = Vec::new();
        for index in 0..3 {
            let repo = root.join(format!("worktrees/worker-{index}"));
            fixture_commit(&repo);
            std::fs::create_dir(repo.join("target")).unwrap();
            let fifo = repo.join("target/release.fifo");
            assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
            // Hold both ends so opening/writing cannot block during failure
            // cleanup, even for the refused child which never opens its FIFO.
            releases.push(std::fs::OpenOptions::new().read(true).write(true).open(&fifo).unwrap());
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
                .env(CHILD, serde_json::to_string(&[&root, &repo, &fake]).unwrap())
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit())
                .spawn().unwrap();
            let output = child.stdout.take().unwrap();
            let sender = sender.clone();
            readers.push(std::thread::spawn(move || {
                let mut lines = Vec::new();
                for line in std::io::BufReader::new(output).lines() {
                    let line = line.unwrap();
                    if line == "PAYLOAD_STARTED" || line.starts_with("RUNNER_RESULT refused:") {
                        sender.send((index, line.clone())).unwrap();
                    }
                    lines.push(line);
                }
                lines
            }));
            children.push(child);
        }
        let mut events = Vec::new();
        for _ in 0..3 {
            match receiver.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }
        // Release before assertions, including a failed handshake, so no
        // payload survives a failed regression.
        for (index, release) in releases.iter_mut().enumerate() {
            if events.len() != 3 || events.iter().any(|(child, line)|
                *child == index && line == "PAYLOAD_STARTED")
            {
                let _ = writeln!(release, "release");
            }
        }
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        let logs: Vec<_> = readers.into_iter().map(|reader| reader.join().unwrap()).collect();
        assert_eq!(events.len(), 3, "all runner processes must answer: {logs:?}");
        assert_eq!(events.iter().filter(|(_, line)| line == "PAYLOAD_STARTED").count(), 2);
        let (_, refusal) = events.iter().find(|(_, line)| line.starts_with("RUNNER_RESULT refused:")).unwrap();
        assert!(refusal.contains("Waiting for build admission"), "{refusal}");
        assert!(refusal.contains("max_concurrent_builders=2"), "{refusal}");
        let mut held_slots = Vec::new();
        for lines in logs.iter().filter(|lines| lines.iter().any(|line| line == "PAYLOAD_STARTED")) {
            // pin: AC3 audit records bind admission/release UTC to one slot,
            // rather than inferring a build lifetime from a reused log inode.
            let stamps: Vec<serde_json::Value> = lines.iter()
                .filter_map(|line| line.strip_prefix("worker-check slot: "))
                .map(|json| serde_json::from_str(json).unwrap()).collect();
            assert_eq!(stamps.len(), 2, "missing admission/release audit: {lines:?}");
            assert_eq!(stamps[0]["event"], "admitted");
            assert_eq!(stamps[1]["event"], "released");
            assert_eq!(stamps[0]["slot"], stamps[1]["slot"]);
            let slot = stamps[0]["slot"].as_u64().unwrap();
            assert!(slot < 2);
            held_slots.push(slot);
            let admitted = chrono::DateTime::parse_from_rfc3339(stamps[0]["utc"].as_str().unwrap()).unwrap();
            let released = chrono::DateTime::parse_from_rfc3339(stamps[1]["utc"].as_str().unwrap()).unwrap();
            assert_eq!(admitted.offset().local_minus_utc(), 0);
            assert!(admitted <= released);
        }
        held_slots.sort_unstable();
        assert_eq!(held_slots, [0, 1]);
        // A subsequent process can acquire a released slot.
        assert!(acquire_slot(&root.join("worker-check-slots"), 2).is_ok());
    }

    /// cas-39f3: `git status` refreshes a stat-dirty index and rewrites it
    /// under `index.lock`; SIGKILL during that write leaves a zero-byte lock
    /// (reproduced: a kill 0.86 s into status on a 5,450-file checkout). The
    /// runner only reads git state, so a builder-cap refusal, like every other
    /// path, must not take the optional index lock at all.
    #[cfg(unix)]
    /// cas-d95c: a verifier's named-test run at a bound head is admitted
    /// against the same builder cap as a worker check, in a checkout under the
    /// counted worktrees root, instead of running as uncounted plain Cargo.
    #[test]
    fn verifier_tests_are_admitted_against_the_builder_cap_cas_d95c() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("project");
        fixture_commit(&repo);
        let head = git(&repo, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let root = repo.join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[factory]\nmax_concurrent_builders = 1\n",
        )
        .unwrap();
        let slots = root.join("worker-check-slots");
        std::fs::create_dir_all(&slots).unwrap();
        let _held = acquire_slot(&slots, 1).unwrap();
        // Exercise the real verifier-test CLI transport before capped admission.
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "cas", "factory", "verifier-test", "--cas-root", root.to_str().unwrap(),
            "--task-id", "cas-proof", "--head", &head, "--",
            "nextest", "run", "-p", "crossterm@0.25.0", "--lib", "-E", "test(take_buffered)",
        ]).unwrap();
        let Some(crate::cli::Commands::Factory(factory)) = cli.command else { panic!("factory command") };
        let Some(crate::cli::factory::FactoryCommands::VerifierTest { cargo_args: args, .. }) = factory.command else { panic!("verifier-test command") };

        crate::store::init_cas_dir(&repo).unwrap();
        crate::store::open_task_store(&root)
            .unwrap()
            .add(&crate::types::Task::new("cas-proof".into(), "proof".into()))
            .unwrap();
        let error = execute_verifier_at(
            &root,
            "cas-proof",
            None,
            &head,
            &args,
            &repo,
            Path::new("/nonexistent/cargo"),
            None,
        )
        .expect_err("the only builder slot is held");
        assert!(
            error.to_string().contains("max_concurrent_builders=1"),
            "{error}"
        );
        let run = std::fs::read_dir(root.join("review-scratch/cas-proof"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            git(&run.join("repo"), &["rev-parse", "HEAD"])
                .unwrap()
                .trim(),
            head
        );

        let unnamed: Vec<String> = ["nextest", "run", "-p", "cas"]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        let error = execute_verifier_at(
            &root,
            "cas-proof",
            None,
            &head,
            &unnamed,
            &repo,
            Path::new("/nonexistent/cargo"),
            None,
        )
        .expect_err("verifiers run named tests only");
        assert!(error.to_string().contains("named tests only"), "{error}");
    }

    #[test]
    fn builder_cap_refusal_never_takes_the_index_lock_cas_39f3() {
        use std::os::unix::fs::MetadataExt;
        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.toml"), "[factory]\nmax_concurrent_builders = 1\n").unwrap();
        let repo = root.join("worktrees/worker-39f3");
        fixture_commit(&repo);
        std::fs::create_dir(repo.join("target")).unwrap();
        // Stat-dirty but content-clean: a plain `git status` would refresh
        // and rewrite the index here.
        std::fs::File::options()
            .write(true)
            .open(repo.join(".gitignore"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(30))
            .unwrap();
        let index = repo.join(".git/index");
        let before = std::fs::metadata(&index).unwrap().ino();
        let slots = root.join("worker-check-slots");
        std::fs::create_dir_all(&slots).unwrap();
        let _held = acquire_slot(&slots, 1).unwrap();

        let args: Vec<String> = ["-p", "cas", "--lib"].iter().map(|arg| arg.to_string()).collect();
        let error = execute_at(&root, &args, &repo, Path::new("/nonexistent/cargo"))
            .expect_err("the only builder slot is held");
        assert!(error.to_string().contains("max_concurrent_builders=1"), "{error}");
        assert!(!repo.join(".git/index.lock").exists(), "the refusal left an index.lock");
        assert_eq!(
            std::fs::metadata(&index).unwrap().ino(),
            before,
            "the runner rewrote the index (took the optional index lock)"
        );
    }

    /// cas-a8dc: a slot is an flock on an open file description. A child
    /// that another test thread forks holds a copy of every descriptor until
    /// it execs (O_CLOEXEC closes it only then), so under a parallel
    /// `cargo test` the slot could stay held past `drop(first)`. The body runs
    /// alone in a re-executed test process, where nothing else forks.
    #[test]
    fn slots_enforce_cap_and_release_when_closed() {
        const CHILD: &str = "CAS_WORKER_SLOT_CAP_CHILD";
        const NAME: &str = "factory_worker_check::tests::slots_enforce_cap_and_release_when_closed";
        if std::env::var_os(CHILD).is_some() {
            let dir = tempfile::tempdir().unwrap();
            let first = acquire_slot(dir.path(), 2).unwrap();
            let second = acquire_slot(dir.path(), 2).unwrap();
            let refusal = acquire_slot(dir.path(), 2).unwrap_err().to_string();
            assert!(refusal.contains("Waiting for build admission"), "{refusal}");
            assert!(refusal.contains("max_concurrent_builders=2"), "{refusal}");
            drop(first);
            assert!(acquire_slot(dir.path(), 2).is_ok());
            drop(second);
            assert!(acquire_slot(dir.path(), 0).is_err());
            return;
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        crate::test_child::assert_passed(&String::from_utf8_lossy(&output.stdout), NAME);
    }

    #[cfg(unix)]
    #[test]
    fn daemon_grandchild_cannot_retain_slot_after_runner_exit() {
        const FIXTURE: &str = "CAS_WORKER_SLOT_DAEMON_FIXTURE";
        if let Some(root) = std::env::var_os(FIXTURE) {
            let root = PathBuf::from(root);
            let _slot = acquire_slot(&root, 1).unwrap();
            let lane = lock_file(&root.join("lane.lock")).unwrap();
            lane.lock_exclusive().unwrap();
            // Model a compiler wrapper which starts a long-lived daemon.
            let status = Command::new("sh")
                .args([
                    "-c",
                    "sleep 2 >/dev/null 2>&1 & echo $! > \"$1/grandchild.pid\"",
                    "fixture",
                ])
                .arg(&root)
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "factory_worker_check::tests::daemon_grandchild_cannot_retain_slot_after_runner_exit", "--nocapture"])
            .env(FIXTURE, dir.path()).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        crate::test_child::assert_passed(
            &String::from_utf8_lossy(&output.stdout),
            "factory_worker_check::tests::daemon_grandchild_cannot_retain_slot_after_runner_exit",
        );
        let pid: i32 = std::fs::read_to_string(dir.path().join("grandchild.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: signal zero only observes the fixture's captured child PID.
        let live = unsafe { libc::kill(pid, 0) } == 0;
        let slot_available = acquire_slot(dir.path(), 1).is_ok();
        let lane = lock_file(&dir.path().join("lane.lock")).unwrap();
        let lane_available = lane.try_lock_exclusive().is_ok();
        // Stop only the grandchild PID captured by this fixture.
        let _ = Command::new("kill").arg(pid.to_string()).status();
        assert!(live, "daemon must still be live after the runner exits");
        assert!(slot_available, "daemon must not retain the builder slot");
        assert!(lane_available, "daemon must not retain the lane lock");
    }

    #[test]
    fn receipt_is_bound_to_worktree_and_exact_commit() {
        let root = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path().canonicalize().unwrap();
        let head = "a".repeat(40);
        let path = receipt_path(root.path(), &repo, &head);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let record = CheckReceipt {
            head: head.clone(),
            repo: repo.clone(),
            packages: vec!["cas".into()],
            test: None,
        };
        std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(
            passing_receipt(root.path(), &repo, &head)
                .unwrap()
                .starts_with(&format!("check: PASS {head}"))
        );
        assert!(passing_receipt(root.path(), &repo, &"b".repeat(40)).is_none());
        assert!(passing_receipt(root.path(), root.path(), &head).is_none());
        std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"cas\"\nversion = \"0.25.0\"\n").unwrap();
        record_passing_test_for_tests(root.path(), &repo, &head, "cas@0.29.0", "test(worker)");
        assert!(passing_test_packages(root.path(), &repo, &head).is_empty(),
            "a passing dependency version cannot cover the workspace crate");
        record_passing_test_for_tests(root.path(), &repo, &head, "cas@0.25.0", "test(worker)");
        assert_eq!(passing_test_packages(root.path(), &repo, &head), ["cas"]);
        assert_eq!(passing_test_receipts(root.path(), &repo, &head), [
            format!("test: PASS {head} cas@0.25.0 test(worker) 1"),
            format!("test: PASS {head} cas@0.29.0 test(worker) 1"),
        ], "receipt identities and audit lines must retain each exact version");
    }

    #[cfg(unix)]
    #[test]
    fn unignored_target_stays_legacy_clean_without_shared_git_mutation_cas_f96d() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off"), ("CAS_FACTORY_DISABLE_TARGET_SEED", "1")]);
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fixture_commit(&source);
        std::fs::write(source.join(".gitignore"), "").unwrap();
        git(&source, &["add", ".gitignore"]).unwrap();
        git(&source, &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", "unignored output"]).unwrap();
        let root = dir.path().join(".cas");
        let worker = root.join("worktrees/worker");
        git(&source, &["worktree", "add", "-q", "--detach", worker.to_str().unwrap()]).unwrap();
        let config = std::fs::read(source.join(".git/config")).unwrap();
        let exclude = std::fs::read(source.join(".git/info/exclude")).unwrap();
        assert!(crate::factory_target_cache::owner::acquire(&root, &worker).unwrap().is_none());
        assert!(!worker.join("target").exists(), "legacy fallback places no marker");
        assert!(!root.join("worker-target-owners").exists());
        let fake = dir.path().join("fake-cargo");
        fake_cargo(&fake, "exit 0");
        execute_at(&root, &["-p".into(), "cas".into(), "--lib".into()], &worker, &fake).unwrap();
        assert!(passing_receipt(&root, &worker, &fixture_head(&worker)).is_some());
        assert!(git(&worker, &["status", "--porcelain", "--untracked-files=all"]).unwrap().is_empty());
        assert_eq!(std::fs::read(source.join(".git/config")).unwrap(), config);
        assert_eq!(std::fs::read(source.join(".git/info/exclude")).unwrap(), exclude);
    }

    #[cfg(unix)]
    #[test]
    fn runner_checks_clean_commit_and_invalidates_failed_retry() {
        use std::os::unix::fs::PermissionsExt;
        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]).unwrap();
        std::fs::write(repo.join(".gitignore"), "/target/\n").unwrap();
        git(&repo, &["add", ".gitignore"]).unwrap();
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        let fake = dir.path().join("fake-cargo");
        std::fs::write(&fake, "#!/bin/sh\ncase \"$*\" in 'check -p cas --tests'|'check -p cas --lib') ;; *) exit 2 ;; esac\n[ \"$CARGO_TARGET_DIR\" = \"$PWD/target\" ] || exit 3\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let args = vec!["-p".into(), "cas".into(), "--tests".into()];
        let head = fixture_head(&repo);
        for target in ["--lib", "--tests"] {
            let args = vec!["-p".into(), "cas".into(), target.into()];
            execute_at(&root, &args, &repo, &fake).unwrap();
            assert!(passing_receipt(&root, &repo, &head).is_some());
        }
        std::fs::write(&fake, "#!/bin/sh\nexit 12\n").unwrap();
        assert!(
            execute_at(&root, &args, &repo, &fake)
                .unwrap_err()
                .to_string()
                .contains("check: FAIL")
        );
        assert!(passing_receipt(&root, &repo, &head).is_none());
        std::fs::write(repo.join("dirty.rs"), "// dirty").unwrap();
        assert!(
            execute_at(&root, &args, &repo, &fake)
                .unwrap_err()
                .to_string()
                .contains("Commit the worker change")
        );
        assert!(execute_at(&root, &args, dir.path(), &fake).is_err());
    }
}
