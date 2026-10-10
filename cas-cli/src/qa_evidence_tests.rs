use super::*;
use std::io::Write;

/// A scratch repo with one delivery commit and a task artifacts dir.
struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    task_dir: PathBuf,
    head: String,
}

const TASK: &str = "cas-test";

fn git_ok(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Commit with a committer date `secs_ago` seconds in the past.
fn commit(repo: &Path, name: &str, secs_ago: i64) -> String {
    std::fs::write(repo.join(name), name).unwrap();
    git_ok(repo, &["add", name]);
    let date = format!("@{} +0000", chrono::Utc::now().timestamp() - secs_ago);
    let output = Command::new("git")
        .args(["commit", "-q", "-m", name])
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .unwrap();
    assert!(output.status.success());
    git_ok(repo, &["rev-parse", "HEAD"])
}

fn trace_zip(path: &Path, events: &[&str]) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file("test.trace", options).unwrap();
    writer.write_all(events.join("\n").as_bytes()).unwrap();
    writer.finish().unwrap();
}

const PASSING: [&str; 4] = [
    r#"{"type":"before","callId":"expect@1","method":"expect","title":"Expect \"toHaveText\""}"#,
    r#"{"type":"after","callId":"expect@1","endTime":2}"#,
    r#"{"type":"before","callId":"pw:api@2","method":"pw:api","title":"Click"}"#,
    r#"{"type":"after","callId":"pw:api@2","endTime":3}"#,
];

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q"]);
        let head = commit(&repo, "app.ts", 120);
        let task_dir = tmp.path().join("artifacts").join(TASK);
        std::fs::create_dir_all(&task_dir).unwrap();
        Self {
            _tmp: tmp,
            repo,
            task_dir,
            head,
        }
    }

    fn bundle_dir(&self) -> PathBuf {
        self.task_dir.join("qa")
    }

    /// Write a complete, valid bundle and return its manifest path.
    fn write_bundle(&self, edit: impl FnOnce(&mut serde_json::Value)) -> PathBuf {
        let dir = self.bundle_dir();
        std::fs::create_dir_all(dir.join("visual-qa")).unwrap();
        trace_zip(&dir.join("trace.zip"), &PASSING);
        let files = [
            (
                "trace-actions.txt",
                "   1. 0:00.1  Expect \"toHaveText\"   2ms\n",
            ),
            ("receipt.webm", "webm"),
            ("final.aria.yml", "- heading \"x\""),
            ("final.aria.json", "{}"),
            ("M01.png", "png"),
            ("visual-qa/app-light-desktop.png", "png"),
            ("visual-qa/app-light-phone.png", "png"),
            ("visual-qa/app-dark-desktop.png", "png"),
            ("visual-qa/app-dark-phone.png", "png"),
            ("visual-qa/visual-qa.md", "# Visual QA — PASS\n"),
            ("visual-qa.stdout", "rendered 4\nPASS\n"),
            (
                "critique.md",
                "| Dimension | Score |\nScored by t on 2026-09-23\n",
            ),
            ("a11y-forced-colors.png", "png"),
            ("a11y-reduced-motion.png", "png"),
            ("a11y-contrast-more.png", "png"),
        ];
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        write_visual_qa_report(&dir, "PASS", chrono::Utc::now(), "http://127.0.0.1:4173/");
        let mut manifest = serde_json::json!({
            "schema": 1,
            "task_id": TASK,
            "producer": "cas-qa-craft",
            "head_sha": self.head,
            "build_url": "http://127.0.0.1:4173/",
            "playwright_version": "1.63.0",
            "created_at": chrono::Utc::now().to_rfc3339(),
            "visual_change": true,
            "visual_qa_status": "pass",
            "files": {
                "trace": "trace.zip",
                "trace_actions": "trace-actions.txt",
                "receipt": "receipt.webm",
                "aria_yaml": "final.aria.yml",
                "aria_json": "final.aria.json",
                "cells": ["M01.png"],
                "a11y": ["a11y-forced-colors.png", "a11y-reduced-motion.png", "a11y-contrast-more.png"],
                "polish_screenshots": [
                    "visual-qa/app-light-desktop.png", "visual-qa/app-light-phone.png",
                    "visual-qa/app-dark-desktop.png", "visual-qa/app-dark-phone.png"
                ],
                "visual_qa": "visual-qa/visual-qa.md",
                "visual_qa_json": "visual-qa/visual-qa.json",
                "visual_qa_stdout": "visual-qa.stdout",
                "critique": "critique.md"
            },
            "critique_score": {"distinctiveness": 4, "fit": 4, "hierarchy": 4, "craft": 4, "accessibility": 5}
        });
        edit(&mut manifest);
        let path = dir.join("bundle.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        path
    }

    fn notes(&self) -> String {
        format!(
            "[2026-09-23 17:00] 🧪 PLATFORM_PROOF qa-bundle: {}/qa/bundle.json",
            self.task_dir.display()
        )
    }

    fn validate(&self, notes: &str) -> Result<BundleReceipt, EvidenceRefusal> {
        validate_bundle(&EvidenceContext {
            task_id: TASK,
            task_artifacts_dir: &self.task_dir,
            repo: &self.repo,
            delivered_head: &self.head,
            notes,
            deployed_origins: &[],
        })
    }
}

/// The report `scripts/visual-qa.mjs --strict` writes for one run.
fn write_visual_qa_report(
    bundle_dir: &Path,
    status: &str,
    generated: chrono::DateTime<chrono::Utc>,
    url: &str,
) {
    std::fs::write(
        bundle_dir.join("visual-qa/visual-qa.json"),
        serde_json::json!({
            "status": status,
            "strict": true,
            "generatedAt": generated.to_rfc3339(),
            "urls": [url],
            "findings": []
        })
        .to_string(),
    )
    .unwrap();
}

fn set_mtime_secs_ago(path: &Path, secs_ago: u64) {
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

#[test]
fn valid_bundle_passes() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let receipt = fx.validate(&fx.notes()).expect("valid bundle");
    assert_eq!(receipt.head_sha, fx.head);
    assert_eq!(receipt.passed_expects, 1);
}

fn scoped_factory_fixture() -> (Fixture, PathBuf, PathBuf) {
    let mut fx = Fixture::new();
    let cas_root = fx.repo.join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    let base = fx.task_dir.parent().unwrap().to_path_buf();
    std::fs::write(cas_root.join("config.toml"), format!(
        "[factory]\nartifacts_root = {:?}\n[qa]\nuser_facing_paths = [\"web/**\"]\n",
        base.display().to_string()
    )).unwrap();
    let [scoped, legacy] = crate::config::factory_task_artifact_dirs(&cas_root, &base, TASK);
    std::fs::remove_dir(&fx.task_dir).unwrap();
    fx.task_dir = scoped.clone();
    (fx, cas_root, legacy)
}

fn hook_bundle_write(fx: &Fixture, cas_root: &Path, bundle: &Path) -> serde_json::Value {
    let input = cas_core::hooks::types::HookInput {
        session_id: "qa-path-contract".into(),
        cwd: fx.repo.display().to_string(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Write".into()),
        tool_input: Some(serde_json::json!({"file_path": bundle, "content": "bundle"})),
        ..Default::default()
    };
    let out = crate::hooks::handlers::handle_pre_tool_use(&input, Some(&cas_root)).unwrap();
    serde_json::to_value(out.hook_specific_output.unwrap()).unwrap()
}

fn scoped_close(fx: &Fixture, cas_root: &Path, notes: &str) -> Result<Vec<String>, String> {
    let mut task = crate::types::Task::new(TASK.into(), "QA path agreement".into());
    task.notes = notes.into();
    crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::qa_evidence_close_gate_for_paths(
        cas_root, &task, &fx.repo, "", Some(&fx.head), Some(&["web/app.css".into()])
    )
}

/// GH #1061: a long-running close service can retain a flat-path citation
/// after PreToolUse has switched new writes to the project namespace.
#[test]
fn scoped_hook_bundle_satisfies_close_with_either_citation_gh_1061() {
    let mut env = crate::test_support::TestEnvGuard::new();
    env.set("CAS_AGENT_ROLE", "worker");
    let (fx, cas_root, legacy) = scoped_factory_fixture();
    let bundle = fx.task_dir.join("qa/bundle.json");
    let decision = hook_bundle_write(&fx, &cas_root, &bundle);
    assert_eq!(decision["permissionDecision"], "allow", "{decision}");
    fx.write_bundle(|_| {});
    assert!(!legacy.exists(), "no flat directory exists in the reported incident");
    for legacy_directory_exists in [false, true] {
        if legacy_directory_exists {
            std::fs::create_dir_all(&legacy).unwrap();
            std::fs::write(legacy.join("LEDGER.md"), "historical ledger").unwrap();
        }
        for citation in [bundle.clone(), legacy.join("qa/bundle.json")] {
            let notes = scoped_close(&fx, &cas_root, &format!("qa-bundle: {}", citation.display()))
                .unwrap_or_else(|error| panic!("citation {}: {error}", citation.display()));
            assert!(notes.iter().any(|note| note.contains(bundle.to_str().unwrap())
                && note.contains("1 passing Expect")), "{notes:?}");
        }
    }
}

#[test]
fn missing_bundle_refusal_requests_hook_writable_path_gh_1061() {
    let mut env = crate::test_support::TestEnvGuard::new();
    env.set("CAS_AGENT_ROLE", "worker");
    let (fx, cas_root, legacy) = scoped_factory_fixture();
    let bundle = fx.task_dir.join("qa/bundle.json");
    for notes in [String::new(), format!("qa-bundle: {}", legacy.join("qa/bundle.json").display())] {
        let error = scoped_close(&fx, &cas_root, &notes).unwrap_err();
        let (_, next) = error.split_once("Next: ").unwrap();
        assert!(next.contains(bundle.to_str().unwrap()), "{error}");
        assert!(!next.contains(legacy.to_str().unwrap()), "{error}");
        assert_eq!(hook_bundle_write(&fx, &cas_root, &bundle)["permissionDecision"], "allow");
    }
    let denied = hook_bundle_write(&fx, &cas_root, &legacy.join("qa/bundle.json"));
    assert_eq!(denied["permissionDecision"], "deny", "{denied}");
    assert!(denied["permissionDecisionReason"].as_str().unwrap()
        .contains(fx.task_dir.parent().unwrap().to_str().unwrap()), "{denied}");
}

#[test]
fn citation_migration_preserves_validation_boundaries_gh_1061() {
    let mut env = crate::test_support::TestEnvGuard::new();
    env.set("CAS_AGENT_ROLE", "worker");
    let (fx, cas_root, legacy) = scoped_factory_fixture();
    let bundle = fx.write_bundle(|_| {});
    let historical = legacy.join("qa/bundle.json");
    std::fs::create_dir_all(historical.parent().unwrap()).unwrap();
    std::fs::write(&historical, "invalid historical manifest").unwrap();
    let error = scoped_close(&fx, &cas_root, &format!("qa-bundle: {}", historical.display())).unwrap_err();
    assert!(error.contains("malformed") && error.contains(historical.to_str().unwrap()), "{error}");
    assert!(error.split_once("Next: ").unwrap().1.contains("rewrite bundle.json"), "{error}");
    let next = error.split_once("Next: ").unwrap().1;
    assert!(next.contains(bundle.to_str().unwrap()), "{error}");
    assert!(!next.contains(historical.to_str().unwrap()), "{error}");
    assert_eq!(hook_bundle_write(&fx, &cas_root, &bundle)["permissionDecision"], "allow");

    // A valid bundle elsewhere cannot be substituted for a cited task file.
    let unrelated = legacy.parent().unwrap().join("other-project/qa/bundle.json");
    std::fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
    std::fs::copy(&bundle, &unrelated).unwrap();
    for citation in [unrelated.clone(), legacy.join("../other-project/qa/bundle.json")] {
        let error = scoped_close(&fx, &cas_root, &format!("qa-bundle: {}", citation.display())).unwrap_err();
        assert!(error.contains("outside the task"), "{error}");
    }
    // Remapping still enforces the delivered commit, rather than trusting the
    // presence of a scoped manifest alone.
    std::fs::remove_file(&historical).unwrap();
    fx.write_bundle(|manifest| manifest["head_sha"] = serde_json::json!("0".repeat(40)));
    let error = scoped_close(&fx, &cas_root, &format!("qa-bundle: {}", historical.display())).unwrap_err();
    assert!(error.contains("stale"), "{error}");
    #[cfg(unix)]
    {
        std::fs::remove_file(&bundle).unwrap();
        std::os::unix::fs::symlink(&unrelated, &bundle).unwrap();
        let error = scoped_close(&fx, &cas_root, &format!("qa-bundle: {}", historical.display())).unwrap_err();
        assert!(error.contains("outside the task"), "{error}");
        // A dangling historical symlink is an existing unsafe citation,
        // never permission to switch to a different bundle.
        std::os::unix::fs::symlink(legacy.join("absent.json"), &historical).unwrap();
        let error = scoped_close(&fx, &cas_root, &format!("qa-bundle: {}", historical.display())).unwrap_err();
        assert!(error.contains("does not exist") && error.contains(historical.to_str().unwrap()), "{error}");
    }
}

#[test]
fn missing_citation_names_the_note_command() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let refusal = fx.validate("no citation here").unwrap_err();
    assert!(refusal.problem.contains("not cited"), "{refusal:?}");
    assert!(
        refusal.command.contains("note_type=platform_proof"),
        "{refusal:?}"
    );
    assert!(refusal.command.contains("qa-bundle:"), "{refusal:?}");
}

