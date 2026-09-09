use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn cas_cmd(project: &TempDir) -> Command {
    let home = project.path().join(".test-home");
    let xdg = project.path().join(".test-xdg-config");
    cas_cmd_at(project.path(), &home, &xdg)
}

fn cas_cmd_at(project: &Path, home: &Path, xdg: &Path) -> Command {
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&xdg).unwrap();
    let mut command = Command::new(cas::test_paths::cas_binary());
    command
        .current_dir(project)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}

#[cfg(unix)]
fn install_fake_gh(project: &TempDir) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = project.path().join("fake-gh");
    fs::write(
        &path,
        r##"#!/bin/sh
case "$1 $2" in
  "release view")
    printf '%s' '{"name":"v3.19.0","url":"https://github.com/example/project/releases/tag/v3.19.0","body":"Release closes #705.","publishedAt":"2026-09-08T21:00:00Z","isDraft":false,"assets":[{"name":"cas-v3.19.0.tar.gz","size":42,"url":"https://github.com/example/project/releases/download/v3.19.0/cas-v3.19.0.tar.gz","digest":"sha256:abc123","contentType":"application/gzip"}]}'
    ;;
  "pr list")
    printf '%s' '[{"number":900,"title":"Release v3.19.0","body":"Closes #746","url":"https://github.com/example/project/pull/900","closingIssuesReferences":[{"number":746}],"mergedAt":"2026-09-08T20:00:00Z"}]'
    ;;
  "issue list")
    printf '%s' '[{"number":705,"title":"Factory worker report","url":"https://github.com/example/project/issues/705","state":"CLOSED","closedAt":"2026-09-01T01:02:03Z","labels":[{"name":"factory"}],"body":"Improve worker observability."},{"number":746,"title":"Simpler install","url":"https://github.com/example/project/issues/746","state":"CLOSED","closedAt":"2026-09-02T01:02:03Z","labels":[],"body":"Make install updates easier."}]'
    ;;
  *) exit 1 ;;
esac
"##
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn install_followup_fake_gh(project: &TempDir) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = project.path().join("fake-gh-followup");
    fs::write(
        &path,
        r##"#!/bin/sh
case "$1 $2" in
  "release view")
    printf '%s' '{"name":"CAS v3.19.0","url":"https://github.com/example/project/releases/tag/v3.19.0","body":"Release closes #767.","publishedAt":"2026-09-09T16:51:40Z","isDraft":false,"assets":[]}'
    ;;
  "pr list")
    printf '%s' '[{"number":785,"title":"Release v3.19.0","body":"Closes #767","url":"https://github.com/example/project/pull/785","closingIssuesReferences":[],"mergedAt":"2026-09-09T16:29:20Z"}]'
    ;;
  "issue list")
    printf '%s' '[{"number":767,"title":"Close gates","url":"https://github.com/example/project/issues/767","state":"CLOSED","closedAt":"2026-09-09T14:47:28Z","labels":[],"body":"Delivery range attribution."}]'
    ;;
  *) exit 1 ;;
