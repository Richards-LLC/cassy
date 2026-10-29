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

fn assert_markdownlint_clean(repo_root: &Path, files: &[&Path]) {
    assert_markdownlint_clean_with_python(repo_root, files, "python3");
}

/// Docs Lint over generated Markdown through the repository's one pinned
/// linter (`scripts/markdownlint.py`, the root devDependency; cas-a1b0), not an
/// `npx` download or a PATH lookup.
fn assert_markdownlint_clean_with_python(repo_root: &Path, files: &[&Path], python: &str) {
    let mut command = ProcessCommand::new(python);
    command.arg(repo_root.join("scripts/markdownlint.py")).args([
        "--config",
        repo_root.join(".markdownlint-cli2.jsonc").to_str().unwrap(),
    ]);
    for file in files {
        command.arg(file);
    }
    let output = match command.output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "SKIP: generated Markdown Docs Lint looked for `{python}` on PATH \
                 (scripts/markdownlint.py); `{python}` is unavailable"
            );
            return;
        }
        Err(error) => panic!("could not run generated Markdown Docs Lint via `{python}`: {error}"),
    };
    if output.status.code() == Some(3) {
        println!(
            "SKIP: the pinned markdownlint-cli2 is unavailable (npm missing): {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    assert!(
        output.status.success(),
        "generated Markdown failed Docs Lint (command stderr):\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(unix)]
#[test]
fn generated_docs_lint_skips_when_python_is_missing() {
    let repo_root = cas::test_paths::workspace_root();
    assert_markdownlint_clean_with_python(&repo_root, &[], "cas-test-missing-python");
}

#[cfg(unix)]
fn install_fake_gh(project: &TempDir) -> PathBuf {
    install_fake_gh_version(project, "3.19.0")
}

#[cfg(unix)]
fn install_fake_gh_version(project: &TempDir, version: &str) -> PathBuf {

    let path = project.path().join("fake-gh");
    cas::test_paths::warm_stub(
        &path,
        &r##"#!/bin/sh
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
"##.replace("3.19.0", version)
    );
    path
}

#[cfg(unix)]
fn install_followup_fake_gh(project: &TempDir) -> PathBuf {

    let path = project.path().join("fake-gh-followup");
    cas::test_paths::warm_stub(
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
    );
    path
}

#[cfg(unix)]
#[test]
fn cli_release_report_renders_fixture_html_and_preserves_source() {
    let project = TempDir::new().unwrap();
    for args in [
        vec!["init", "-q", "-b", "feature/report"],
        vec!["remote", "add", "origin", "git@github.com:example/project.git"],
        vec!["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/master"],
    ] {
        assert!(ProcessCommand::new("git")
            .current_dir(project.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    }
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
    assert_eq!(first["github_repo"], "example/project");
    // The report wears `cas deliverable brand`: no DESIGN.md here, so the defaults.
    assert_eq!(first["brand_source"], "defaults");

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
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("| Area | Changes | What changed |"));
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("| Sessions | 1 | Worker report improvements |"));
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("| Releases | 1 | Simpler install |"));
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("| GitHub issues closed | 2 |"));
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("[#705](https://github.com/example/project/issues/705)"));
    assert!(source.contains("sha256:abc123"));
    assert!(html.contains("<html lang=\"en\">"));
    assert!(html.contains("--bg:#F7F4EE") && !html.contains("/*cas:brand*/"), "{html}");
    assert!(html.contains("data-change-map=\"1\""));
    assert!(html.contains("data-area=\"Sessions\""));
    assert!(html.contains("2 GitHub issues closed with this release"));
    assert!(html.contains("https://github.com/example/project/issues/746"));
    for extension in ["md", "pdf"] {
        assert!(html.contains(&format!(
            "href=\"https://github.com/example/project/blob/master/docs/release-reports/v3.19.0.{extension}\""
        )), "{html}");
    }

    // One source exercises verified Pair: lookup alongside explicit captures.
    // Put evidence under the configured project namespace, not the checkout.
    use sha2::{Digest, Sha256};
    let cas_root = project.path().join(".cas");
    let mut config = cas::config::Config::load(&cas_root).unwrap();
    config.set("factory.artifacts_root", project.path().join("artifacts").to_str().unwrap()).unwrap();
    config.save(&cas_root).unwrap();
    let pair_dir = cas::deliverable::pairs::task_dir(&config, &cas_root, "cas-report");
    fs::create_dir_all(&pair_dir).unwrap();
    let png = include_bytes!("../../hub-web/public/favicon-16.png");
    let side = |name: &str| {
        fs::write(pair_dir.join(format!("{name}.png")), png).unwrap();
        serde_json::json!({"png":format!("{name}.png"),"jpg":format!("{name}.png"),
            "sha256":format!("{:x}",Sha256::digest(png)),
            "jpg_sha256":format!("{:x}",Sha256::digest(png)),
            "ref":"fixture","source":"fixture","captured_at":"2026-10-26T00:00:00Z"})
    };
    let before = side("before");
    let after = side("after");
    fs::write(pair_dir.join("pairs.json"), serde_json::to_vec(&serde_json::json!({
        "schema":1,"task_id":"cas-report","base_sha":"base","head_sha":"head","origin":"fixture",
        "pairs":[{"id":"roster","surface":"drivers","viewport":[1280,800],"scheme":"light","before":before,"after":after}],
        "unpaired":[{"id":"missing","surface":"drivers","viewport":[1280,800],"scheme":"light","after":after}]
    })).unwrap()).unwrap();
    fs::write(project.path().join("before.png"), png).unwrap();
    fs::write(project.path().join("after.png"), png).unwrap();
    let source = source.replace("\n## Under the hood", "\n#### Paired change\n\nWas: earlier.\n\nNow: shipped.\n\nPair: cas-report/roster\n\nSource: capture receipt.\n\n#### Explicit change\n\nWas: earlier.\n\nNow: shipped.\n\nBefore: before.png\n\nAfter: after.png\n\n#### Unpaired change\n\nWas: earlier.\n\nNow: shipped.\n\nPair: cas-report/missing\n\n#### Unavailable change\n\nWas: earlier.\n\nNow: shipped.\n\nPair: cas-absent/roster\n\n## Under the hood");
    fs::write(&source_path, &source).unwrap();
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
    assert_eq!(second["change_count"], 2);
    assert_eq!(second["issue_count"], 2);
    assert_eq!(fs::read_to_string(&source_path).unwrap(), source);
    let html = fs::read_to_string(&html_path).unwrap();
    assert_eq!(html.matches("class=\"captures\"").count(), 2, "{html}");
    for title in ["Paired change", "Explicit change"] {
        assert!(html.contains(&format!("alt=\"Before: {title}\"")), "{html}");
        assert!(html.contains(&format!("alt=\"After: {title}\"")), "{html}");
    }
    for title in ["Unpaired change", "Unavailable change"] {
        assert!(!html.contains(&format!("alt=\"After: {title}\"")), "{html}");
    }
    assert!(!html.contains("Pair: cas-"), "capture metadata leaked into Now text");
    assert!(html.contains("Source: capture receipt."));
}

