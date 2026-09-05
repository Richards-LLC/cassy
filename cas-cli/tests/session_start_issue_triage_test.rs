use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::TempDir;

fn cas_cmd(project: &TempDir) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = project.path().join(".test-home");
    let xdg = project.path().join(".test-xdg-config");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&xdg).unwrap();
    cmd.current_dir(project.path())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

fn init(project: &TempDir) {
    cas_cmd(project).args(["init", "--yes"]).assert().success();
}

#[cfg(unix)]
fn install_fake_gh(project: &TempDir, body: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let bin = project.path().join("fake-bin");
    fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    let log = project.path().join("gh-calls.log");
    fs::write(
        &gh,
        format!("#!/bin/sh\nprintf '%s\\n' called >> \"$GH_CALL_LOG\"\n{body}\n"),
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, log)
}

#[cfg(unix)]
fn session_start(
    project: &TempDir,
    role: &str,
    fake_bin: &Path,
    call_log: &Path,
) -> (String, Duration) {
    let input = serde_json::json!({
        "session_id": format!("issue-triage-{role}"),
        "cwd": project.path(),
        "hook_event_name": "SessionStart"
    });
    let started = Instant::now();
    let output = cas_cmd(project)
        .args(["hook", "SessionStart"])
        .env("CAS_AGENT_ROLE", role)
        .env("PATH", fake_bin)
        .env("GH_CALL_LOG", call_log)
        .write_stdin(serde_json::to_string(&input).unwrap())
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "SessionStart failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (String::from_utf8(output.stdout).unwrap(), elapsed)
}

#[cfg(unix)]
#[test]
fn configured_supervisor_sees_issue_count_recent_titles_and_reuses_fresh_cache() {
    let project = TempDir::new().unwrap();
    init(&project);
    cas_cmd(&project)
        .args(["config", "set", "issues.repo", "owner/cas"])
        .assert()
        .success();

    let response = r#"printf '%s\n' '{"data":{"repository":{"issues":{"totalCount":7,"nodes":[{"number":105,"title":"Newest report"},{"number":104,"title":"Second report"},{"number":103,"title":"Third report"}]}}}}'"#;
    let (fake_bin, call_log) = install_fake_gh(&project, response);

    let (first, _) = session_start(&project, "supervisor", &fake_bin, &call_log);
    assert!(first.contains("GitHub issue triage"), "{first}");
    assert!(first.contains("7 open"), "{first}");
    assert!(first.contains("#105 Newest report"), "{first}");
    assert!(first.contains("#104 Second report"), "{first}");
    assert!(first.contains("#103 Third report"), "{first}");
    assert!(first.contains("## Where to file bugs"), "{first}");
    for repo in [
        "owner/cas",
        "Richards-LLC/cassy",
        "Richards-LLC/mecha-cassy",
        "Richards-LLC/petra-stella-cloud",
    ] {
        assert!(first.contains(repo), "missing {repo}: {first}");
    }
    assert!(first.contains(
        "If you hit a bug during operation, file a ticket in the matching repo before moving on."
    ));

    let (second, _) = session_start(&project, "supervisor", &fake_bin, &call_log);
    assert!(second.contains("7 open"), "{second}");
    assert_eq!(fs::read_to_string(call_log).unwrap().lines().count(), 1);
}

#[cfg(unix)]
#[test]
fn issue_triage_is_silent_for_workers_and_when_repo_is_unset() {
    let worker_project = TempDir::new().unwrap();
    init(&worker_project);
    cas_cmd(&worker_project)
        .args(["config", "set", "issues.repo", "owner/cas"])
        .assert()
        .success();
    let (fake_bin, call_log) = install_fake_gh(
        &worker_project,
        r#"printf '%s\n' '{"data":{"repository":{"issues":{"totalCount":1,"nodes":[]}}}}'"#,
    );
    let (worker, _) = session_start(&worker_project, "worker", &fake_bin, &call_log);
    assert!(!worker.contains("GitHub issue triage"), "{worker}");
    assert!(!call_log.exists(), "worker session invoked gh");

    let unset_project = TempDir::new().unwrap();
    init(&unset_project);
    let (fake_bin, call_log) = install_fake_gh(
        &unset_project,
        r#"printf '%s\n' '{"data":{"repository":{"issues":{"totalCount":1,"nodes":[]}}}}'"#,
    );
    let (unset, _) = session_start(&unset_project, "supervisor", &fake_bin, &call_log);
    assert!(!unset.contains("GitHub issue triage"), "{unset}");
    assert!(!call_log.exists(), "unset issues.repo invoked gh");
}

#[cfg(unix)]
#[test]
fn issue_triage_failures_are_silent_and_timeout_is_bounded() {
    let missing_project = TempDir::new().unwrap();
    init(&missing_project);
    cas_cmd(&missing_project)
        .args(["config", "set", "issues.repo", "owner/cas"])
        .assert()
        .success();
    let empty_bin = missing_project.path().join("empty-bin");
    fs::create_dir(&empty_bin).unwrap();
    let missing_log = missing_project.path().join("missing-gh.log");
    let (missing, elapsed) =
        session_start(&missing_project, "supervisor", &empty_bin, &missing_log);
    assert!(!missing.contains("GitHub issue triage"), "{missing}");
    assert!(elapsed < Duration::from_secs(3), "missing gh: {elapsed:?}");

    // Authentication, network, and rate-limit failures all reach CAS as a
    // non-successful `gh api` exit and must share the same silent path.
    for (name, body) in [
        ("unauthenticated", "exit 4"),
        ("offline-or-rate-limited", "exit 1"),
        ("slow-gh", "sleep 5"),
    ] {
        let project = TempDir::new().unwrap();
        init(&project);
        cas_cmd(&project)
            .args(["config", "set", "issues.repo", "owner/cas"])
            .assert()
            .success();
        let (fake_bin, call_log) = install_fake_gh(&project, body);

        let (output, elapsed) = session_start(&project, "supervisor", &fake_bin, &call_log);
        assert!(!output.contains("GitHub issue triage"), "{name}: {output}");
        assert!(
            elapsed < Duration::from_secs(3),
            "{name} blocked SessionStart for {elapsed:?}"
        );
    }
}
