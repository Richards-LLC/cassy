//! Fail-first evidence: a capped named run that failed, bound to the touched
//! tests it ran, and the red-control mode that produces one for a test written
//! after its fix.
//!
//! Red control runs the named tests against an uncommitted, tracked-only
//! mutation (typically the fix reverted). The mutation is snapshotted with
//! plumbing into `refs/cas/red/<task>/<n>`, so HEAD, the branch and the
//! delivery anchor never see it and an ordinary branch push never carries
//! it. The tracked files are restored to HEAD whatever the run's outcome.
use super::*;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Red {
    version: u32,
    /// The tested commit: the delivery-side commit, or a red-control snapshot.
    head: String,
    /// The commit a red-control mutation was made on; absent for committed runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    tests: Vec<RedTest>,
    log_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RedTest {
    id: String,
    fingerprint: String,
    names: Vec<String>,
    failed: Vec<String>,
}

/// A red-control snapshot of the mutation the named tests run against.
pub(super) struct Snapshot {
    pub(super) label: String,
    /// The snapshot commit, whose parent is `parent`.
    pub(super) commit: String,
    pub(super) parent: String,
    paths: Vec<String>,
}

fn path(cas_root: &Path, repo: &Path, tested: &str, test: &TargetedTest) -> PathBuf {
    let receipt = test_receipt_path(cas_root, repo, tested, test);
    let name = receipt.file_name().expect("receipt file name").to_string_lossy().into_owned();
    receipt.with_file_name(format!("red-{name}"))
}

impl Red {
    /// This red run failed `test` as it stands at `delivery`, before the change:
    /// on a different tree whose base commit is the delivery or its ancestor.
    pub(super) fn proves(&self, repo: &Path, delivery: &str, test: &stability::PlannedTest) -> bool {
        let anchored = self.parent.as_deref().unwrap_or(&self.head);
        self.tests.iter().any(|red| red.id == test.id && red.fingerprint == test.fingerprint && !red.failed.is_empty())
            && valid_git_oid(&self.head) && valid_git_oid(anchored)
            && self.parent.as_ref().is_none_or(|parent| {
                git(repo, &["rev-parse", &format!("{}^", self.head)]).is_ok_and(|first| &first == parent)
            })
            && (anchored == delivery || receipt_head_is_ancestor(repo, anchored, delivery))
            && git(repo, &["rev-parse", &format!("{}^{{tree}}", self.head)]).ok()
                != git(repo, &["rev-parse", &format!("{delivery}^{{tree}}")]).ok()
    }
}

/// Every red receipt recorded for this worktree.
pub(super) fn records(cas_root: &Path, repo: &Path) -> Vec<Red> {
    let Ok(repo) = repo.canonicalize() else { return Vec::new() };
    let Some(directory) = receipt_path(cas_root, &repo, "red").parent().map(Path::to_path_buf) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    entries.flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("red-"))
        .filter_map(|entry| serde_json::from_slice::<Red>(&std::fs::read(entry.path()).ok()?).ok())
        .filter(|red| red.version == 1)
        .collect()
}

/// Store the runner's red sidecar for a failed named run, if it wrote one.
pub(super) fn persist(
    cas_root: &Path, repo: &Path, tested: &str, test: &TargetedTest,
    count_file: &Path, snapshot: Option<&Snapshot>,
) -> Result<Option<String>> {
    let sidecar = count_file.with_extension("red.json");
    let bytes = match std::fs::read(&sidecar) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    std::fs::remove_file(&sidecar)?;
    let mut red: Red = serde_json::from_slice(&bytes).context("invalid red-run receipt")?;
    if red.head != tested || !red.tests.iter().any(|test| !test.failed.is_empty()) {
        bail!("red-run receipt does not name the tested commit {tested} and a failed touched test");
    }
    red.parent = snapshot.map(|snapshot| snapshot.parent.clone());
    red.label = snapshot.map(|snapshot| snapshot.label.clone());
    let destination = path(cas_root, repo, tested, test);
    std::fs::create_dir_all(destination.parent().context("receipt directory")?)?;
    std::fs::write(&destination, serde_json::to_vec(&red)?)?;
    let failed = red.tests.iter().filter(|test| !test.failed.is_empty())
        .map(|test| test.id.as_str()).collect::<Vec<_>>().join(",");
    Ok(Some(format!("red: FAIL {tested} tests={failed} receipt={}", destination.display())))
}

fn valid_label(label: &str) -> bool {
    (1..=64).contains(&label.len())
        && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
}

