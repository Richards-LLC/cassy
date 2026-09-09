use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn cas_cmd(project: &TempDir) -> Command {
    let mut command = Command::new(cas::test_paths::cas_binary());
    let home = project.path().join(".test-home");
    let xdg = project.path().join(".test-xdg-config");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&xdg).unwrap();
    command
        .current_dir(project.path())
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