#[test]
fn missing_bundle_json_is_rejected() {
    let fx = Fixture::new();
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("does not exist"), "{refusal:?}");
}

#[test]
fn each_missing_or_empty_required_file_is_named_with_its_command() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    std::fs::remove_file(fx.bundle_dir().join("trace-actions.txt")).unwrap();
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(
        refusal.problem.contains("files.trace_actions"),
        "{refusal:?}"
    );
    assert!(
        refusal.command.contains("npx playwright trace actions"),
        "{refusal:?}"
    );

    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    std::fs::write(fx.bundle_dir().join("receipt.webm"), "").unwrap();
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(
        refusal.problem.contains("files.receipt") && refusal.problem.contains("empty"),
        "{refusal:?}"
    );

    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["files"]
            .as_object_mut()
            .unwrap()
            .remove("critique");
    });
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("files.critique"), "{refusal:?}");
}

#[test]
fn polish_renders_must_cover_light_dark_desktop_phone() {
    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["files"]["polish_screenshots"] =
            serde_json::json!(["visual-qa/app-light-desktop.png"]);
    });
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("-light-phone.png"), "{refusal:?}");
    assert!(
        refusal.command.contains("visual-qa.mjs --strict"),
        "{refusal:?}"
    );
}

#[test]
fn stale_head_sha_is_rejected_after_a_later_commit() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let later = Fixture {
        head: commit(&fx.repo, "fix.ts", 60),
        ..fx
    };
    let refusal = later.validate(&later.notes()).unwrap_err();
    assert!(refusal.problem.starts_with("stale"), "{refusal:?}");
    assert!(refusal.command.contains(&later.head[..8]), "{refusal:?}");
}

#[test]
fn stale_file_mtime_is_rejected() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    set_mtime_secs_ago(&fx.bundle_dir().join("M01.png"), 3600);
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(
        refusal.problem.contains("stale") && refusal.problem.contains("cells[0]"),
        "{refusal:?}"
    );
}

#[test]
fn stale_created_at_is_rejected() {
    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["created_at"] = serde_json::json!("2020-01-01T00:00:00Z");
    });
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("created_at"), "{refusal:?}");
}

#[test]
fn descendant_head_sha_covers_the_delivery() {
    let fx = Fixture::new();
    let descendant = commit(&fx.repo, "merge-tip.ts", 30);
    fx.write_bundle(|manifest| manifest["head_sha"] = serde_json::json!(descendant));
    fx.validate(&fx.notes())
        .expect("descendant build covers the delivery");
}

#[test]
fn failing_or_assertion_free_trace_is_rejected() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let mut events = PASSING.to_vec();
    events.push(r#"{"type":"before","callId":"expect@9","method":"expect","title":"Expect \"toBeVisible\""}"#);
    events.push(r#"{"type":"after","callId":"expect@9","error":{"message":"not found"}}"#);
    trace_zip(&fx.bundle_dir().join("trace.zip"), &events);
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("failing run"), "{refusal:?}");

    trace_zip(&fx.bundle_dir().join("trace.zip"), &PASSING[2..]);
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("no passing Expect"), "{refusal:?}");

    std::fs::write(fx.bundle_dir().join("trace.zip"), "not a zip").unwrap();
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("not a zip"), "{refusal:?}");
}

/// cas-b10d: a trace zip with these entries, stored uncompressed.
fn entries_zip(path: &Path, entries: &[(&str, String)]) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(body.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}

/// What `visual-qa.mjs --scrub-trace` writes for a signed-in runner zip's
/// network record and evaluate argument (cas-b10d, captured from the scrubber).
const SCRUBBED_NETWORK: &str = r#"{"type":"resource-snapshot","snapshot":{"request":{"url":"/account","headers":[{"name":"Authorization","value":"[REDACTED]"},{"name":"Cookie","value":"[REDACTED]"}],"cookies":[]},"response":{"headers":[{"name":"Set-Cookie","value":"[REDACTED]"}]}}}"#;
const SCRUBBED_EVALUATE: &str = r#"{"type":"before","callId":"call@3","method":"evaluate","params":{"arg":"{\"refreshToken\":\"[REDACTED]\",\"idToken\":\"[REDACTED]\"}"}}"#;

/// cas-b10d: the cas-qa-craft signed-in recipe keeps the runner's tracing on
/// and scrubs the finished zip. Its output keeps the runner's test.trace, so
/// the gate counts real assertions, and every credential reads [REDACTED]:
/// the authenticated (deployed) bundle passes the credential scan.
#[test]
fn a_scrubbed_runner_trace_from_the_signed_in_recipe_is_accepted_cas_b10d() {
    let fx = Fixture::new();
    fx.write_deployed_bundle(|_| {});
    entries_zip(
        &fx.bundle_dir().join("trace.zip"),
        &[
            ("test.trace", PASSING.join("\n")),
            ("trace.trace", SCRUBBED_EVALUATE.to_string()),
            ("trace.network", SCRUBBED_NETWORK.to_string()),
        ],
    );
    let receipt = fx
        .validate_with_origins(&staging())
        .expect("the recipe's scrubbed runner trace is accepted");
    assert_eq!(receipt.passed_expects, 1);
}

/// cas-b10d: what the old recipe's `saveQaTrace` wrote: a library
/// `context.tracing` zip whose protocol calls include assertions, but with
/// no runner test.trace and so no test outcome. It stays refused, and the
/// refusal names the runner recipe and its scrub step.
#[test]
fn a_library_context_trace_is_refused_with_the_runner_recipe_cas_b10d() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    entries_zip(
        &fx.bundle_dir().join("trace.zip"),
        &[
            ("trace.trace", PASSING.join("\n")),
            ("trace.network", SCRUBBED_NETWORK.to_string()),
        ],
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("has no test.trace"), "{}", refusal.problem);
    assert!(refusal.problem.contains("library context.tracing"), "{}", refusal.problem);
    assert!(refusal.command.contains("Playwright test runner"), "{}", refusal.command);
    assert!(refusal.command.contains("--scrub-trace"), "{}", refusal.command);
}

/// cas-b10d: skipping the recipe's scrub leaves the runner's recorded
/// session cookie in the network log; the signed-in bundle is refused by name.
#[test]
fn an_unscrubbed_signed_in_runner_trace_is_refused_cas_b10d() {
    let fx = Fixture::new();
    fx.write_deployed_bundle(|_| {});
    let raw_network = SCRUBBED_NETWORK.replacen(
        r#"{"name":"Cookie","value":"[REDACTED]"}"#,
        r#"{"name":"Cookie","value":"__session=synthetic-session-cookie-value"}"#,
        1,
    );
    entries_zip(
        &fx.bundle_dir().join("trace.zip"),
        &[("test.trace", PASSING.join("\n")), ("trace.network", raw_network)],
    );
    let refusal = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(refusal.problem.contains("carrying credentials"), "{}", refusal.problem);
    assert!(refusal.problem.contains("trace.network"), "{}", refusal.problem);
}

#[test]
fn passing_poll_and_to_pass_ignore_caught_inner_expect_retries_gh1013() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let events = [
        r#"{"type":"before","callId":"poll@1","method":"expect","title":"Expect \"poll toBe\""}"#,
        r#"{"type":"before","callId":"retry@1","parentId":"poll@1","method":"expect","title":"Expect \"toBe\""}"#,
        r#"{"type":"after","callId":"retry@1","error":{"message":"not ready"}}"#,
        r#"{"type":"before","callId":"retry@2","parentId":"poll@1","method":"expect","title":"Expect \"toBe\""}"#,
        r#"{"type":"after","callId":"retry@2"}"#,
        r#"{"type":"after","callId":"poll@1"}"#,
        r#"{"type":"before","callId":"toPass@1","method":"expect","title":"wait for audit"}"#,
        r#"{"type":"before","callId":"callback@1","parentId":"toPass@1","method":"test.step","title":"poll callback"}"#,
        r#"{"type":"before","callId":"retry@3","parentId":"callback@1","method":"expect","title":"Expect \"toContain\""}"#,
        r#"{"type":"after","callId":"retry@3","error":{"message":"waiting"}}"#,
        r#"{"type":"after","callId":"callback@1"}"#,
        r#"{"type":"after","callId":"toPass@1"}"#,
    ];
    trace_zip(&fx.bundle_dir().join("trace.zip"), &events);
    assert_eq!(trace_expect_summary(&fx.bundle_dir().join("trace.zip")).unwrap(), ExpectSummary { passed: 2, failed: 0 });
    fx.validate(&fx.notes()).expect("the final poll outcomes passed");

    let mut terminal_failure = events.to_vec();
    terminal_failure[5] = r#"{"type":"after","callId":"poll@1","error":{"message":"poll timed out"}}"#;
    trace_zip(&fx.bundle_dir().join("trace.zip"), &terminal_failure);
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("failing run"), "{refusal:?}");
}