/// Snapshot the worktree's tracked-only mutation without touching HEAD,
/// the index or the branch.
fn snapshot(repo: &Path, label: &str, task: &str) -> Result<Snapshot> {
    if !valid_label(label) {
        bail!("Red-control label must be 1-64 characters of [A-Za-z0-9_.:-]");
    }
    // `git` trims the output, so read raw porcelain lines: XY, a space, the path.
    let status = Command::new("git").args(["status", "--porcelain", "--untracked-files=all"])
        .env("GIT_OPTIONAL_LOCKS", "0").current_dir(repo).output()?;
    if !status.status.success() {
        bail!("git status failed");
    }
    if String::from_utf8_lossy(&status.stdout).lines()
        .any(|line| !line.bytes().take(2).all(|code| matches!(code, b' ' | b'M' | b'D')))
    {
        bail!("Red control runs against a mutation of tracked files only; remove, unstage or commit added, renamed and untracked files first");
    }
    let parent = git(repo, &["rev-parse", "HEAD"])?;
    let paths = git(repo, &["diff", "--name-only", "--no-renames", "HEAD"])?
        .lines().map(str::to_owned).collect::<Vec<_>>();
    if paths.is_empty() {
        bail!("Red control needs an uncommitted mutation of tracked files (for example the fix reverted)");
    }
    let index = tempfile::NamedTempFile::new()?;
    let plumbing = |args: &[&str]| -> Result<String> {
        let output = Command::new("git").args(args).current_dir(repo)
            .env("GIT_INDEX_FILE", index.path()).output()?;
        if !output.status.success() {
            bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    // Start from the worktree's own index so `add -u` reuses its stat cache.
    let real_index = repo.join(git(repo, &["rev-parse", "--git-path", "index"])?);
    if std::fs::copy(&real_index, index.path()).is_err() {
        plumbing(&["read-tree", "HEAD"])?;
    }
    plumbing(&["add", "-u", "--", "."])?;
    let tree = plumbing(&["write-tree"])?;
    let commit = plumbing(&[
        "-c", "user.name=Cassy red control", "-c", "user.email=red-control@cas.invalid",
        "commit-tree", &tree, "-p", &parent, "-m", &format!("red control {label}: never a delivery"),
    ])?;
    let prefix = format!("refs/cas/red/{task}/");
    let count = git(repo, &["for-each-ref", "--format=%(refname)", &prefix])?.lines().count();
    // An empty old value creates the ref and refuses to overwrite one.
    git(repo, &["update-ref", &format!("{prefix}{}", count + 1), &commit, ""])?;
    Ok(Snapshot { label: label.to_owned(), commit, parent, paths })
}

/// Return the mutated tracked files to the snapshot's parent.
struct Restore<'a> { repo: &'a Path, snapshot: &'a Snapshot }
impl Drop for Restore<'_> {
    fn drop(&mut self) {
        let mut args = vec!["checkout", self.snapshot.parent.as_str(), "--"];
        args.extend(self.snapshot.paths.iter().map(String::as_str));
        if let Err(error) = git(self.repo, &args) {
            eprintln!("red control: restoring the mutated files failed ({error:#}); run `git checkout HEAD -- .`");
        }
    }
}

/// Run the named tests once against the worktree's mutation and record the
/// failure as fail-first evidence. A pass under the mutation is refused.
pub(super) fn execute(
    cas_root: &Path, args: &[String], cwd: &Path, cargo: &Path,
    scope_runner: Option<&Path>, isolated_test_env: Option<&Path>, label: &str,
) -> Result<()> {
    if targeted_test(args).is_none() {
        bail!("Red control needs a named targeted test: nextest run -p <crate> [--lib|--test <harness>] -E 'test(name)'");
    }
    let repo = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let task = active_task(cas_root).map_or_else(|| "untasked".to_owned(), |task| task.id);
    let snapshot = snapshot(&repo, label, &task)?;
    println!(
        "red control: {label} snapshot {} on {}; mutated files are restored afterwards (if interrupted: git checkout HEAD -- .)",
        snapshot.commit, snapshot.parent,
    );
    let restore = Restore { repo: &repo, snapshot: &snapshot };
    let result = run_capped(cas_root, args, cwd, cargo, scope_runner, isolated_test_env, Some(&snapshot));
    drop(restore);
    if result.is_err() {
        eprintln!(
            "red control {label}: mutated files restored; re-apply the mutation with `git diff HEAD {} | git apply` to retry",
            snapshot.commit,
        );
    }
    result
}

#[cfg(test)]
pub(super) fn record_for_tests(cas_root: &Path, repo: &Path, tested: &str, delivery: &str, target: &str, id: &str) {
    let planned = stability::plan(repo, target, delivery).unwrap();
    let test = planned.tests.iter().find(|test| test.id == id).expect("touched test");
    let red = Red {
        version: 1, head: tested.into(), parent: None, label: None, log_sha256: "fixture".into(),
        tests: vec![RedTest { id: id.into(), fingerprint: test.fingerprint.clone(), names: vec!["new_test".into()], failed: vec!["new_test".into()] }],
    };
    let repo = repo.canonicalize().unwrap();
    let targeted = TargetedTest { package: "cas".into(), harness: None, filter: "test(red)".into() };
    let destination = path(cas_root, &repo, tested, &targeted);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(destination, serde_json::to_vec(&red).unwrap()).unwrap();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Red control records a failing run against the mutation, then leaves
    /// HEAD, the branch and the worktree exactly as they were; only the
    /// snapshot ref names the mutation, and a pass under it is refused.
    #[test]
    fn red_control_records_the_mutation_red_and_leaves_the_branch_untouched() {
        let _env = crate::test_support::TestEnvGuard::with_optional_vars(
            &[("CAS_FACTORY_BUILD_GUARD", Some("off")), ("CAS_SESSION_ID", None)],
        );
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/worker");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let commit = |message: &str| {
            git(&repo, &["add", "-A"]).unwrap();
            git(&repo, &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-qm", message]).unwrap();
            git(&repo, &["rev-parse", "HEAD"]).unwrap()
        };
        git(&repo, &["init", "-q", "-b", "factory/worker"]).unwrap();
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"cas\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(repo.join("src/lib.rs"), "pub fn ok() -> bool { false }\n").unwrap();
        commit("base");
        let fixed = "pub fn ok() -> bool { true }\n#[test]\nfn touched() { assert!(ok()); }\n";
        std::fs::write(repo.join("src/lib.rs"), fixed).unwrap();
        let head = commit("fix with its test");
        let fake = dir.path().join("fake-cargo");
        let run = |outcome: &str| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&fake, format!(
                r#"#!/bin/sh
if [ "$2" = list ]; then printf '{{"rust-suites":{{"cas":{{"testcases":{{"touched":{{"ignored":false}}}}}}}}}}\n'; exit 0; fi
{outcome}
"#)).unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        run("printf '        FAIL [   0.010s] (1/1) cas touched\\n     Summary [ 0.01s] 1 test run: 0 passed, 1 failed\\n'; exit 100");
        let args = ["nextest", "run", "-p", "cas", "--lib", "-E", "test(touched)"].map(str::to_owned);
        std::fs::write(repo.join("src/lib.rs"), fixed.replace("true", "false")).unwrap();
        execute(&root, &args, &repo, &fake, None, None, "revert-fix").unwrap();

        assert_eq!(git(&repo, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(git(&repo, &["symbolic-ref", "--short", "HEAD"]).unwrap(), "factory/worker");
        assert_eq!(git(&repo, &["status", "--porcelain", "--untracked-files=all"]).unwrap(), "");
        let snapshot = git(&repo, &["rev-parse", "refs/cas/red/untasked/1"]).unwrap();
        assert_eq!(git(&repo, &["rev-parse", &format!("{snapshot}^")]).unwrap(), head);
        assert!(git(&repo, &["show", &format!("{snapshot}:src/lib.rs")]).unwrap().contains("false"));
        let reds = records(&root, &repo);
        let planned = stability::PlannedTest {
            id: "src/lib.rs::touched".into(),
            fingerprint: reds[0].tests[0].fingerprint.clone(),
            runner: "rust".into(),
        };
        assert!(reds[0].proves(&repo, &head, &planned), "{reds:?}");
        assert!(!reds[0].proves(&repo, &head, &stability::PlannedTest { fingerprint: "edited".into(), ..planned }));

        run("printf '        PASS [   0.010s] (1/1) cas touched\\n     Summary [ 0.01s] 1 test run: 1 passed, 0 skipped\\n'");
        std::fs::write(repo.join("src/lib.rs"), fixed.replace("true", "false")).unwrap();
        assert!(execute(&root, &args, &repo, &fake, None, None, "revert-fix").is_err());
        assert_eq!(git(&repo, &["status", "--porcelain", "--untracked-files=all"]).unwrap(), "");
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]).unwrap(), head);
    }
}
