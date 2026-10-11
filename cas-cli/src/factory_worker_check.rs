//! Capped compile and targeted-test evidence. The runner serializes admission
//! and holds OS slot/lane locks until Cargo exits; descendants never inherit them.
//! A separate target lifetime lease is inherited to protect surviving children.
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    if !valid_package(&package) {
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
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(repo.join("Cargo.toml"))?)?;
    let mut manifests = vec![repo.join("Cargo.toml")];
    if let Some(members) = manifest
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(toml::Value::as_array)
    {
        for member in members.iter().filter_map(toml::Value::as_str) {
            let pattern = repo.join(member).join("Cargo.toml");
            manifests.extend(
                glob::glob(&pattern.to_string_lossy())?
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
            != Some(test.package.as_str())
        {
            continue;
        }
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
        if found {
            return Ok(());
        }
        bail!(
            "Worker test refused: {harness} is not an explicit test harness in {} (cas-test-targets.py inventory)",
            test.package
        );
    }
    bail!(
        "Worker test refused: package {} not found in workspace",
        test.package
    )
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

fn acquire_slot(root: &Path, cap: usize) -> Result<File> {
    std::fs::create_dir_all(root)?;
    for index in 0..cap {
        let slot = lock_file(&root.join(format!("slot-{index}.lock")))?;
        match slot.try_lock_exclusive() {
            Ok(()) => return Ok(slot),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("Worker check refused: max_concurrent_builders={cap} slots are occupied; retry later")
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

fn worker_zig(
    repo: &Path,
    configured: Option<&OsStr>,
    search_path: Option<&OsStr>,
) -> Result<Option<PathBuf>> {
    let common = PathBuf::from(git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?);
    let source = common
        .parent()
        .context("Git common directory has no parent")?;
    // The runner also serves projects which do not consume the Zig toolchain.
    if !repo.join("crates/ghostty_vt_sys/build.rs").is_file()
        && !source.join("crates/ghostty_vt_sys/build.rs").is_file()
    {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    if let Some(configured) = configured.filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(configured));
    }
    if let Some(search_path) = search_path {
        candidates.extend(std::env::split_paths(search_path).map(|dir| dir.join("zig")));
    }
    candidates.extend([
        repo.join(".context/zig/zig"),
        source.join(".context/zig/zig"),
    ]);
    for candidate in candidates {
        // Relative ZIG/PATH entries must be bound before Cargo changes cwd.
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            repo.join(candidate)
        };
        if executable(&candidate) {
            return Ok(Some(candidate.canonicalize()?));
        }
    }
    bail!(
        "Missing Zig compiler in ZIG, PATH or source/main checkout .context.\nRun ./scripts/bootstrap-zig.sh in the source repo, or set an absolute ZIG."
    )
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

pub(crate) fn passing_test_receipts(cas_root: &Path, repo: &Path, head: &str) -> Vec<String> {
    let mut receipts = passing_test_records(cas_root, repo, head)
        .into_iter()
        .map(|(package, test)| {
            format!("test: PASS {head} {package} {} {}", test.filter, test.count)
        })
        .collect::<Vec<_>>();
    receipts.sort();
    receipts.dedup();
    receipts
}

/// cas-b38a: the packages with at least one passing targeted test at `head`
/// in this worktree, sorted and once each. The close gate counts a delivered
/// crate as tested when its package is listed here.
pub(crate) fn passing_test_packages(cas_root: &Path, repo: &Path, head: &str) -> Vec<String> {
    let mut packages = passing_test_records(cas_root, repo, head)
        .into_iter()
        .map(|(package, _)| package)
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    packages
}

/// Every valid passing targeted-test receipt for `head` in this worktree.
fn passing_test_records(cas_root: &Path, repo: &Path, head: &str) -> Vec<(String, TestReceipt)> {
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
    let mut records = Vec::new();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{head}-test-"))
        {
            continue;
        }
        let record = std::fs::read(entry.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CheckReceipt>(&bytes).ok());
        let Some(mut record) = record else {
            continue;
        };
        let Some(test) = record.test.take() else {
            continue;
        };
        if record.repo == repo
            && record.head == head
            && record.packages.len() == 1
            && valid_package(&record.packages[0])
            && named_filter(&test.filter)
            && test.count > 0
            && test
                .harness
                .as_ref()
                .is_none_or(|harness| valid_package(harness))
        {
            records.push((record.packages.remove(0), test));
        }
    }
    records
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
pub(crate) struct LaneLock(File);
impl Drop for LaneLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
pub(crate) fn try_lock_lane(cas_root: &Path, repo: &Path) -> std::io::Result<Option<LaneLock>> {
    let repo = repo.canonicalize()?;
    let slots = cas_root.join("worker-check-slots");
    std::fs::create_dir_all(&slots)?;
    let key = hex::encode(Sha256::digest(repo.as_os_str().as_encoded_bytes()));
    let lock = lock_file(&slots.join(format!("lane-{key}.lock"))).map_err(std::io::Error::other)?;
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(Some(LaneLock(lock))),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error),
    }
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

pub(crate) fn execute(cas_root: &Path, args: &[String]) -> Result<()> {
    execute_at(
        cas_root,
        args,
        &std::env::current_dir()?,
        Path::new("cargo"),
    )
}

fn execute_at(cas_root: &Path, args: &[String], cwd: &Path, cargo: &Path) -> Result<()> {
    let test = targeted_test(args);
    let packages = match &test {
        Some(test) => vec![test.package.clone()],
        None => check_packages(args).context("Only package-scoped cargo check or nextest run -p <crate> [--lib|--test <harness>] -E 'test(name)' is allowed")?,
    };
    let repo = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let cas_root = cas_root.canonicalize()?;
    // Check only isolated, seeded worker caches; never use a shared target.
    if !repo.starts_with(cas_root.join("worktrees")) {
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
    let _lane = try_lock_lane(&cas_root, &repo)?
        .context("This worktree already has a worker check; retry later")?;
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
    let zig = worker_zig(
        &repo,
        std::env::var_os("ZIG").as_deref(),
        std::env::var_os("PATH").as_deref(),
    )?;
    let admission = lock_file(&slots.join("admission.lock"))?;
    admission.lock_exclusive()?;
    let snapshot = crate::factory_build_guard::inspect(&cas_root, &config, 1);
    let violations = snapshot.admission_violations(load_admitted);
    if !violations.is_empty() {
        bail!("Worker check refused: {}; retry later", violations.join("; "));
    }
    if load_admitted && !snapshot.violations().is_empty() {
        println!(
            "Worker check: continuing the proof that just passed at {head}; its load admission carries over (load_1m={:?}, cpu_count={})",
            snapshot.load_1m, snapshot.cpu_count
        );
    }
    // OS locks close the snapshot-to-spawn race, even when the soft guard's
    // environment override is set. No force/disable option bypasses the cap.
    let slot = acquire_slot(&slots, config.max_concurrent_builders)?;
    // A parked target may contain proof logs but no debug artifacts. Seed only
    // missing children from the published immutable baseline under the lane lock.
    if let Err(error) = crate::ui::factory::seed_worker_target_from_baseline(&cas_root, &repo) {
        tracing::warn!(%error, "worker target re-seed skipped; Cargo will rebuild privately");
    }
    #[cfg(unix)]
    let target_lease = crate::factory_target_cache::owner::acquire(&cas_root, &repo)?;
    #[cfg(unix)]
    discard_shared_fingerprints(&repo.join("target"))?;
    let count_file = slots.join(format!("count-{lane_key}"));
    let mut command = if test.is_some() {
        // Embed the shared zero-test guard so projects need no cas-src scripts.
        let mut command = Command::new("bash");
        command
            .arg("-c")
            .arg(include_str!("../../scripts/run-verified-tests.sh"))
            .arg("worker-verified-tests")
            .env("VERIFIED_TEST_REPO_ROOT", &repo)
            .env("VERIFIED_TEST_COUNT_FILE", &count_file)
            .env_remove("VERIFIED_TEST_LOG")
            .env("CARGO", cargo);
        command
    } else {
        let mut command = Command::new(cargo);
        command.arg("check");
        command
    };
    if let Some(zig) = zig {
        command.env("ZIG", zig);
    }
    let mut cargo_args = args.to_vec();
    // An omitted harness means library tests; do not build every test binary.
    if test.as_ref().is_some_and(|test| test.harness.is_none())
        && cargo_args.get(4).is_some_and(|arg| arg == "-E")
    {
        cargo_args.insert(4, "--lib".into());
    }
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
    let mut child = command
        .args(&cargo_args)
        .current_dir(&repo)
        .env("CARGO_TARGET_DIR", repo.join("target"))
        // cas-4b15: a cold sccache client would pass the deliberately inherited
        // target lease to its long-lived server. Empty overrides also defeat
        // Cargo config/CARGO_BUILD_* fallbacks, including workspace wrappers.
        // Cargo/rustc descendants still inherit the output lifetime lease.
        .env("RUSTC_WRAPPER", "")
        .env("RUSTC_WORKSPACE_WRAPPER", "")
        .spawn()
        .context("start capped worker Cargo command")?;
    FileExt::unlock(&admission)?;
    drop(admission);
    let status = child.wait()?;
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
            "test: PASS {head} {} {} {}",
            record.packages[0], test.filter, test.count
        );
    } else {
        println!("check: PASS {head} packages={}", record.packages.join(","));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        write(&pass("abc", &packages, Some(TestReceipt { filter: "test(x)".into(), harness: None, count: 1 })));
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
            max_concurrent_builders: 2,
            disabled: false,
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
        let output = crate::hooks::handlers::handle_pre_tool_use(&request, Some(&root)).unwrap();
        let value = serde_json::to_value(output).unwrap();
        let rewritten = value
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|value| value.as_str())
            .unwrap();
        assert!(rewritten.contains("factory worker-check"));
        assert!(rewritten.ends_with("> target/worker-check.log 2>&1 &"));
        let head = fixture_head(&repo);
        // Execute the admitted shell suffix before invoking the real runner
        // boundary. The Cargo stand-in exits zero and compiles nothing.
        let suffix = rewritten.split_once('>').unwrap().1;
        assert!(
            Command::new("sh")
                .args(["-c", &format!(": >{suffix} wait")])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
        assert!(repo.join("target/worker-check.log").exists());
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
    }

    #[test]
    fn harnesses_use_explicit_manifest_inventory() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(
            repo.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"package\"]\n",
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

    #[cfg(unix)]
    fn fake_cargo(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn zig_resolution_precedence_and_unrelated_project() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo with spaces");
        fixture_commit(&repo);
        let repo = repo.canonicalize().unwrap();
        let empty_path = OsStr::new("");
        assert!(worker_zig(&repo, None, Some(empty_path)).unwrap().is_none());
        let consumer = repo.join("crates/ghostty_vt_sys/build.rs");
        std::fs::create_dir_all(consumer.parent().unwrap()).unwrap();
        std::fs::write(consumer, "// Zig consumer").unwrap();
        let fallback = repo.join(".context/zig/zig");
        std::fs::create_dir_all(fallback.parent().unwrap()).unwrap();
        fake_cargo(&fallback, "exit 0");
        let configured = repo.join("configured-zig");
        fake_cargo(&configured, "exit 0");
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let on_path = bin.join("zig");
        fake_cargo(&on_path, "exit 0");
        assert_eq!(
            worker_zig(
                &repo,
                Some(OsStr::new("configured-zig")),
                Some(bin.as_os_str())
            )
            .unwrap(),
            Some(configured)
        );
        assert_eq!(
            worker_zig(&repo, Some(OsStr::new("missing")), Some(bin.as_os_str())).unwrap(),
            Some(on_path.canonicalize().unwrap())
        );
        assert_eq!(
            worker_zig(&repo, None, Some(empty_path)).unwrap(),
            Some(fallback.clone())
        );
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = worker_zig(&repo, None, Some(empty_path))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Missing Zig compiler"));
        assert!(error.contains("bootstrap-zig.sh"));
    }

    #[cfg(unix)]
    #[test]
    fn zig_runner_exports_main_checkout_toolchain_and_refuses_before_cargo() {
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
        let zig = source.join(".context/zig/zig");
        std::fs::create_dir_all(zig.parent().unwrap()).unwrap();
        fake_cargo(&zig, "exit 0");
        let zig = zig.canonicalize().unwrap();
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
        // Preserve the tools needed by Git and the zero-test guard, but omit Zig.
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let host_path = std::env::var_os("PATH").unwrap();
        for tool in ["git", "bash", "rm", "mktemp", "tee", "sed", "grep", "awk"] {
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
            r#"[ "$ZIG" = "$(cd "$PWD/../../.." && pwd)/.context/zig/zig" ] || exit 9
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
        std::fs::remove_file(zig).unwrap();
        let error = execute_at(&root, &args, &repo, &fake)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Missing Zig compiler"), "{error}");
        assert!(passing_receipt(&root, &repo, &head).is_none());
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

    #[cfg(unix)]
    #[test]
    fn targeted_runner_guards_counts_dirty_trees_and_failed_retries() {
        let _env =
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
        execute_at(&root, &args, &repo, &fake).unwrap();
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
    fn six_simultaneous_invocations_respect_cap_even_with_soft_guard_off() {
        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[factory]\nmax_concurrent_builders = 2\n",
        )
        .unwrap();
        let fake = dir.path().join("fake-cargo");
        // Admitted children remain live until the controller releases them;
        // this makes every refusal compete with two occupied OS slot locks.
        fake_cargo(
            &fake,
            r#"touch "$PWD/target/started"
while [ ! -e "$PWD/../../release" ]; do sleep 0.01; done
printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'"#,
        );
        let mut repos = Vec::new();
        for index in 0..6 {
            let repo = root.join(format!("worktrees/worker-{index}"));
            fixture_commit(&repo);
            std::fs::create_dir(repo.join("target")).unwrap();
            repos.push(repo);
        }
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut threads = Vec::new();
        for repo in &repos {
            let (repo, root, fake, barrier, sender) = (
                repo.clone(),
                root.clone(),
                fake.clone(),
                barrier.clone(),
                sender.clone(),
            );
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                let args = vec![
                    "nextest".into(),
                    "run".into(),
                    "-p".into(),
                    "cas".into(),
                    "--lib".into(),
                    "-E".into(),
                    "test(worker)".into(),
                ];
                let result = execute_at(&root, &args, &repo, &fake);
                sender
                    .send(result.map_err(|error| error.to_string()))
                    .unwrap();
            }));
        }
        let mut refusals = Vec::new();
        for _ in 0..4 {
            match receiver.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(result) => refusals.push(result),
                Err(_) => break,
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while repos
            .iter()
            .filter(|repo| repo.join("target/started").exists())
            .count()
            < 2
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let started = repos
            .iter()
            .filter(|repo| repo.join("target/started").exists())
            .count();
        // Release before assertions so a failing test cannot strand children.
        std::fs::write(root.join("release"), "go").unwrap();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(
            refusals.len(),
            4,
            "four invocations must be refused before release"
        );
        for refusal in refusals {
            assert!(refusal.unwrap_err().contains("max_concurrent_builders=2"));
        }
        assert_eq!(started, 2);
        for _ in 0..2 {
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .unwrap();
        }
        assert_eq!(
            repos
                .iter()
                .map(|repo| passing_test_receipts(&root, repo, &clean_head(repo).unwrap()).len())
                .sum::<usize>(),
            2
        );
    }

    /// cas-39f3: `git status` refreshes a stat-dirty index and rewrites it
    /// under `index.lock`; SIGKILL during that write leaves a zero-byte lock
    /// (reproduced: a kill 0.86 s into status on a 5,450-file checkout). The
    /// runner only reads git state, so a builder-cap refusal, like every other
    /// path, must not take the optional index lock at all.
    #[cfg(unix)]
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

    #[test]
    fn slots_enforce_cap_and_release_when_closed() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire_slot(dir.path(), 2).unwrap();
        let second = acquire_slot(dir.path(), 2).unwrap();
        assert!(acquire_slot(dir.path(), 2).is_err());
        drop(first);
        assert!(acquire_slot(dir.path(), 2).is_ok());
        drop(second);
        assert!(acquire_slot(dir.path(), 0).is_err());
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