#[test]
fn polish_proof_and_critique_floor_are_enforced() {
    let fx = Fixture::new();
    fx.write_bundle(|manifest| manifest["visual_qa_status"] = serde_json::json!("unavailable"));
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("unavailable"), "{refusal:?}");

    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    write_visual_qa_report(
        &fx.bundle_dir(),
        "FAIL",
        chrono::Utc::now(),
        "http://127.0.0.1:4173/",
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("status \"FAIL\""), "{refusal:?}");

    let fx = Fixture::new();
    fx.write_bundle(|manifest| manifest["critique_score"]["fit"] = serde_json::json!(3));
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("fit=3"), "{refusal:?}");

    let fx = Fixture::new();
    fx.write_bundle(|manifest| manifest["critique_score"]["craft"] = serde_json::json!(0));
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("craft=0"), "{refusal:?}");
}

#[test]
fn visual_change_requires_all_three_a11y_modes() {
    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["files"]["a11y"] = serde_json::json!(["a11y-forced-colors.png"])
    });
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("reduced-motion"), "{refusal:?}");

    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["visual_change"] = serde_json::json!(false);
        manifest["files"]["a11y"] = serde_json::json!([]);
    });
    fx.validate(&fx.notes())
        .expect("non-visual change needs no a11y captures");
}

#[test]
fn citations_outside_the_task_or_from_an_independent_round_are_rejected() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let outside = fx.task_dir.parent().unwrap().join("other");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::copy(
        fx.bundle_dir().join("bundle.json"),
        outside.join("bundle.json"),
    )
    .unwrap();
    let refusal = fx
        .validate(&format!("qa-bundle: {}/bundle.json", outside.display()))
        .unwrap_err();
    assert!(refusal.problem.contains("outside the task"), "{refusal:?}");

    #[cfg(unix)]
    {
        let link = fx.task_dir.join("escape");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let refusal = fx
            .validate(&format!("qa-bundle: {}/bundle.json", link.display()))
            .unwrap_err();
        assert!(refusal.problem.contains("outside the task"), "{refusal:?}");
    }

    let round = fx.task_dir.join("independent-qa/round-1");
    std::fs::create_dir_all(&round).unwrap();
    std::fs::copy(
        fx.bundle_dir().join("bundle.json"),
        round.join("bundle.json"),
    )
    .unwrap();
    let refusal = fx
        .validate(&format!("qa-bundle: {}/bundle.json", round.display()))
        .unwrap_err();
    assert!(
        refusal.problem.contains("not cited"),
        "a reviewer's round is never read as the implementer's bundle: {refusal:?}"
    );
}

#[test]
fn listed_files_may_not_escape_the_bundle() {
    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["files"]["receipt"] = serde_json::json!("../../../repo/app.ts")
    });
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("escaping"), "{refusal:?}");
}

#[test]
fn newest_citation_wins() {
    let notes = "qa-bundle: /a/old/bundle.json\n[later] qa-bundle: `/a/new/bundle.json`.\n\
                 [reviewer] qa-bundle: /a/independent-qa/round-1/bundle.json";
    assert_eq!(
        cited_bundle_path(notes).as_deref(),
        Some("/a/new/bundle.json"),
        "a later independent round citation must not shadow the implementer's bundle"
    );
    assert_eq!(cited_bundle_path("nothing"), None);
}

#[test]
fn ledger_tier_requires_a_fresh_pass_row() {
    let fx = Fixture::new();
    let ctx = EvidenceContext {
        task_id: TASK,
        task_artifacts_dir: &fx.task_dir,
        repo: &fx.repo,
        delivered_head: &fx.head,
        notes: "",
        deployed_origins: &[],
    };
    assert!(
        validate_ledger(&ctx)
            .unwrap_err()
            .problem
            .contains("missing")
    );
    let ledger = fx.task_dir.join("LEDGER.md");
    std::fs::write(&ledger, "| id | cell | expected | observed | verdict | label | evidence | defect |\n| M01 | cli | ok | ok | NOT EXERCISED | fixture | - | - |\n").unwrap();
    assert!(
        validate_ledger(&ctx)
            .unwrap_err()
            .problem
            .contains("no row with verdict PASS")
    );
    std::fs::write(
        &ledger,
        "| M01 | cli | ok | ok | PASS | real-build | qa/M01.txt | - |\n",
    )
    .unwrap();
    validate_ledger(&ctx).expect("fresh pipe-form PASS row");
    std::fs::write(
        &ledger,
        "M01 | cli | ok | ok | PASS | real-build | qa/M01.txt | -\n",
    )
    .unwrap();
    validate_ledger(&ctx).expect("fresh doc-style PASS row");
    set_mtime_secs_ago(&ledger, 3600);
    assert!(
        validate_ledger(&ctx)
            .unwrap_err()
            .problem
            .starts_with("stale")
    );
}

#[test]
fn trace_summary_counts_top_level_errors_as_failures() {
    let summary = expect_summary_from_events(
        &[
            PASSING[0],
            PASSING[1],
            r#"{"type":"error","message":"boom"}"#,
        ]
        .join("\n"),
    );
    assert_eq!(
        summary,
        ExpectSummary {
            passed: 1,
            failed: 1
        }
    );
}

const HEALER_DIFF: &str = r#"diff --git a/hub-web/e2e/generated/fleet.spec.ts b/hub-web/e2e/generated/fleet.spec.ts
--- a/hub-web/e2e/generated/fleet.spec.ts
+++ b/hub-web/e2e/generated/fleet.spec.ts
@@ -10,2 +10,4 @@ test.describe("Populated fleet", () => {
     const fleet = page.locator('[aria-label="Fleet"]');
+    // The table marks quiet-marten as Needs you, but the verdict disagrees.
+    test.fixme(true, "Fleet verdict disagrees with its work-state table");
     await expect(fleet).toContainText("2 machines");
diff --git a/hub-web/e2e/webkit.spec.ts b/hub-web/e2e/webkit.spec.ts
--- a/hub-web/e2e/webkit.spec.ts
+++ b/hub-web/e2e/webkit.spec.ts
@@ -0,0 +1,3 @@
+// cas-allow-skip: WebKit lacks the clipboard permission this test needs
+test.skip(browserName === "webkit", "no clipboard");
+test.only("focused", async () => {});
diff --git a/cas-cli/src/lib.rs b/cas-cli/src/lib.rs
--- a/cas-cli/src/lib.rs
+++ b/cas-cli/src/lib.rs
@@ -1,1 +1,2 @@
+// test.skip( in Rust source is not a JS test file
"#;

#[test]
fn healer_fixme_is_found_and_allowed_skips_carry_their_reason() {
    let markers = added_skip_markers(HEALER_DIFF);
    assert_eq!(markers.len(), 3, "{markers:?}");
    assert_eq!(
        markers[0],
        SkipMarker {
            file: "hub-web/e2e/generated/fleet.spec.ts".into(),
            line: 12,
            marker: "test.fixme(",
            allowed: None,
        }
    );
    assert_eq!(markers[1].marker, "test.skip(");
    assert_eq!(markers[1].line, 2);
    assert_eq!(
        markers[1].allowed.as_deref(),
        Some("WebKit lacks the clipboard permission this test needs")
    );
    assert_eq!(markers[2].marker, "test.only(");
    assert_eq!(
        markers[2].allowed, None,
        "the allow reason covers only the next line"
    );
}

#[test]
fn skip_marker_matching_respects_word_boundaries_and_file_kinds() {
    assert_eq!(marker_in("process.exit(1)"), None);
    assert_eq!(marker_in("unit.skip(x)"), None);
    assert_eq!(marker_in("  xit('pending', () => {})"), Some("xit("));
    assert_eq!(
        marker_in("test.describe.skip('group')"),
        Some("test.describe.skip(")
    );
    assert!(is_js_test_file("hub-web/e2e/a.ts"));
    assert!(is_js_test_file("src/button.test.tsx"));
    assert!(!is_js_test_file("hub-web/src/main.ts"));
    assert!(!is_js_test_file("tests/cli_test.rs"));
}

#[test]
fn delivery_test_diff_reads_only_js_test_files() {
    let fx = Fixture::new();
    let base = fx.head.clone();
    std::fs::create_dir_all(fx.repo.join("e2e")).unwrap();
    std::fs::write(fx.repo.join("e2e/a.spec.ts"), "test.fixme(true, 'x');\n").unwrap();
    std::fs::write(fx.repo.join("notes.md"), "test.skip(\n").unwrap();
    git_ok(&fx.repo, &["add", "."]);
    git_ok(
        &fx.repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "heal",
        ],
    );
    let head = git_ok(&fx.repo, &["rev-parse", "HEAD"]);
    let diff = delivery_test_diff(
        &fx.repo,
        &base,
        &head,
        &["e2e/a.spec.ts".into(), "notes.md".into()],
    )
    .unwrap();
    let markers = added_skip_markers(&diff);
    assert_eq!(markers.len(), 1, "{diff}");
    assert_eq!(markers[0].file, "e2e/a.spec.ts");
    assert_eq!(
        delivery_test_diff(&fx.repo, &base, &head, &["notes.md".into()]).as_deref(),
        Some("")
    );
}

#[test]
fn close_gate_rejects_unexplained_markers_before_evidence() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let notes = fx.notes();
    let ctx = EvidenceContext {
        task_id: TASK,
        task_artifacts_dir: &fx.task_dir,
        repo: &fx.repo,
        delivered_head: &fx.head,
        notes: &notes,
        deployed_origins: &[],
    };
    let markers = added_skip_markers(HEALER_DIFF);
    let error = run_close_gate(&ctx, EvidenceTier::None, &[], &markers).unwrap_err();
    assert!(
        error.starts_with("TASK CLOSE REJECTED: cas-test adds test skip/focus markers"),
        "{error}"
    );
    assert!(
        error.contains("hub-web/e2e/generated/fleet.spec.ts:12 `test.fixme`"),
        "{error}"
    );
    assert!(
        error.contains("hub-web/e2e/webkit.spec.ts:3 `test.only`"),
        "{error}"
    );
    assert!(
        !error.contains("webkit.spec.ts:2"),
        "allowed skip must not be listed: {error}"
    );
    assert!(error.contains(ALLOW_SKIP), "{error}");

    let allowed_only: Vec<SkipMarker> = markers
        .into_iter()
        .filter(|marker| marker.allowed.is_some())
        .collect();
    let pass = run_close_gate(
        &ctx,
        EvidenceTier::Bundle,
        &["demo_statement".into()],
        &allowed_only,
    )
    .unwrap();
    assert!(
        pass.notes
            .iter()
            .any(|note| note.contains("Allowed skip marker hub-web/e2e/webkit.spec.ts:2")),
        "{pass:?}"
    );
    assert!(
        pass.notes
            .iter()
            .any(|note| note.starts_with("QA evidence bundle accepted")),
        "{pass:?}"
    );
}