/// A managed renderer installed before link overrides must not reject a new
/// binary's arguments. Exercise selection and rendering through the CLI.
#[cfg(unix)]
#[test]
fn cli_release_report_falls_back_from_renderer_without_link_arguments_cas_692a() {
    let project = TempDir::new().unwrap();
    let output = project.path().join("reports");
    fs::create_dir_all(&output).unwrap();
    // The exemplar names a home path in its evidence; internal hygiene
    // (`cas deliverable finalize`) now refuses that, so the copy drops it.
    let exemplar = fs::read_to_string(
        cas::test_paths::workspace_root().join("docs/release-reports/2026-09-08-v3.19.0.md"),
    )
    .unwrap()
    .replace("/home/pippenz/", "~/");
    fs::write(output.join("v3.19.0.md"), exemplar).unwrap();
    let installed = project.path().join(".claude/skills/cas-release-report/scripts");
    fs::create_dir_all(&installed).unwrap();
    fs::write(installed.join("render.py"), r#"
# Previous managed renderer supports these source formats:
# front matter needs a closing
# ['area', 'changes', 'what changed']
import argparse
parser = argparse.ArgumentParser()
parser.add_argument('source')
parser.add_argument('--output', required=True)
parser.add_argument('--project-root')
args = parser.parse_args()
raise RuntimeError('obsolete renderer must be replaced by builtin')
"#)
    .unwrap();
    cas_cmd(&project)
        .args(["--json", "release", "report", "3.19.0", "--out", "reports"])
        .assert()
        .success();
    let html = fs::read_to_string(output.join("v3.19.0.html")).unwrap();
    assert!(html.contains("data-change-map="), "{html}");
    assert!(html.contains("Markdown source"), "{html}");
}

/// cas-7860: the release PR's issues are closed by hand minutes after it
/// merges. A report run that starts before those closures must wait inside
/// the counting window and count them, not print 0.
#[cfg(unix)]
#[test]
fn cli_release_report_counts_issues_closed_after_the_run_starts() {
    let project = TempDir::new().unwrap();
    fs::write(
        project.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [3.19.0] - 2026-09-08\n\n\
         ### Fixed — replies thread between supervisors and workers (#801)\n\n- Detail.\n\n\
         ### Fixed — reminders wake an idle supervisor (#802)\n\n- Detail.\n\n\
         ### Fixed — a parked delivery merges after the worker moves on (#700 follow-up)\n\n- Detail.\n",
    )
    .unwrap();
    cas_cmd(&project).args(["init", "--yes"]).assert().success();
    cas_cmd(&project)
        .args(["config", "set", "issues.repo", "example/project"])
        .assert()
        .success();

    let stamp = |offset: i64| {
        (chrono::Utc::now() + chrono::Duration::seconds(offset))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };
    let (created, merged, closed) = (stamp(-600), stamp(-10), stamp(20));
    let issue = |number: u64, state: &str, closed_at: &str| {
        format!(
            r#"{{"number":{number},"title":"Issue {number}","url":"https://github.com/example/project/issues/{number}","state":"{state}","closedAt":{closed_at},"labels":[],"body":""}}"#
        )
    };
    let open = format!(
        "[{},{},{}]",
        issue(801, "OPEN", "null"),
        issue(802, "OPEN", "null"),
        issue(700, "CLOSED", "\"2026-09-01T00:00:00Z\"")
    );
    let closed_now = format!(
        "[{},{},{}]",
        issue(801, "CLOSED", &format!("\"{closed}\"")),
        issue(802, "CLOSED", &format!("\"{closed}\"")),
        issue(700, "CLOSED", "\"2026-09-01T00:00:00Z\"")
    );
    let reads = project.path().join("issue-reads");
    let fake_gh = project.path().join("fake-gh-late-closures");
    cas::test_paths::warm_stub(
        &fake_gh,
        &format!(
            r#"#!/bin/sh
case "$1 $2" in
  "release view")
    printf '%s' '{{"name":"v3.19.0","url":"https://github.com/example/project/releases/tag/v3.19.0","body":"","publishedAt":"{merged}","isDraft":false,"assets":[]}}'
    ;;
  "pr list")
    printf '%s' '[{{"number":901,"title":"Release v3.19.0","body":"Release PR","url":"https://github.com/example/project/pull/901","closingIssuesReferences":[],"createdAt":"{created}","mergedAt":"{merged}"}}]'
    ;;
  "issue list")
    if [ -f '{reads}' ]; then printf '%s' '{closed_now}'; else : > '{reads}'; printf '%s' '{open}'; fi
    ;;
  *) exit 1 ;;
