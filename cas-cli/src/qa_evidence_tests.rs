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