#[test]
fn close_gate_message_names_reasons_problem_and_next_command() {
    let fx = Fixture::new();
    let ctx = EvidenceContext {
        task_id: TASK,
        task_artifacts_dir: &fx.task_dir,
        repo: &fx.repo,
        delivered_head: &fx.head,
        notes: "",
        deployed_origins: &[],
    };
    let error = run_close_gate(
        &ctx,
        EvidenceTier::Bundle,
        &["journeys:J03".into(), "demo_statement".into()],
        &[],
    )
    .unwrap_err();
    assert!(
        error.starts_with("TASK CLOSE REJECTED: cas-test is user-facing (journeys:J03; demo_statement) and its QA evidence bundle is not cited"),
        "{error}"
    );
    assert!(error.contains("Next: produce the bundle under"), "{error}");
    assert!(error.contains(CONTRACT_REFERENCE), "{error}");
    let error = run_close_gate(
        &ctx,
        EvidenceTier::Ledger { terminal_qa: false },
        &["demo_statement".into()],
        &[],
    )
    .unwrap_err();
    assert!(error.contains("QA evidence ledger is missing"), "{error}");
    assert!(
        run_close_gate(&ctx, EvidenceTier::None, &[], &[])
            .unwrap()
            .notes
            .is_empty()
    );
}

#[test]
fn delivery_range_before_and_after_merge() {
    let fx = Fixture::new();
    let base = fx.head.clone();
    git_ok(&fx.repo, &["branch", "-M", "main"]);
    git_ok(&fx.repo, &["checkout", "-q", "-b", "factory/w"]);
    let tip = commit(&fx.repo, "ui.tsx", 30);
    let (from, to) = delivery_range(&fx.repo, &tip, "main").unwrap();
    assert_eq!((from.as_str(), to.as_str()), (base.as_str(), tip.as_str()));
    assert_eq!(
        range_paths(&fx.repo, &from, &to).unwrap(),
        vec!["ui.tsx".to_string()]
    );
    git_ok(&fx.repo, &["checkout", "-q", "main"]);
    commit(&fx.repo, "other.rs", 20);
    git_ok(
        &fx.repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merge",
            "factory/w",
        ],
    );
    let (from, to) = delivery_range(&fx.repo, &tip, "main").unwrap();
    assert!(from.ends_with("^1"), "{from}");
    assert_eq!(
        range_paths(&fx.repo, &from, &to).unwrap(),
        vec!["ui.tsx".to_string()]
    );
}

#[test]
fn qa_range_paths_ignore_deleted_html_css_and_whitespace_vue_gh_1027_1037() {
    let fx = Fixture::new();
    let base = fx.head.clone();
    commit(&fx.repo, "old.html", 60);
    commit(&fx.repo, "old.css", 50);
    std::fs::write(fx.repo.join("app.vue"), "<template><p>Hello</p></template>\n").unwrap();
    git_ok(&fx.repo, &["add", "app.vue"]);
    git_ok(&fx.repo, &["commit", "-qm", "initial vue"]);
    let before = git_ok(&fx.repo, &["rev-parse", "HEAD"]);
    std::fs::remove_file(fx.repo.join("old.html")).unwrap();
    std::fs::remove_file(fx.repo.join("old.css")).unwrap();
    std::fs::write(fx.repo.join("app.vue"), "<template> <p>Hello</p> </template>\n").unwrap();
    git_ok(&fx.repo, &["add", "-A"]);
    git_ok(&fx.repo, &["commit", "-qm", "remove and format"]);
    let after = git_ok(&fx.repo, &["rev-parse", "HEAD"]);
    assert!(range_paths(&fx.repo, &before, &after).unwrap().is_empty());
    assert_eq!(range_paths(&fx.repo, &base, &before).unwrap(), vec!["app.vue", "old.css", "old.html"]);
}

#[test]
fn journey_polish_exception_does_not_reach_the_delivery_close() {
    // Supervisor decision (cas-0cd5): a journey bundle without polish proof
    // is valid for the release evaluation but cannot close a delivery.
    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["producer"] = serde_json::json!("journey");
        manifest["visual_qa_status"] = serde_json::json!("unavailable");
        for key in [
            "polish_screenshots",
            "visual_qa",
            "visual_qa_json",
            "visual_qa_stdout",
            "critique",
        ] {
            manifest["files"].as_object_mut().unwrap().remove(key);
        }
    });
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(
        refusal
            .problem
            .contains("journey bundle without polish proof"),
        "{refusal:?}"
    );

    let fx = Fixture::new();
    fx.write_bundle(|manifest| manifest["producer"] = serde_json::json!("journey"));
    fx.validate(&fx.notes())
        .expect("a journey bundle with polish proof is accepted");
}

#[test]
fn ledger_pass_row_must_be_real_build() {
    let fx = Fixture::new();
    let ctx = EvidenceContext {
        task_id: TASK,
        task_artifacts_dir: &fx.task_dir,
        repo: &fx.repo,
        delivered_head: &fx.head,
        notes: "",
        deployed_origins: &[],
    };
    let ledger = fx.task_dir.join("LEDGER.md");
    std::fs::write(
        &ledger,
        "| M01 | cli | ok | ok | PASS | fixture | qa/M01.txt | - |\n",
    )
    .unwrap();
    let refusal = validate_ledger(&ctx).unwrap_err();
    assert!(refusal.problem.contains("label real-build"), "{refusal:?}");
}

#[test]
fn terminal_qa_receipt_must_pass_and_be_fresh() {
    let fx = Fixture::new();
    let ctx = EvidenceContext {
        task_id: TASK,
        task_artifacts_dir: &fx.task_dir,
        repo: &fx.repo,
        delivered_head: &fx.head,
        notes: "",
        deployed_origins: &[],
    };
    let refusal = validate_terminal_qa(&ctx).unwrap_err();
    assert!(refusal.problem.starts_with("missing"), "{refusal:?}");
    assert!(
        refusal.command.contains("scripts/terminal-qa.mjs"),
        "{refusal:?}"
    );

    let dir = fx.task_dir.join("terminal-qa/cas-status");
    std::fs::create_dir_all(&dir).unwrap();
    let report = dir.join("report.md");
    std::fs::write(&report, "terminal-qa: FAIL cas-status · 14 runs · 2 fail\n").unwrap();
    assert!(
        validate_terminal_qa(&ctx)
            .unwrap_err()
            .problem
            .starts_with("failing")
    );

    std::fs::write(
        &report,
        "terminal-qa: PASS cas-status · 14 runs · 0 fail · 0 warn\n",
    )
    .unwrap();
    assert_eq!(validate_terminal_qa(&ctx).unwrap(), report);

    set_mtime_secs_ago(&report, 3600);
    assert!(
        validate_terminal_qa(&ctx)
            .unwrap_err()
            .problem
            .starts_with("stale")
    );

    // The ledger tier composes both requirements.
    std::fs::write(
        fx.task_dir.join("LEDGER.md"),
        "| M01 | cas status | ok | ok | PASS | real-build | qa/M01.txt | - |\n",
    )
    .unwrap();
    let error = run_close_gate(
        &ctx,
        EvidenceTier::Ledger { terminal_qa: true },
        &["demo_statement".into()],
        &[],
    )
    .unwrap_err();
    assert!(error.contains("terminal-qa receipt is stale"), "{error}");
    std::fs::write(
        &report,
        "terminal-qa: PASS cas-status · 14 runs · 0 fail · 0 warn\n",
    )
    .unwrap();
    let pass = run_close_gate(
        &ctx,
        EvidenceTier::Ledger { terminal_qa: true },
        &["demo_statement".into()],
        &[],
    )
    .unwrap();
    assert!(
        pass.notes
            .iter()
            .any(|note| note.starts_with("terminal-qa receipt accepted")),
        "{pass:?}"
    );
}

// cas-a6a3 (GH #1007): a claimed visual-QA pass needs the strict run's own
// report, newer than the delivered commit, of a local build.

#[test]
fn visual_qa_pass_claim_without_the_run_report_is_refused() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    // The claim and the PASS lines are there, but the run never wrote its report.
    std::fs::write(fx.bundle_dir().join("visual-qa/visual-qa.json"), "{}").unwrap();
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(
        refusal.problem.contains("claims a visual-QA pass") && refusal.problem.contains("status"),
        "{refusal:?}"
    );
    assert!(
        refusal.command.contains("visual-qa.mjs --strict"),
        "{refusal:?}"
    );
    assert!(refusal.command.contains("local URL"), "{refusal:?}");
}

#[test]
fn visual_qa_run_against_a_production_origin_does_not_count() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    write_visual_qa_report(
        &fx.bundle_dir(),
        "PASS",
        chrono::Utc::now(),
        "https://hub.petrastella.io/commander/",
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(
        refusal.problem.contains("hub.petrastella.io")
            && refusal.problem.contains("not a local build"),
        "{refusal:?}"
    );
}

#[test]
fn visual_qa_run_older_than_the_delivered_commit_is_refused() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    // The delivered commit is 120 s old; this run is from an hour before it.
    write_visual_qa_report(
        &fx.bundle_dir(),
        "PASS",
        chrono::Utc::now() - chrono::Duration::hours(1),
        "http://127.0.0.1:4173/",
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(
        refusal.problem.contains("before the delivered commit"),
        "{refusal:?}"
    );
}

#[test]
fn visual_qa_run_that_failed_or_was_not_strict_is_refused() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    write_visual_qa_report(
        &fx.bundle_dir(),
        "FAIL",
        chrono::Utc::now(),
        "http://127.0.0.1:4173/",
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("status \"FAIL\""), "{refusal:?}");

    std::fs::write(
        fx.bundle_dir().join("visual-qa/visual-qa.json"),
        serde_json::json!({"status": "PASS", "strict": false, "generatedAt": chrono::Utc::now().to_rfc3339(), "urls": ["http://127.0.0.1:4173/"]}).to_string(),
    )
    .unwrap();
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("without --strict"), "{refusal:?}");
}

#[test]
fn visual_qa_local_runs_are_accepted() {
    for url in [
        "http://127.0.0.1:26361/commander/",
        "http://localhost:4173/",
        "http://app.localhost:4173/",
        "http://[::1]:4173/",
        "file:///tmp/dist/index.html",
    ] {
        let fx = Fixture::new();
        fx.write_bundle(|_| {});
        write_visual_qa_report(&fx.bundle_dir(), "PASS", chrono::Utc::now(), url);
        fx.validate(&fx.notes())
            .unwrap_or_else(|refusal| panic!("{url}: {refusal:?}"));
    }
}

#[test]
fn single_source_visual_qa_report_is_authority_over_text_markers_gh_1017_1025() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    std::fs::write(
        fx.bundle_dir().join("visual-qa/visual-qa.md"),
        "# Visual QA\n\nResult: **PASS**\n",
    )
    .unwrap();
    std::fs::write(
        fx.bundle_dir().join("visual-qa.stdout"),
        "PASS local-source: 0 issue(s) [no issues]\n",
    )
    .unwrap();
    let report = serde_json::json!({
        "generatedAt": chrono::Utc::now().to_rfc3339(),
        "input": "http://127.0.0.1:4173/",
        "strict": true,
        "totalIssues": 0,
        "validRenders": 1,
        "renders": [{"valid": true}]
    });
    let report_path = fx.bundle_dir().join("visual-qa/visual-qa.json");
    std::fs::write(&report_path, report.to_string()).unwrap();
    fx.validate(&fx.notes()).expect("real single-source PASS needs no hand-edited markers");

    for (field, value) in [
        ("totalIssues", serde_json::json!(1)),
        ("validRenders", serde_json::json!(0)),
        ("renders", serde_json::json!([{"valid": false}])),
        ("strict", serde_json::json!(false)),
        ("input", serde_json::json!("https://example.com/")),
    ] {
        let mut failed = report.clone();
        failed[field] = value;
        std::fs::write(&report_path, failed.to_string()).unwrap();
        assert!(fx.validate(&fx.notes()).is_err(), "{field} must invalidate the run");
    }
}

