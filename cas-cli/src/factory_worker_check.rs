//! Capped compile and targeted-test evidence. The runner serializes admission
//! and holds OS slot/lane locks until Cargo exits; descendants never inherit them.
//! A separate target lifetime lease is inherited to protect surviving children.
use std::collections::BTreeMap;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_path_opt_out: Option<String>,
}

impl TestReceipt {
    fn path_note(&self) -> String {
        self.host_path_opt_out.as_ref()
            .map(|reason| format!(" [host PATH opt-out: {reason}]"))
            .unwrap_or_default()
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

fn waiting_for_admission(reason: &str) -> String {
    format!("Waiting for build admission: {reason}.\nRetry the same capped Cargo command when host pressure or a builder slot clears.")
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
    bail!("{}", waiting_for_admission(&format!("max_concurrent_builders={cap} slots are occupied")))
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
        .map(|(receipt_head, package, test)| {
            let rebind = (receipt_head != head)
                .then(|| format!(" rebound from {receipt_head} to {head}"))
                .unwrap_or_default();
            format!(
                "test: PASS {receipt_head} {package} {} {}{}{rebind}",
                test.filter,
                test.count,
                test.path_note()
            )
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
        .map(|(_, package, _)| package)
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    packages
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

    for (receipt_head, package, test) in ancestors {
        let key = (package, test.harness.clone(), test.filter.clone());
        if exact.contains_key(&key)
            || !receipt_range_has_no_proof_inputs(&repo, &receipt_head, head)
        {
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
        || !valid_package(&record.packages[0])
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

fn receipt_range_has_no_proof_inputs(
    repo: &Path,
    receipt_head: &str,
    delivered_head: &str,
) -> bool {
    if !valid_git_oid(receipt_head)
        || !valid_git_oid(delivered_head)
        || !receipt_head_is_ancestor(repo, receipt_head, delivered_head)
    {
        return false;
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
        return false;
    };
    changes.status.success()
        && changes.stdout.split(|byte| *byte == 0).all(|path| {
            path.is_empty()
                || std::str::from_utf8(path)
                    .is_ok_and(|path| !is_test_or_build_input_path(path))
        })
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
    let slots = cas_root.join("worker-check-slots");
    std::fs::create_dir_all(&slots)?;
    let lock = lock_file(&lane_lock_path(cas_root, repo)?).map_err(std::io::Error::other)?;
    match lock.try_lock_exclusive() {
        Ok(()) => Ok(Some(LaneLock(lock))),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error),
    }
}

fn lane_lock_path(cas_root: &Path, repo: &Path) -> std::io::Result<PathBuf> {
    let repo = repo.canonicalize()?;
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
        bail!("{}", waiting_for_admission(&violations.join("; ")));
    }
    if load_admitted && !snapshot.violations().is_empty() {
        println!(
            "Worker check: continuing the proof that just passed at {head}; its CPU admission carries over (load_1m={:?}, cpu_count={})",
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
    // cas-3f93: the private target, relocated when `[factory] scratch_root` is set.
    let target_dir = crate::factory_scratch::worker_target_dir(&repo);
    #[cfg(unix)]
    discard_shared_fingerprints(&target_dir)?;
    let count_file = slots.join(format!("count-{lane_key}"));
    let host_path_opt_out = if test.is_some() { host_path_opt_out()? } else { None };
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
        command
    } else {
        let mut command = Command::new(cargo);
        command.arg("check");
        command
    };
    #[cfg(unix)]
    if let Some((_, path)) = &test_path {
        command.env("PATH", path);
    }
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
    let mut child = command
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
                host_path_opt_out,
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
        write(&pass("abc", &packages, Some(TestReceipt { filter: "test(x)".into(), harness: None, count: 1, host_path_opt_out: None })));
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
        // A pipe handshake holds admitted payloads until all N+1 processes
        // report admission/refusal. No compiler, host load, or sleep race.
        fake_cargo(&fake, r#"printf 'PAYLOAD_STARTED\n'
IFS= read -r release
printf '     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\n'"#);
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut children = Vec::new();
        let mut readers = Vec::new();
        for index in 0..3 {
            let repo = root.join(format!("worktrees/worker-{index}"));
            fixture_commit(&repo);
            std::fs::create_dir(repo.join("target")).unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
                .env(CHILD, serde_json::to_string(&[&root, &repo, &fake]).unwrap())
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
                .spawn().unwrap();
            let output = child.stdout.take().unwrap();
            let sender = sender.clone();
            readers.push(std::thread::spawn(move || {
                let mut lines = Vec::new();
                for line in std::io::BufReader::new(output).lines() {
                    let line = line.unwrap();
                    if line == "PAYLOAD_STARTED" || line.starts_with("RUNNER_RESULT refused:") {
                        sender.send(line.clone()).unwrap();
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
        for child in &mut children {
            if let Some(mut input) = child.stdin.take() {
                let _ = writeln!(input, "release");
            }
        }
        for child in &mut children {
            assert!(child.wait().unwrap().success());
        }
        let logs: Vec<_> = readers.into_iter().map(|reader| reader.join().unwrap()).collect();
        assert_eq!(events.len(), 3, "all runner processes must answer: {logs:?}");
        assert_eq!(events.iter().filter(|line| *line == "PAYLOAD_STARTED").count(), 2);
        let refusal = events.iter().find(|line| line.starts_with("RUNNER_RESULT refused:")).unwrap();
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