esac
"##
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn cli_release_report_renders_fixture_html_and_preserves_source() {
    let project = TempDir::new().unwrap();
    fs::write(
        project.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [3.19.0] - 2026-09-08\n\n### Added\n- Worker report improvements (#705)\n\n### Fixed\n- Simpler install (#746)\n\n## [3.18.0] - 2026-08-01\n\n### Changed\n- Older change\n",
    )
    .unwrap();
    fs::create_dir_all(project.path().join("docs/release-notes")).unwrap();
    fs::write(
        project.path().join("docs/release-notes/v3.19.0.md"),
        "# v3.19.0 release notes\n\nCloses #746.\n",
    )
    .unwrap();

    cas_cmd(&project).args(["init", "--yes"]).assert().success();
    cas_cmd(&project)
        .args(["config", "set", "issues.repo", "example/project"])
        .assert()
        .success();
    let fake_gh = install_fake_gh(&project);

    let first = cas_cmd(&project)
        .env("GH_BIN", &fake_gh)
        .args([
            "--json",
            "release",
            "report",
            "3.19.0",
            "--out",
            "docs/release-reports",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(first["source_written"], true);
    assert_eq!(first["html_written"], true);
    assert_eq!(first["issue_count"], 2);
    assert_eq!(first["asset_count"], 1);

    let source_path = project.path().join("docs/release-reports/v3.19.0.md");
    let html_path = project.path().join("docs/release-reports/v3.19.0.html");
    let source = fs::read_to_string(&source_path).unwrap();
    let html = fs::read_to_string(&html_path).unwrap();
    if let Some(fixture_dir) = std::env::var_os("CAS_RELEASE_REPORT_FIXTURE_DIR") {
        let fixture_dir = PathBuf::from(fixture_dir);
        fs::create_dir_all(&fixture_dir).unwrap();
        fs::copy(&source_path, fixture_dir.join("v3.19.0.md")).unwrap();
        fs::copy(&html_path, fixture_dir.join("v3.19.0.html")).unwrap();
    }
    assert!(source.starts_with("---\nversion: 3.19.0\n"));
    assert!(source.contains("## Change map\n"));
    assert!(source.contains("| Factory | 1 | #705 |"));
    assert!(source.contains("| Install | 1 | #746 |"));
    assert!(source.contains("sha256:abc123"));
    assert!(html.contains("<html lang=\"en\">"));
    assert!(html.contains("data-change-map=\"1\""));
    assert!(html.contains("data-issue=\"705\""));
    assert!(html.contains("data-issue=\"746\""));

    let second = cas_cmd(&project)
        .env("GH_BIN", &fake_gh)
        .args([
            "--json",
            "release",
            "report",
            "v3.19.0",
            "--out",
            "docs/release-reports",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second: Value = serde_json::from_slice(&second).unwrap();
    assert_eq!(second["source_written"], false);
    assert_eq!(fs::read_to_string(&source_path).unwrap(), source);
    assert_eq!(fs::read_to_string(&html_path).unwrap(), html);
}

#[cfg(unix)]
#[test]
fn cli_release_report_resolves_main_config_from_a_git_worktree() {
    let project = TempDir::new().unwrap();
    let run = |args: &[&str]| {
        let output = ProcessCommand::new("git")
            .current_dir(project.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(["init", "--quiet"].as_slice());
    run(["config", "user.email", "fixture@example.test"].as_slice());
    run(["config", "user.name", "Fixture"].as_slice());

    fs::write(
        project.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [3.19.0] - 2026-09-08\n\n### Changed\n- Close gates judge only the task's own delivery and preserve exact repository proof. (#767)\n",
    )
    .unwrap();
    fs::create_dir_all(project.path().join("docs/release-notes")).unwrap();
    fs::write(
        project.path().join("docs/release-notes/v3.19.0.md"),
        "# v3.19.0 release notes\n\nUser top-level\n\n```text\n*Live on production — User — Cassy v3.19.0*\nWas: the report had no trusted closure register. Now: the release is ready to inspect.\n```\n\nUser reply\n\n```text\n*Release reports*\n\n• *Build the report* — Was: reports were manual. Now: one command builds them.\n\n*Already live on hosts that updated from main*\n\n• *Recall memory* — Was: recall could disappear. Now: memory stays visible.\n\n• *Fair task closes* — Was: an old task could block a close. Now: verification checks the task's own range (#767).\n\n• *Honest start-up line* — Was: worker identity was unclear. Now: the provider is named.\n```\n\nDev top-level\n\n```text\n*Live on production — Dev — Cassy v3.19.0*\nWas: report sources were gathered manually. Now: the assembler gathers them.\n```\n\nDev reply\n\n```text\n*Release report command*\n\n• *Assembler* — Was: sources were manual. Now: release reports are assembled.\n```\n",
    )
    .unwrap();
    run(["add", "CHANGELOG.md", "docs/release-notes/v3.19.0.md"].as_slice());
    run(["commit", "--quiet", "-m", "fixture"].as_slice());

    cas_cmd(&project).args(["init", "--yes"]).assert().success();
    cas_cmd(&project)
        .args(["config", "set", "issues.repo", "example/project"])
        .assert()
        .success();
    let config_path = project.path().join(".cas/config.toml");
    let mut config = fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[project]\ncanonical_id = \"configured-project\"\n");
    fs::write(config_path, config).unwrap();

    let worktree = project.path().join("report-worktree");
    run([
        "worktree",
        "add",
        "--quiet",
        "-b",
        "report-fixture",
        worktree.to_str().unwrap(),
        "HEAD",
    ]
    .as_slice());
    let fake_gh = install_followup_fake_gh(&project);
    let home = project.path().join(".test-home");
    let xdg = project.path().join(".test-xdg-config");
    let receipt_dir = home.join(".cas/artifacts/release/v3.19.0-fixture");
    fs::create_dir_all(&receipt_dir).unwrap();
    fs::write(receipt_dir.join("gate.green.epoch"), "1788970165\n").unwrap();
    fs::write(
        receipt_dir.join("release-published.receipt"),
        "TAG=v3.19.0\nPUBLISHED_AT=2026-09-09T16:51:40Z\n",
    )
    .unwrap();
    let output = cas_cmd_at(&worktree, &home, &xdg)
        .env("GH_BIN", &fake_gh)
        .args([
            "--json",
            "release",
            "report",
            "3.19.0",
            "--out",
            "docs/release-reports",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let result: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["project"], "configured-project");
    assert_eq!(result["github_repo"], "example/project");
    assert_eq!(result["issue_count"], 1);
    assert_eq!(result["release_published_at"], "2026-09-09T16:51:40Z");
    assert_eq!(
        result["theme_counts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|theme| theme["theme"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["Release", "Memory", "Verification", "Factory"]
    );
    assert!(
        !result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("issues.repo is unset"))
    );

    let source = fs::read_to_string(worktree.join("docs/release-reports/v3.19.0.md")).unwrap();
    let (user_section, developer_and_rest) = source.split_once("## Under the hood").unwrap();
    assert!(source.contains("Published 9 September 2026 · 16:51 UTC"));
    assert!(source.contains("the release is ready to inspect."));
    assert!(source.contains("themes: [\"Release\", \"Memory\", \"Verification\", \"Factory\"]"));
    assert!(source.contains("| Verification | 1 | #767 |"));
    assert!(!source.contains("has no verified closed GitHub issues"));
    assert!(user_section.contains("### Release reports"));
    assert!(user_section.contains("### Already live on hosts that updated from main"));
    assert!(user_section.contains("one command builds them."));
    assert!(!user_section.contains("#### Live on production"));
    assert!(developer_and_rest.contains("### Release report command"));
    assert!(!developer_and_rest.contains("the assembler gathers them."));
    assert!(developer_and_rest.contains("release reports are assembled."));
    assert!(!developer_and_rest.contains("#### Live on production"));
    assert!(
        !developer_and_rest[..developer_and_rest.find("## Fixes ledger").unwrap()]
            .contains("one command builds the report.")
    );
}