#[test]
fn local_origin_predicate() {
    for local in [
        "http://127.0.0.1:1/",
        "http://127.8.9.10/",
        "https://localhost/",
        "http://0.0.0.0:3000/",
        "http://[::1]/",
        "file:///x.html",
        // visual-qa.mjs opens a target without a scheme as a local file.
        "/tmp/dist/index.html",
    ] {
        assert!(is_local_origin(local), "{local}");
    }
    for remote in [
        "https://hub.petrastella.io/",
        "https://cas-hub-static-abc-richards-llc.vercel.app/commander/",
        "http://192.168.1.20:4173/",
        "http://localhost.evil.com/",
        "",
        "http://",
        "ftp://127.0.0.1/",
    ] {
        assert!(!is_local_origin(remote), "{remote}");
    }
}

/// cas-e371 (GH #1023 finding 1): a visual-QA report with its findings list.
fn scoped_report(
    path: &Path,
    status: &str,
    generated: chrono::DateTime<chrono::Utc>,
    urls: &[&str],
    findings: serde_json::Value,
) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        serde_json::json!({
            "status": status,
            "strict": true,
            "generatedAt": generated.to_rfc3339(),
            "urls": urls,
            "findings": findings
        })
        .to_string(),
    )
    .unwrap();
}

fn backlog_finding(origin: &str, selector: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "insufficient-contrast",
        "selector": selector,
        "elementPath": selector,
        "url": format!("{origin}/?fixture=home"),
        "scheme": "dark",
        "viewport": {"name": "desktop", "width": 1280, "height": 800}
    })
}

const TIP: &str = "http://127.0.0.1:4173";
const BASE: &str = "http://127.0.0.1:4174";

/// A bundle claiming `scoped`, with a delivered-build run (one backlog finding)
/// and a base-build run over the same page on another port.
fn scoped_fixture() -> Fixture {
    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["visual_qa_status"] = serde_json::json!("scoped");
        manifest["files"]["visual_qa_baseline_json"] =
            serde_json::json!("visual-qa-baseline/visual-qa.json");
    });
    let dir = fx.bundle_dir();
    scoped_report(
        &dir.join("visual-qa/visual-qa.json"),
        "FAIL",
        chrono::Utc::now(),
        &[&format!("{TIP}/?fixture=home")],
        serde_json::json!([backlog_finding(TIP, "footer a")]),
    );
    scoped_report(
        &dir.join("visual-qa-baseline/visual-qa.json"),
        "FAIL",
        chrono::Utc::now() - chrono::Duration::days(3),
        &[&format!("{BASE}/?fixture=home")],
        serde_json::json!([
            backlog_finding(BASE, "footer a"),
            backlog_finding(BASE, "header nav a")
        ]),
    );
    fx
}

#[test]
fn scoped_visual_qa_accepts_a_page_backlog_the_delivery_did_not_add() {
    let fx = scoped_fixture();
    fx.validate(&fx.notes())
        .expect("a finding the base build already has does not block a narrow delivery");
}

#[test]
fn scoped_visual_qa_refuses_a_finding_the_delivery_introduced() {
    let fx = scoped_fixture();
    scoped_report(
        &fx.bundle_dir().join("visual-qa/visual-qa.json"),
        "FAIL",
        chrono::Utc::now(),
        &[&format!("{TIP}/?fixture=home")],
        serde_json::json!([
            backlog_finding(TIP, "footer a"),
            backlog_finding(TIP, "#send")
        ]),
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(
        refusal.problem.contains("introduced 1 visual-QA finding") && refusal.problem.contains("#send"),
        "{refusal:?}"
    );
    assert!(refusal.command.contains("merge-base"), "{refusal:?}");

    // The same finding twice on the tip, once on the base: one is new.
    scoped_report(
        &fx.bundle_dir().join("visual-qa/visual-qa.json"),
        "FAIL",
        chrono::Utc::now(),
        &[&format!("{TIP}/?fixture=home")],
        serde_json::json!([
            backlog_finding(TIP, "footer a"),
            backlog_finding(TIP, "footer a")
        ]),
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("introduced 1"), "{refusal:?}");
}

#[test]
fn scoped_visual_qa_needs_a_comparable_local_base_run() {
    // No baseline key.
    let fx = scoped_fixture();
    fx.write_bundle(|manifest| manifest["visual_qa_status"] = serde_json::json!("scoped"));
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("visual_qa_baseline_json"), "{refusal:?}");

    // A base run that did not check the delivered page.
    let fx = scoped_fixture();
    scoped_report(
        &fx.bundle_dir().join("visual-qa-baseline/visual-qa.json"),
        "FAIL",
        chrono::Utc::now(),
        &[&format!("{BASE}/?fixture=other")],
        serde_json::json!([backlog_finding(BASE, "footer a")]),
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("did not check /?fixture=home"), "{refusal:?}");

    // A base run against a deployed site.
    let fx = scoped_fixture();
    scoped_report(
        &fx.bundle_dir().join("visual-qa-baseline/visual-qa.json"),
        "FAIL",
        chrono::Utc::now(),
        &["https://staging.example.com/?fixture=home"],
        serde_json::json!([backlog_finding("https://staging.example.com", "footer a")]),
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("not a local build"), "{refusal:?}");

    // A delivered-build run from before the delivered commit.
    let fx = scoped_fixture();
    scoped_report(
        &fx.bundle_dir().join("visual-qa/visual-qa.json"),
        "FAIL",
        chrono::Utc::now() - chrono::Duration::hours(1),
        &[&format!("{TIP}/?fixture=home")],
        serde_json::json!([backlog_finding(TIP, "footer a")]),
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("before the delivered commit"), "{refusal:?}");

    // Reports without a findings list cannot be compared.
    let fx = scoped_fixture();
    write_visual_qa_report(&fx.bundle_dir(), "FAIL", chrono::Utc::now(), "http://127.0.0.1:4173/");
    std::fs::write(
        fx.bundle_dir().join("visual-qa/visual-qa.json"),
        serde_json::json!({
            "status": "FAIL", "strict": true,
            "generatedAt": chrono::Utc::now().to_rfc3339(),
            "urls": [format!("{TIP}/?fixture=home")]
        })
        .to_string(),
    )
    .unwrap();
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("`findings` list"), "{refusal:?}");
}

// ---------------------------------------------------------------------------
// cas-a6ab (GH #1023 finding 4): a deployed authenticated staging run stands
// in for a local build when local auth is impossible.
// ---------------------------------------------------------------------------

const STAGING: &str = "https://staging.gabber.studio";
const CORS_REASON: &str = "staging backend CORS rejects the localhost origin on /auth/session, so /creator-profile has no local authenticated build";
/// Clerk-shaped session JWT; never allowed into a bundle.
const SESSION_JWT: &str =
    "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ1c2VyXzEyMzQ1Njc4OSJ9.c2lnbmF0dXJlLXNpZ25hdHVyZS1zaWc";

impl Fixture {
    /// A complete deployed-origin bundle for the delivered head.
    fn write_deployed_bundle(&self, edit: impl FnOnce(&mut serde_json::Value)) -> PathBuf {
        let head = self.head.clone();
        let path = self.write_bundle(|manifest| {
            manifest["build_url"] = serde_json::json!(format!("{STAGING}/creator-profile"));
            manifest["deployed"] = serde_json::json!({
                "origin": STAGING,
                "reason": CORS_REASON,
                "deployed_sha": head,
                "deployment_proof": "deployment.json",
            });
            edit(manifest);
        });
        let dir = self.bundle_dir();
        std::fs::write(
            dir.join("deployment.json"),
            serde_json::json!({ "environment": "staging", "commit": self.head }).to_string(),
        )
        .unwrap();
        write_visual_qa_report(
            &dir,
            "PASS",
            chrono::Utc::now(),
            &format!("{STAGING}/creator-profile"),
        );
        path
    }

    fn validate_with_origins(&self, origins: &[String]) -> Result<BundleReceipt, EvidenceRefusal> {
        let notes = self.notes();
        validate_bundle(&EvidenceContext {
            task_id: TASK,
            task_artifacts_dir: &self.task_dir,
            repo: &self.repo,
            delivered_head: &self.head,
            notes: &notes,
            deployed_origins: origins,
        })
    }
}

fn staging() -> Vec<String> {
    vec![STAGING.to_string()]
}

#[test]
fn deployed_staging_bundle_with_reason_and_proof_satisfies_the_gate() {
    let fx = Fixture::new();
    fx.write_deployed_bundle(|_| {});
    let receipt = fx
        .validate_with_origins(&staging())
        .unwrap_or_else(|refusal| panic!("{refusal:?}"));
    assert_eq!(receipt.head_sha, fx.head);
    // The same run is not local, so without the declaration it is refused.
    let local_only = fx.validate_with_origins(&[]).unwrap_err();
    assert!(
        local_only.problem.contains("wrong origin"),
        "{local_only:?}"
    );
}

#[test]
fn deployed_bundle_without_a_reason_is_refused() {
    let fx = Fixture::new();
    fx.write_deployed_bundle(|manifest| manifest["deployed"]["reason"] = serde_json::json!(" "));
    let refusal = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(
        refusal.problem.contains("without its reason"),
        "{refusal:?}"
    );
}

#[test]
fn deployed_bundle_from_an_unconfigured_or_local_origin_is_refused() {
    let fx = Fixture::new();
    fx.write_deployed_bundle(|manifest| {
        manifest["deployed"]["origin"] = serde_json::json!("https://gabber.studio")
    });
    let wrong = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(
        wrong.problem.contains("wrong origin") && wrong.problem.contains("https://gabber.studio"),
        "{wrong:?}"
    );
    assert!(wrong.command.contains("qa.deployed_origins"), "{wrong:?}");

    let fx = Fixture::new();
    fx.write_deployed_bundle(|manifest| {
        manifest["deployed"]["origin"] = serde_json::json!("http://localhost:3000")
    });
    let local = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(local.problem.contains("names a local origin"), "{local:?}");

    // The visual-QA run itself must be on the declared origin.
    let fx = Fixture::new();
    fx.write_deployed_bundle(|_| {});
    write_visual_qa_report(
        &fx.bundle_dir(),
        "PASS",
        chrono::Utc::now(),
        "https://gabber.studio/",
    );
    let elsewhere = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(
        elsewhere.problem.contains("https://gabber.studio/"),
        "{elsewhere:?}"
    );
}

#[test]
fn deployed_bundle_for_another_commit_or_without_proof_is_refused() {
    let fx = Fixture::new();
    fx.write_deployed_bundle(|manifest| {
        manifest["deployed"]["deployed_sha"] = serde_json::json!("a".repeat(40))
    });
    let stale = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(
        stale.problem.starts_with("stale: the deployment served"),
        "{stale:?}"
    );

    let fx = Fixture::new();
    fx.write_deployed_bundle(|_| {});
    std::fs::write(
        fx.bundle_dir().join("deployment.json"),
        "{\"commit\":\"main\"}",
    )
    .unwrap();
    let unproven = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(unproven.problem.starts_with("unproven"), "{unproven:?}");

    let fx = Fixture::new();
    fx.write_deployed_bundle(|_| {});
    std::fs::remove_file(fx.bundle_dir().join("deployment.json")).unwrap();
    let missing = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(
        missing.problem.contains("deployed.deployment_proof")
            && missing.problem.contains("does not exist"),
        "{missing:?}"
    );
}