esac
"#,
            reads = reads.display()
        ),
    );

    // A standalone receipt lives under the artifact root, outside the default
    // `release/` directory. Its absolute input path must not leak into report
    // source, where finalize's internal hygiene rejects home paths.
    let home = TempDir::new_in(std::env::var_os("HOME").map(PathBuf::from).unwrap()).unwrap();
    let xdg = project.path().join(".test-xdg-config");
    let run_dir = home.path().join(".cas/artifacts/cas-fixture");
    fs::create_dir_all(&run_dir).unwrap();
    let receipt = run_dir.join("release-latency.receipt");
    fs::write(
        &receipt,
        "TAG=v3.19.0\nPUBLISH_START_KIND=gate-green\nGATE_GREEN_EPOCH=1788972265\n\
         PUBLISHED_AT=2026-09-09T16:49:25Z\nPUBLISH_LATENCY_SECONDS=300\nBUDGET_SECONDS=600\n\
         WITHIN_BUDGET=true\n",
    )
    .unwrap();

    let output = cas_cmd_at(project.path(), home.path(), &xdg)
        .env("GH_BIN", &fake_gh)
        .env("CAS_RELEASE_REPORT_POLL_SECS", "1")
        .args(["--json", "release", "report", "3.19.0", "--out", "docs/release-reports"])
        .arg("--latency-receipt")
        .arg(&receipt)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let result: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["green_to_published_seconds"], 300, "{result}");
    // cas-1a83: Evidence URLs are links in the HTML, not escaped `<…>` text.
    let html = fs::read_to_string(project.path().join("docs/release-reports/v3.19.0.html")).unwrap();
    assert!(
        html.contains(r#"<a href="https://github.com/example/project/pull/901">"#),
        "the release PR must render as a link"
    );
    assert!(!html.contains("&lt;https://"), "an Evidence URL rendered as text");
    assert_eq!(result["issue_count"], 2, "{result}");
    assert_eq!(result["change_count"], 3);
    let source = fs::read_to_string(project.path().join("docs/release-reports/v3.19.0.md")).unwrap();
    assert!(
        source.contains(
            "Release receipt (relative to `~/.cas/artifacts`): `cas-fixture/release-latency.receipt`."
        ),
        "{source}"
    );
    assert!(!source.contains(home.path().to_str().unwrap()), "{source}");
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("| GitHub issues closed | 2 |"), "{source}");
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("[#801](https://github.com/example/project/issues/801)"));
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("[#802](https://github.com/example/project/issues/802)"));
    assert!(
        // pin: the ledger names out-of-window closures instead of counting them
        source.contains("closed outside this release's counting window, so not counted: [#700]"),
        "{source}"
    );
    // pin: page one ends where the renderer's fixed "What you can do now" section starts
    let page_one = &source[..source.find("## What you can do now").unwrap()];
    for zero in ["| 0 |", "no listed issues", "issue-bearing", "verified closure"] {
        assert!(!page_one.contains(zero), "page one shows {zero:?}:\n{page_one}");
    }
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
    assert_eq!(result["change_count"], 1);
    assert_eq!(
        result["change_areas"]
            .as_array()
            .unwrap()
            .iter()
            .map(|area| area["area"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["Tasks"]
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
    // pin: the operator reads Eastern 12-hour times (cas-5389)
    assert!(source.contains("Published 9 September 2026 · 12:51 PM ET"));
    assert!(!source.contains("UTC"), "{source}");
    // pin: front matter stays a machine date in UTC (cas-5389)
    assert!(source.contains("date: 2026-09-09\n"));
    assert!(source.contains("The release is ready to inspect."));
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("areas: [\"Tasks\"]"));
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("| Tasks | 1 |"));
    // pin: generated Markdown rows are render.py's input contract (cas-7860)
    assert!(source.contains("[#767](https://github.com/example/project/issues/767)"));
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

#[cfg(unix)]
#[test]
fn generated_release_report_and_receipts_draft_pass_docs_lint() {
    let project = TempDir::new().unwrap();
    fs::write(
        project.path().join("CHANGELOG.md"),
        "# Changelog\n\n## [3.19.0] - 2026-09-08\n\n### Added\n- A lint-clean release report (#767)\n",
    )
    .unwrap();
    fs::create_dir_all(project.path().join("docs/release-notes")).unwrap();
    fs::write(
        project.path().join("docs/release-notes/v3.19.0.md"),
        r#"# v3.19.0 release notes

User top-level

```text
*Live on production — User — Cassy v3.19.0*
Was: reports were hard to lint. Now: the generated report is clean.
```

User reply

```text
**Only reply:**

• *Release report* — Was: reports were hard to lint. → Now: the report is clean.
```

Dev top-level

```text
*Live on production — Dev — Cassy v3.19.0*
Was: receipts needed hand edits. Now: the generator emits valid Markdown.
```

Dev reply

```text
**Only reply:**

• *Receipt template* — Was: links were bare. → Now: links are wrapped.
```
"#,
    )
    .unwrap();

    cas_cmd(&project).args(["init", "--yes"]).assert().success();
    cas_cmd(&project)
        .args(["config", "set", "issues.repo", "example/project"])
        .assert()
        .success();
    let fake_gh = install_followup_fake_gh(&project);
    cas_cmd(&project)
        .env("GH_BIN", &fake_gh)
        .args([
            "release",
            "report",
            "3.19.0",
            "--out",
            "docs/release-reports",
        ])
        .assert()
        .success();

    let repo_root = cas::test_paths::workspace_root();
    let receipt_dir = repo_root.join("scripts/release-train.d");
    let receipt_script = receipt_dir.join("receipts.sh");
    if !receipt_dir.is_dir() || !receipt_script.is_file() {
        println!(
            "SKIP generated_release_report_and_receipts_draft_pass_docs_lint: source checkout \
             is absent; looked for {}",
            receipt_script.display()
        );
        return;
    }
    let receipt_run_dir = project.path().join("receipt-run");
    fs::create_dir_all(&receipt_run_dir).unwrap();
    fs::write(
        receipt_run_dir.join("announce.receipt"),
        "POSTED_AT=2099-01-02T00:00:00Z\n\
         CHANNEL=cas-internal\n\
         CHANNEL_ID=C01234567\n\
         USER_TOP_LEVEL_ID=user-1\n\
         USER_TOP_LEVEL_PART_IDS=[\"user-1\"]\n\
         USER_TOP_LEVEL_PERMALINK=https://example.test/user-1\n\
         USER_REPLY_ID=user-2\n\
         USER_REPLY_PART_IDS=[\"user-2a\",\"user-2\"]\n\
         USER_REPLY_PERMALINK=https://example.test/user-2\n\
         DEV_TOP_LEVEL_ID=dev-1\n\
         DEV_TOP_LEVEL_PART_IDS=[\"dev-1\"]\n\
         DEV_TOP_LEVEL_PERMALINK=https://example.test/dev-1\n\
         DEV_REPLY_ID=dev-2\n\
         DEV_REPLY_PART_IDS=[\"dev-2\"]\n\
         DEV_REPLY_PERMALINK=https://example.test/dev-2\n",
    )
    .unwrap();
    let receipt_block = ProcessCommand::new("bash")
        .args([
            "-c",
            r#"
source "$1"
run_dir="$2"
release_train_announce_posted_block
"#,
            "receipt-block",
            receipt_script.to_str().unwrap(),
            receipt_run_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        receipt_block.status.success(),
        "receipt template failed: {}",
        String::from_utf8_lossy(&receipt_block.stderr)
    );
    let receipt_text = String::from_utf8(receipt_block.stdout).unwrap();

    let announce_path = project.path().join("generated-announce.md");
    fs::write(
        &announce_path,
        format!(
            "# Slack draft — fixture\n\n## User thread\n\n{}\n",
            receipt_text
        ),
    )
    .unwrap();
    let report_path = project.path().join("docs/release-reports/v3.19.0.md");
    let report = fs::read_to_string(&report_path).unwrap();
    assert!(report.contains("### Only reply (user)"));
    assert!(report.contains("### Only reply (dev)"));
    assert!(!report.contains("### Only reply:"));
    assert!(
        report
            .contains("GitHub release: <https://github.com/example/project/releases/tag/v3.19.0>")
    );
    assert!(receipt_text.contains("<https://example.test/user-1>"));
    // pin: the POSTED block shows the operator's Eastern 12-hour time (cas-5389)
    assert!(receipt_text.contains("1 January 2099 · 7:00 PM ET"), "{receipt_text}");
    assert!(!receipt_text.contains("UTC"), "{receipt_text}");
    assert_markdownlint_clean(&repo_root, &[&report_path, &announce_path]);
}


#[cfg(unix)]
#[test]
fn cli_release_report_counts_grouped_387_entries_and_preserves_issues_cas_711a() {
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join("docs/release-notes")).unwrap();
    fs::write(project.path().join("docs/release-notes/v9.99.1.md"), "# Release notes\n\nCloses #705, #746.\n").unwrap();
    cas_cmd(&project).args(["init", "--yes"]).assert().success();
    let mut config = cas::config::Config::load(&project.path().join(".cas")).unwrap();
    config.set("issues.repo", "example/project").unwrap();
    config.save(&project.path().join(".cas")).unwrap();
    let fake_gh = install_fake_gh_version(&project, "9.99.1");
    // A captured release input keeps the historical shape stable after version bumps.
    let grouped = include_str!("fixtures/changelog_grouped_387.md");
    assert_eq!(grouped.lines().filter(|line| line.starts_with("### ")).count(), 7);
    assert_eq!(grouped.lines().filter(|line| line.starts_with("- **")).count(), 27);
    let legacy = "## [9.99.1] - 2026-10-28\n\n### Fixed — factory reminder probes (#705)\n- External probes run apart from the daemon loop.\n  - **Details** stay part of this single change.\n\n### Changed — task coordination (#746)\n- A refusal lists every missing requirement.\n";
    for (body, expected) in [(grouped, 27), (legacy, 2)] {
        fs::write(project.path().join("CHANGELOG.md"), format!("# Changelog\n\n{body}")).unwrap();
        let output = cas_cmd(&project)
            .env("GH_BIN", &fake_gh)
            .args(["--json", "release", "report", "9.99.1", "--refresh-sources"])
            .assert().success().get_output().stdout.clone();
        let report: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(report["change_count"], expected, "{report}");
        assert_eq!(report["issue_count"], 2, "official GitHub count is independent: {report}");
        let html = fs::read_to_string(project.path().join("docs/release-reports/v9.99.1.html")).unwrap();
        assert_eq!(html.matches("data-change=\"").count(), expected, "one rendered mark per shipped change");
    }
}
