//! Capped compile and targeted-test evidence. The runner serializes admission
//! and holds OS slot/lane locks until Cargo exits; descendants never inherit them.
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
    let output = Command::new("git").args(args).current_dir(repo).output()?;
    if !output.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn clean_head(repo: &Path) -> Result<String> {
    if !git(repo, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
        bail!("Commit the worker change before checking; worker PASS must name a clean commit");
    }
    git(repo, &["rev-parse", "HEAD"])
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
    let mut receipts = Vec::new();
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
        let Some(record) = record else {
            continue;
        };
        let Some(test) = record.test else {
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
            receipts.push(format!(
                "test: PASS {head} {} {} {}",
                record.packages[0], test.filter, test.count
            ));
        }
    }
    receipts.sort();
    receipts.dedup();
    receipts
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
    let lane = lock_file(&slots.join(format!("lane-{lane_key}.lock")))?;
    lane.try_lock_exclusive()
        .context("This worktree already has a worker check; retry later")?;
    let receipt = match &test {
        Some(test) => test_receipt_path(&cas_root, &repo, &head, test),
        None => receipt_path(&cas_root, &repo, &head),
    };
    // A failed retry must not leave an earlier PASS at this SHA.
    match std::fs::remove_file(&receipt) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let admission = lock_file(&slots.join("admission.lock"))?;
    admission.lock_exclusive()?;
    let snapshot = crate::factory_build_guard::inspect(&cas_root, &config, 1);
    if !snapshot.violations().is_empty() {
        bail!(
            "Worker check refused: {}; retry later",
            snapshot.violations().join("; ")
        );
    }
    // OS locks close the snapshot-to-spawn race, even when the soft guard's
    // environment override is set. No force/disable option bypasses the cap.
    let slot = acquire_slot(&slots, config.max_concurrent_builders)?;
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
    let mut cargo_args = args.to_vec();
    // An omitted harness means library tests; do not build every test binary.
    if test.as_ref().is_some_and(|test| test.harness.is_none())
        && cargo_args.get(4).is_some_and(|arg| arg == "-E")
    {
        cargo_args.insert(4, "--lib".into());
    }
    let mut child = command
        .args(&cargo_args)
        .current_dir(&repo)
        .env("CARGO_TARGET_DIR", repo.join("target"))
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
        let head = clean_head(&repo).unwrap();
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
        let head = clean_head(&repo).unwrap();
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
    fn runner_checks_clean_commit_and_invalidates_failed_retry() {
        use std::os::unix::fs::PermissionsExt;
        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_BUILD_GUARD", "off")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]).unwrap();
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
        let head = clean_head(&repo).unwrap();
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