#[test]
fn secret_scan_accepts_exact_redaction_placeholders_in_each_shape() {
    for value in ["REDACTED", "[REDACTED]", "<redacted>", "***", ""] {
        let json_value = serde_json::to_string(value).unwrap();
        for text in [
            format!(r#"{{"name":"Authorization","value":{json_value}}}"#),
            format!("Cookie: {value}\n"),
            format!(r#"{{"cookies":[{{"name":"session","value":{json_value}}}]}}"#),
        ] {
            assert_eq!(first_secret(&text), None, "placeholder in {text}");
        }
    }
}

#[test]
fn secret_scan_refuses_real_values_in_each_shape() {
    for value in [
        "12345678",
        "REDACTED-real",
        "[REDACTED]suffix",
        "REDACTED real",
    ] {
        let json_value = serde_json::to_string(value).unwrap();
        for text in [
            format!(r#"{{"name":"Set-Cookie","value":{json_value}}}"#),
            format!("Authorization: {value}\n"),
            format!(r#"{{"cookies":[{{"value":{json_value}}}]}}"#),
        ] {
            assert!(first_secret(&text).is_some(), "real value in {text}");
        }
    }
}

#[test]
fn secret_scan_checks_every_value_next_to_redactions() {
    for text in [
        r#"[{"name":"Cookie","value":"REDACTED"},{"name":"Authorization","value":"12345678"}]"#,
        "Cookie: REDACTED\nAuthorization: 12345678\n",
        r#"{"cookies":[{"value":"12345678"},{"value":"[REDACTED]"}]}"#,
        r#"{"cookies":[{"value":"[REDACTED]"},{"value":"12345678"}]}"#,
    ] {
        assert!(first_secret(text).is_some(), "mixed values in {text}");
    }
}

#[test]
fn secret_scan_decodes_json_values_without_exempting_partial_redactions() {
    for text in [
        r#"{"name":"Cookie","value":"\u005bREDACTED\u005d"}"#,
        r#"{"cookies":[{"value":"\u005bREDACTED\u005d"}]}"#,
        "Cookie: \tREDACTED\t \r\n",
    ] {
        assert_eq!(first_secret(text), None, "exact value in {text}");
    }
    for text in [
        r#"{"name":"Cookie","value":" REDACTED "}"#,
        r#"{"cookies":[{"value":"escaped\"real-value"},{"value":"[REDACTED]"}]}"#,
    ] {
        assert!(first_secret(text).is_some(), "real value in {text}");
    }
}

#[test]
fn secret_scan_applies_redaction_rule_to_text_and_trace_without_echoing_values() {
    let dir = tempfile::tempdir().unwrap();
    let text_path = dir.path().join("actions.txt");
    let trace_path = dir.path().join("trace.zip");
    let listed = [
        ("trace_actions".to_string(), text_path.clone()),
        ("trace".to_string(), trace_path.clone()),
    ];
    for value in ["REDACTED", "[REDACTED]", "<redacted>", "***", ""] {
        let event = format!(r#"{{"name":"Authorization","value":"{value}"}}"#);
        std::fs::write(&text_path, format!("Cookie: {value}\n")).unwrap();
        trace_zip(&trace_path, &[&event]);
        assert!(check_no_secrets(dir.path(), &listed).is_ok());
    }
    for key in ["trace_actions", "trace"] {
        std::fs::write(&text_path, "Cookie: REDACTED\n").unwrap();
        trace_zip(&trace_path, &[r#"{"name":"Cookie","value":"REDACTED"}"#]);
        let real_value = "real-secret-123";
        if key == "trace_actions" {
            std::fs::write(&text_path, format!("Cookie: {real_value}\n")).unwrap();
        } else {
            let event = format!(r#"{{"name":"Cookie","value":"{real_value}"}}"#);
            trace_zip(&trace_path, &[&event]);
        }
        let refusal = check_no_secrets(dir.path(), &listed).unwrap_err();
        let message = format!("{refusal:?}");
        assert!(refusal.problem.contains(key), "{message}");
        assert!(message.contains("8 or more characters"), "{message}");
        for placeholder in ["REDACTED", "[REDACTED]", "<redacted>", "***", "empty"] {
            assert!(message.contains(placeholder), "{message}");
        }
        assert!(
            !message.contains(real_value),
            "the value must not be echoed"
        );
    }
}

#[test]
fn deployed_bundle_carrying_credentials_is_refused_without_echoing_them() {
    // A session cookie copied into a text artifact.
    let fx = Fixture::new();
    fx.write_deployed_bundle(|_| {});
    std::fs::write(
        fx.bundle_dir().join("trace-actions.txt"),
        format!("   1. 0:00.1  Expect \"toHaveText\"   2ms\nCookie: __session={SESSION_JWT}\n"),
    )
    .unwrap();
    let leaked = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(
        leaked.problem.contains("carrying credentials"),
        "{leaked:?}"
    );
    assert!(leaked.problem.contains("trace_actions"), "{leaked:?}");
    assert!(
        !format!("{leaked:?}").contains(SESSION_JWT),
        "the secret must not be echoed"
    );

    // An auth header Playwright recorded in the trace's network log.
    let fx = Fixture::new();
    fx.write_deployed_bundle(|_| {});
    let trace = fx.bundle_dir().join("trace.zip");
    let file = std::fs::File::create(&trace).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file("test.trace", options).unwrap();
    writer.write_all(PASSING.join("\n").as_bytes()).unwrap();
    writer.start_file("0-trace.network", options).unwrap();
    writer
        .write_all(
            format!(
                r#"{{"type":"resource-snapshot","snapshot":{{"request":{{"headers":[{{"name":"Authorization","value":"Bearer {SESSION_JWT}"}}]}}}}}}"#
            )
            .as_bytes(),
        )
        .unwrap();
    writer.finish().unwrap();
    let network = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(
        network.problem.contains("carrying credentials")
            && network.problem.contains("0-trace.network"),
        "{network:?}"
    );
    assert!(!format!("{network:?}").contains(SESSION_JWT));

    // A saved storage state anywhere in the bundle.
    let fx = Fixture::new();
    fx.write_deployed_bundle(|_| {});
    std::fs::write(fx.bundle_dir().join("staging-storage-state.json"), "{}").unwrap();
    let state = fx.validate_with_origins(&staging()).unwrap_err();
    assert!(state.problem.contains("storage state"), "{state:?}");
}

#[test]
fn deployed_origins_config_accepts_remote_origins_only() {
    assert_eq!(
        url_origin("https://Staging.Gabber.Studio/creator-profile?x=1").as_deref(),
        Some("https://staging.gabber.studio")
    );
    assert_eq!(url_origin("file:///tmp/x.html"), None);
    let mut config = crate::config::Config::default();
    config
        .set(
            "qa.deployed_origins",
            "https://staging.gabber.studio/path, https://preview.example.com",
        )
        .unwrap();
    assert_eq!(
        config.qa().deployed_origins,
        vec![
            "https://staging.gabber.studio",
            "https://preview.example.com"
        ]
    );
    assert!(
        config
            .set("qa.deployed_origins", "http://localhost:3000")
            .is_err()
    );
    assert!(config.set("qa.deployed_origins", "not a url").is_err());
}

/// cas-7c15 (GH #1078): a Quasar focus input, whose `f_<uuid>` id is minted
/// on every render.
fn quasar_focus_finding(origin: &str, uuid: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "tiny-target",
        "selector": format!("#f_{uuid}"),
        "elementPath": format!("div.q-field > input#f_{uuid}"),
        "otherElementPath": format!("label[for=f_{uuid}]"),
        "url": format!("{origin}/?fixture=home"),
        "scheme": "dark",
        "viewport": {"name": "desktop", "width": 1280, "height": 800}
    })
}

#[test]
fn normalize_random_ids_replaces_only_uuid_shaped_fragments_cas_7c15() {
    assert_eq!(
        normalize_random_ids("#f_4cedc84f-1b2a-4c3d-9e8f-0123456789ab"),
        "#f_<uuid>"
    );
    assert_eq!(
        normalize_random_ids("label[for=f_4CEDC84F-1B2A-4C3D-9E8F-0123456789AB] > x"),
        "label[for=f_<uuid>] > x"
    );
    assert_eq!(
        normalize_random_ids(
            "a#00000000-0000-0000-0000-000000000000 b#ffffffff-ffff-ffff-ffff-ffffffffffff"
        ),
        "a#<uuid> b#<uuid>"
    );
    // Not UUID-shaped: stable ids, short hex, glued hex, wrong grouping.
    for stable in [
        "#send",
        "#f_4cedc84f",
        "#f_a4cedc84f-1b2a-4c3d-9e8f-0123456789ab",
        "#f_4cedc84f-1b2a-4c3d-9e8f-0123456789abc",
        "#f_4cedc84f1b2a-4c3d-9e8f-0123456789ab",
        "#f_4cedc84g-1b2a-4c3d-9e8f-0123456789ab",
        "",
    ] {
        assert_eq!(normalize_random_ids(stable), stable, "{stable}");
    }
}

#[test]
fn scoped_visual_qa_ignores_per_render_random_ids_but_still_reports_new_findings_cas_7c15() {
    let fx = scoped_fixture();
    let dir = fx.bundle_dir();
    let tip_ids = [
        "4cedc84f-1b2a-4c3d-9e8f-0123456789ab",
        "9a8b7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c6d",
    ];
    let base_ids = [
        "0f1e2d3c-4b5a-4968-8776-655443322110",
        "AABBCCDD-EEFF-4011-8233-445566778899",
    ];
    scoped_report(
        &dir.join("visual-qa-baseline/visual-qa.json"),
        "FAIL",
        chrono::Utc::now() - chrono::Duration::days(3),
        &[&format!("{BASE}/?fixture=home")],
        serde_json::json!([
            backlog_finding(BASE, "footer a"),
            quasar_focus_finding(BASE, base_ids[0]),
            quasar_focus_finding(BASE, base_ids[1]),
        ]),
    );
    // Same findings, only the random ids differ: nothing is new.
    scoped_report(
        &dir.join("visual-qa/visual-qa.json"),
        "FAIL",
        chrono::Utc::now(),
        &[&format!("{TIP}/?fixture=home")],
        serde_json::json!([
            backlog_finding(TIP, "footer a"),
            quasar_focus_finding(TIP, tip_ids[0]),
            quasar_focus_finding(TIP, tip_ids[1]),
        ]),
    );
    fx.validate(&fx.notes())
        .expect("findings that differ only in per-render random ids are the base's backlog");

    // A genuinely new finding, and a third random-id input the base has only
    // two of, are still reported.
    scoped_report(
        &dir.join("visual-qa/visual-qa.json"),
        "FAIL",
        chrono::Utc::now(),
        &[&format!("{TIP}/?fixture=home")],
        serde_json::json!([
            backlog_finding(TIP, "footer a"),
            backlog_finding(TIP, "#send"),
            quasar_focus_finding(TIP, tip_ids[0]),
            quasar_focus_finding(TIP, tip_ids[1]),
            quasar_focus_finding(TIP, "11111111-2222-4333-8444-555555555555"),
        ]),
    );
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(
        refusal.problem.contains("introduced 2 visual-QA finding")
            && refusal.problem.contains("#send")
            && refusal.problem.contains("#f_<uuid>"),
        "{refusal:?}"
    );
}

// cas-f290: captured cas-a286 Atlas findings, including the ancestor class
// rename and the 0.02px width difference in the independently built reports.
fn cas_f290_atlas_finding(origin: &str, scheme: &str, base: bool) -> serde_json::Value {
    let class = if base {
        "os-dropped"
    } else {
        "codename-squeezed"
    };
    let element = format!(
        "div > div.conversation-shell.thread-open > main.conversation-main > header.conversation-heading.thead > div.conversation-identity:nth-of-type(2) > div.id > span.conversation-host > span.host-where.{class}:nth-of-type(1) > span.host-machine:nth-of-type(1)"
    );
    serde_json::json!({
        "type": "clipped-content", "reason": "text-bounds-exceed-overflow-ancestor",
        "selector": element, "elementPath": element, "ancestorPath": element,
        "textSample": "Atlas",
        "ancestorBox": {"x":94.0,"y":32.31,"width":if base {34.48} else {34.5},"height":14.38,"right":if base {128.48} else {128.5},"bottom":46.69},
        "url":format!("{origin}/commander/?conversation=1"), "scheme":scheme,
        "viewport":{"name":"phone","width":390,"height":844}
    })
}

fn cas_f290_reports(fx: &Fixture, tip: serde_json::Value, base: serde_json::Value) {
    for (origin, directory, findings) in
        [(TIP, "visual-qa", tip), (BASE, "visual-qa-baseline", base)]
    {
        scoped_report(
            &fx.bundle_dir().join(directory).join("visual-qa.json"),
            "FAIL",
            chrono::Utc::now(),
            &[&format!("{origin}/commander/?conversation=1")],
            findings,
        );
    }
}

#[test]
fn scoped_visual_qa_pairs_exact_cas_a286_renamed_atlas_findings_cas_f290() {
    let fx = scoped_fixture();
    cas_f290_reports(
        &fx,
        serde_json::json!([
            cas_f290_atlas_finding(TIP, "light", false),
            cas_f290_atlas_finding(TIP, "dark", false)
        ]),
        serde_json::json!([
            cas_f290_atlas_finding(BASE, "light", true),
            cas_f290_atlas_finding(BASE, "dark", true)
        ]),
    );
    fx.validate(&fx.notes())
        .expect("same Atlas clipping survives the ancestor class rename");
    // A fresh producer adds glyph bounds; the old producer has only the
    // clipping ancestor. Compare the shared ancestorBox, not different kinds.
    let mut enriched = cas_f290_atlas_finding(TIP, "light", false);
    enriched["textBounds"] = serde_json::json!({"x":94,"y":31.31,"width":34.5,"height":15});
    cas_f290_reports(
        &fx,
        serde_json::json!([enriched]),
        serde_json::json!([cas_f290_atlas_finding(BASE, "light", true)]),
    );
    fx.validate(&fx.notes())
        .expect("old and new producers pair shared bounds");
}

#[test]
fn scoped_visual_qa_keeps_new_renamed_findings_added_cas_f290() {
    let fx = scoped_fixture();
    let original = cas_f290_atlas_finding(TIP, "light", false);
    for (pointer, value) in [
        ("/type", serde_json::json!("contrast")),
        ("/reason", serde_json::json!("text-outside-scroll-range")),
        ("/textSample", serde_json::json!("Borealis")),
        ("/ancestorBox/x", serde_json::json!(300.0)),
        ("/ancestorBox/width", serde_json::json!(12.0)),
        ("/viewport/width", serde_json::json!(412)),
        ("/scheme", serde_json::json!("dark")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        cas_f290_reports(
            &fx,
            serde_json::json!([changed]),
            serde_json::json!([cas_f290_atlas_finding(BASE, "light", true)]),
        );
        let refusal = fx.validate(&fx.notes()).unwrap_err();
        assert!(
            refusal.problem.contains("introduced 1"),
            "{pointer}: {refusal:?}"
        );
    }
}

#[test]
fn scoped_visual_qa_matches_semantic_identity_but_preserves_rules_cas_f290() {
    let fx = scoped_fixture();
    let mut tip = cas_f290_atlas_finding(TIP, "light", false);
    let mut base = cas_f290_atlas_finding(BASE, "light", true);
    for finding in [&mut tip, &mut base] {
        finding.as_object_mut().unwrap().remove("ancestorBox");
        finding["role"] = serde_json::json!("button");
        finding["accessibleName"] = serde_json::json!("Send for review");
    }
    cas_f290_reports(
        &fx,
        serde_json::json!([tip.clone()]),
        serde_json::json!([base.clone()]),
    );
    fx.validate(&fx.notes())
        .expect("a renamed button retains role and accessible name");
    for (field, value) in [
        ("accessibleName", "Discard"),
        ("role", "link"),
        ("reason", "text-outside-scroll-range"),
    ] {
        let mut changed = tip.clone();
        changed[field] = serde_json::json!(value);
        cas_f290_reports(
            &fx,
            serde_json::json!([changed]),
            serde_json::json!([base.clone()]),
        );
        assert!(
            fx.validate(&fx.notes())
                .unwrap_err()
                .problem
                .contains("introduced 1")
        );
    }
}

#[test]
fn scoped_visual_qa_does_not_pair_renames_without_identity_cas_f290() {
    let fx = scoped_fixture();
    assert_ne!(
        visual_qa_selector(&backlog_finding(TIP, r#"button[aria-label="Open  log"]"#)),
        visual_qa_selector(&backlog_finding(BASE, r#"button[aria-label="Open log"]"#)),
        "legacy CSS identity preserves significant quoted spaces",
    );
    for invalid_bounds in [
        serde_json::Value::Null,
        serde_json::json!({"x":94,"y":32.31,"width":-1,"height":14.38}),
    ] {
        let mut tip = cas_f290_atlas_finding(TIP, "light", false);
        let mut base = cas_f290_atlas_finding(BASE, "light", true);
        tip["ancestorBox"] = invalid_bounds.clone();
        base["ancestorBox"] = invalid_bounds;
        cas_f290_reports(&fx, serde_json::json!([tip]), serde_json::json!([base]));
        assert!(
            fx.validate(&fx.notes())
                .unwrap_err()
                .problem
                .contains("introduced 1")
        );
    }
}

#[test]
fn scoped_visual_qa_uses_each_base_finding_once_without_order_bias_cas_f290() {
    let fx = scoped_fixture();
    let make = |origin, base, x| {
        let mut finding = cas_f290_atlas_finding(origin, "light", base);
        finding["ancestorBox"]["x"] = serde_json::json!(x);
        finding
    };
    // First head can pair with either base (within 0.5px); second only with first.
    // An arbitrary first-match loop incorrectly reports the second as added.
    let tip = serde_json::json!([make(TIP, false, 94.2), make(TIP, false, 93.8)]);
    let base = serde_json::json!([make(BASE, true, 94.0), make(BASE, true, 94.6)]);
    cas_f290_reports(&fx, tip.clone(), base);
    fx.validate(&fx.notes())
        .expect("one-to-one pairing finds the complete matching");
    cas_f290_reports(&fx, tip, serde_json::json!([make(BASE, true, 94.0)]));
    assert!(
        fx.validate(&fx.notes())
            .unwrap_err()
            .problem
            .contains("introduced 1")
    );
}

fn cas_5488_badge(origin: &str, index: usize, y: f64) -> serde_json::Value {
    serde_json::json!({
        "type": "contrast", "selector": format!(".error-feed > .badge:nth-of-type({index})"),
        "textSample": "Error", "textBounds": {"x":24,"y":y,"width":48,"height":16},
        "ratio": 3.1, "threshold": 4.5, "foreground": [128,128,128], "background": [255,255,255],
        "url": format!("{origin}/commander/?conversation=1"), "scheme": "light",
        "viewport": {"name":"phone","width":390,"height":844}
    })
}

#[test]
fn scoped_visual_qa_pairs_translated_badges_cas_5488() {
    let fx = scoped_fixture();
    let base: Vec<_> = (1..=20).map(|i| cas_5488_badge(BASE, i, 100.0 + i as f64 * 20.0)).collect();
    let tip: Vec<_> = (1..=20).map(|i| cas_5488_badge(TIP, i, 639.0 + i as f64 * 20.0)).collect();
    cas_f290_reports(&fx, serde_json::json!(tip), serde_json::json!(base));
    fx.validate(&fx.notes()).expect("twenty identical badges translated by 539px are existing findings");
}

#[test]
fn scoped_visual_qa_translation_preserves_new_findings_cas_5488() {
    let fx = scoped_fixture();
    let base = cas_5488_badge(BASE, 1, 100.0);
    let tip = cas_5488_badge(TIP, 1, 639.0);
    for (pointer, value) in [
        ("/selector", serde_json::json!(".new-badge")),
        ("/textSample", serde_json::json!("Warning")),
        ("/type", serde_json::json!("clipped-content")),
        ("/textBounds/width", serde_json::json!(32)),
        ("/ratio", serde_json::json!(1.5)),
        ("/foreground", serde_json::json!([200,200,200])),
    ] {
        let mut changed = tip.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        cas_f290_reports(&fx, serde_json::json!([changed]), serde_json::json!([base.clone()]));
        let refusal = fx.validate(&fx.notes()).unwrap_err();
        assert!(refusal.problem.contains("introduced 1"), "{pointer}: {refusal:?}");
    }
    cas_f290_reports(&fx, serde_json::json!([tip.clone(), tip]), serde_json::json!([base]));
    assert!(fx.validate(&fx.notes()).unwrap_err().problem.contains("introduced 1"));
}

/// cas-1ca0: a valid pixel/trace bundle cannot cover a journey it never ran.
#[test]
fn cas_1ca0_close_refuses_a_hand_picked_journey_subset() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let notes = fx.notes();
    let ctx = EvidenceContext {
        task_id: TASK,
        task_artifacts_dir: &fx.task_dir,
        repo: &fx.repo,
        delivered_head: &fx.head,
        notes: &notes,
        deployed_origins: &[],
    };
    let error = run_close_gate(
        &ctx, EvidenceTier::Bundle,
        &["affected-journeys".into(), "journeys:HUB-J7".into()], &[],
    ).expect_err("selected HUB-J7 is missing even though the hand-picked evidence passes");
    assert!(error.contains("HUB-J7"), "{error}");
    assert!(error.contains("scripts/journey-eval.sh"), "{error}");
}

fn write_journey_receipt(
    fx: &Fixture,
    ids: &[&str],
    scope: &str,
    edit: impl FnOnce(&mut serde_json::Value),
) -> PathBuf {
    let path = fx.task_dir.join("journey-receipt.json");
    let mut value = serde_json::json!({
        "schema": 1, "producer": "journey-eval", "kind": "local", "scope": scope,
        "base_sha": fx.head, "head_sha": fx.head, "selection_ids": ids,
        "tool_version": "playwright 1.63.0", "suite_exit": 0,
        "results": ids.iter().map(|id| serde_json::json!({
            "id": id, "status": "PASS", "passed": 2, "failed": 0, "skipped": 0,
        })).collect::<Vec<_>>()
    });
    edit(&mut value);
    std::fs::write(&path, value.to_string()).unwrap();
    path
}

fn journey_context<'a>(fx: &'a Fixture, notes: &'a str) -> EvidenceContext<'a> {
    EvidenceContext {
        task_id: TASK,
        task_artifacts_dir: &fx.task_dir,
        repo: &fx.repo,
        delivered_head: &fx.head,
        notes,
        deployed_origins: &[],
    }
}

#[test]
fn cas_1ca0_close_accepts_complete_affected_receipt_and_ci_receipt() {
    let fx = Fixture::new();
    let receipt = write_journey_receipt(&fx, &["HUB-J1", "HUB-J7"], "affected", |_| {});
    fx.write_bundle(|v| v["journey_receipt"] = serde_json::json!(receipt));
    let notes = fx.notes();
    let ctx = journey_context(&fx, &notes);
    let reasons = vec![journeys::selection_reason(
        &fx.head,
        &["HUB-J1".into(), "HUB-J7".into()],
    )];
    let pass = run_close_gate(&ctx, EvidenceTier::Bundle, &reasons, &[]).unwrap();
    assert!(
        pass.notes
            .iter()
            .any(|n| n.contains("JOURNEY_SELECTION:") && n.contains("HUB-J7"))
    );
    write_journey_receipt(&fx, &["HUB-J1", "HUB-J7"], "affected", |v| {
        v["kind"] = serde_json::json!("ci");
        v["ci_run_url"] = serde_json::json!("https://github.com/org/repo/actions/runs/42");
    });
    run_close_gate(&ctx, EvidenceTier::Bundle, &reasons, &[]).unwrap();
}

#[test]
fn cas_1ca0_missing_selected_failed_skipped_and_stale_receipts_refuse() {
    let fx = Fixture::new();
    let receipt = write_journey_receipt(&fx, &["HUB-J1"], "affected", |_| {});
    fx.write_bundle(|v| v["journey_receipt"] = serde_json::json!(receipt));
    let notes = fx.notes();
    let ctx = journey_context(&fx, &notes);
    let reasons = vec![journeys::selection_reason(
        &fx.head,
        &["HUB-J1".into(), "HUB-J7".into()],
    )];
    let error = run_close_gate(&ctx, EvidenceTier::Bundle, &reasons, &[]).unwrap_err();
    assert!(error.contains("missing selected IDs [HUB-J7]"), "{error}");
    assert!(error.contains("scripts/journey-eval.sh"), "{error}");
    for key in ["failed", "skipped"] {
        write_journey_receipt(&fx, &["HUB-J1", "HUB-J7"], "affected", |v| {
            v["results"][1][key] = serde_json::json!(1)
        });
        let error = run_close_gate(&ctx, EvidenceTier::Bundle, &reasons, &[]).unwrap_err();
        assert!(error.contains("nonpassing IDs [HUB-J7]"), "{error}");
    }
    write_journey_receipt(&fx, &["HUB-J1", "HUB-J7"], "affected", |v| {
        v["head_sha"] = serde_json::json!("0".repeat(40))
    });
    assert!(
        run_close_gate(&ctx, EvidenceTier::Bundle, &reasons, &[])
            .unwrap_err()
            .contains("exact delivered tip")
    );
}

#[test]
fn cas_1ca0_receipt_provenance_and_task_namespace_are_required() {
    let fx = Fixture::new();
    let notes = fx.notes();
    let ctx = journey_context(&fx, &notes);
    let ids = vec!["HUB-J7".into()];
    for key in ["suite_exit", "schema"] {
        let path = write_journey_receipt(&fx, &["HUB-J7"], "affected", |v| {
            v[key] = serde_json::json!(99)
        });
        assert!(journeys::validate_journey_receipt(&ctx, &path, &fx.head, &ids, false).is_err());
    }
    let path = write_journey_receipt(&fx, &["HUB-J7"], "affected", |v| {
        v["tool_version"] = serde_json::json!("")
    });
    assert!(journeys::validate_journey_receipt(&ctx, &path, &fx.head, &ids, false).is_err());
    let outside = fx.repo.join("foreign-receipt.json");
    std::fs::copy(path, &outside).unwrap();
    assert!(
        journeys::validate_journey_receipt(&ctx, &outside, &fx.head, &ids, false)
            .unwrap_err()
            .problem
            .contains("escapes")
    );
}

#[test]
fn cas_1ca0_independent_qa_reuses_exact_tip_implementer_receipt() {
    let fx = Fixture::new();
    let receipt = write_journey_receipt(&fx, &["HUB-J7"], "affected", |_| {});
    let manifest = fx.write_bundle(|v| {
        v["producer"] = serde_json::json!("independent-qa");
        v["journey_receipt"] = serde_json::json!(receipt);
    });
    let recorded = format!(
        "JOURNEY_SELECTION: head={} base={} ids=HUB-J7",
        fx.head, fx.head
    );
    let ctx = journey_context(&fx, &recorded);
    let reused = journeys::check_round_journeys(&ctx, &manifest, &recorded, &[])
        .unwrap()
        .unwrap();
    assert_eq!(reused, receipt.canonicalize().unwrap());
    write_journey_receipt(&fx, &["HUB-J1"], "affected", |_| {});
    assert!(
        journeys::check_round_journeys(&ctx, &manifest, &recorded, &[])
            .unwrap_err()
            .problem
            .contains("HUB-J7")
    );
    assert!(
        journeys::check_round_journeys(&ctx, &manifest, "", &["hub-web/src/main.ts".into()])
            .unwrap_err()
            .problem
            .contains("no affected-journey selection")
    );
}

#[test]
fn cas_1ca0_epic_requires_full_catalog_receipt_and_selector_errors_refuse() {
    let mut fx = Fixture::new();
    assert!(journeys::select_journeys(&fx.repo, &fx.head, &fx.head, None).is_err());
    std::fs::create_dir_all(fx.repo.join("scripts")).unwrap();
    // Real Python call validates reviewed revision/base env; this fixture
    // selector stands in for the source-graph owner's separate implementation.
    std::fs::write(
        fx.repo.join("scripts/journeys-for-diff.py"),
        r#"
import json, os
assert len(os.environ['CAS_JOURNEYS_HEAD']) == 40
assert len(os.environ['CAS_JOURNEYS_BASE']) == 40
print(json.dumps({'journeys': [{'id': 'HUB-J1'}, {'id': 'HUB-J7'}]}))
"#,
    )
    .unwrap();
    git_ok(&fx.repo, &["add", "scripts/journeys-for-diff.py"]);
    git_ok(&fx.repo, &["commit", "-q", "-m", "fixture selector"]);
    fx.head = git_ok(&fx.repo, &["rev-parse", "HEAD"]);
    let path = write_journey_receipt(&fx, &["HUB-J1", "HUB-J7"], "affected", |_| {});
    let notes = format!("journey-receipt: {}", path.display());
    let ctx = journey_context(&fx, &notes);
    let error = journeys::check_epic_journeys(&ctx).unwrap_err();
    assert!(error.problem.contains("full-suite"), "{error:?}");
    assert!(error.command.contains("no journey filter"), "{error:?}");
    write_journey_receipt(&fx, &["HUB-J1", "HUB-J7"], "full", |_| {});
    journeys::check_epic_journeys(&ctx).unwrap();
    write_journey_receipt(&fx, &["HUB-J1"], "full", |_| {});
    assert!(
        journeys::check_epic_journeys(&ctx)
            .unwrap_err()
            .problem
            .contains("HUB-J7")
    );
    std::fs::write(
        fx.repo.join("scripts/journeys-for-diff.py"),
        "raise SystemExit(2)\n",
    )
    .unwrap();
    // Uncommitted checkout drift cannot replace the reviewed selector.
    write_journey_receipt(&fx, &["HUB-J1", "HUB-J7"], "full", |_| {});
    journeys::check_epic_journeys(&ctx).unwrap();
    git_ok(&fx.repo, &["add", "scripts/journeys-for-diff.py"]);
    git_ok(&fx.repo, &["commit", "-q", "-m", "broken committed selector"]);
    fx.head = git_ok(&fx.repo, &["rev-parse", "HEAD"]);
    let ctx = journey_context(&fx, &notes);
    assert!(
        journeys::check_epic_journeys(&ctx)
            .unwrap_err()
            .problem
            .contains("selection failed")
    );
}

#[test]
fn cas_1ca0_empty_impact_does_not_require_browser_run() {
    let fx = Fixture::new();
    let receipt = write_journey_receipt(&fx, &[], "affected", |_| {});
    fx.write_bundle(|v| v["journey_receipt"] = serde_json::json!(receipt));
    let notes = fx.notes();
    let ctx = journey_context(&fx, &notes);
    let selection =
        journeys::check_close_journeys(&ctx, &[journeys::selection_reason(&fx.head, &[])])
            .unwrap()
            .unwrap();
    assert!(selection.contains("no affected journeys"));
}

#[test]
fn cas_1ca0_doc_rebind_reuses_execution_but_catalog_and_source_changes_refuse() {
    let mut fx = Fixture::new();
    let executed = fx.head.clone();
    std::fs::create_dir_all(fx.repo.join("docs")).unwrap();
    fx.head = commit(&fx.repo, "docs/QA.md", 0);
    assert!(journeys::doc_only_rebind(&fx.repo, &executed, &fx.head));
    let receipt = write_journey_receipt(&fx, &["HUB-J7"], "affected", |v| {
        v["base_sha"] = serde_json::json!(executed);
        v["executed_head_sha"] = serde_json::json!(executed);
    });
    fx.write_bundle(|v| {
        v["journey_receipt"] = serde_json::json!(receipt);
        v["executed_head_sha"] = serde_json::json!(executed);
        v["created_at"] =
            serde_json::json!((chrono::Utc::now() - chrono::Duration::seconds(90)).to_rfc3339());
    });
    write_visual_qa_report(
        &fx.bundle_dir(),
        "PASS",
        chrono::Utc::now() - chrono::Duration::seconds(90),
        "http://localhost:31000",
    );
    for key in [
        "trace.zip",
        "trace-actions.txt",
        "receipt.webm",
        "final.aria.yml",
        "final.aria.json",
        "M01.png",
    ] {
        set_mtime_secs_ago(&fx.bundle_dir().join(key), 90);
    }
    let notes = fx.notes();
    let ctx = journey_context(&fx, &notes);
    run_close_gate(
        &ctx,
        EvidenceTier::Bundle,
        &[journeys::selection_reason(&executed, &["HUB-J7".into()])],
        &[],
    )
    .unwrap();
    std::fs::create_dir_all(fx.repo.join("docs/qa")).unwrap();
    let catalog_change = commit(&fx.repo, "docs/qa/journeys.md", 0);
    assert!(!journeys::doc_only_rebind(
        &fx.repo,
        &fx.head,
        &catalog_change
    ));
    let source_change = commit(&fx.repo, "changed.ts", 0);
    assert!(!journeys::doc_only_rebind(
        &fx.repo,
        &executed,
        &source_change
    ));
}
