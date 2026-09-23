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
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
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
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
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
        Self { _tmp: tmp, repo, task_dir, head }
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
            ("trace-actions.txt", "   1. 0:00.1  Expect \"toHaveText\"   2ms\n"),
            ("receipt.webm", "webm"),
            ("final.aria.yml", "- heading \"x\""),
            ("final.aria.json", "{}"),
            ("M01.png", "png"),
            ("visual-qa/app-light-desktop.png", "png"),
            ("visual-qa/app-light-phone.png", "png"),
            ("visual-qa/app-dark-desktop.png", "png"),
            ("visual-qa/app-dark-phone.png", "png"),
            ("visual-qa/visual-qa.md", "# Visual QA — PASS\n"),
            ("visual-qa/visual-qa.json", "{}"),
            ("visual-qa.stdout", "rendered 4\nPASS\n"),
            ("critique.md", "| Dimension | Score |\nScored by t on 2026-09-23\n"),
            ("a11y-forced-colors.png", "png"),
            ("a11y-reduced-motion.png", "png"),
            ("a11y-contrast-more.png", "png"),
        ];
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
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
    assert!(refusal.command.contains("note_type=platform_proof"), "{refusal:?}");
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
    assert!(refusal.problem.contains("files.trace_actions"), "{refusal:?}");
    assert!(refusal.command.contains("npx playwright trace actions"), "{refusal:?}");

    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    std::fs::write(fx.bundle_dir().join("receipt.webm"), "").unwrap();
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("files.receipt") && refusal.problem.contains("empty"), "{refusal:?}");

    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["files"].as_object_mut().unwrap().remove("critique");
    });
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("files.critique"), "{refusal:?}");
}

#[test]
fn polish_renders_must_cover_light_dark_desktop_phone() {
    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["files"]["polish_screenshots"] = serde_json::json!(["visual-qa/app-light-desktop.png"]);
    });
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("-light-phone.png"), "{refusal:?}");
    assert!(refusal.command.contains("visual-qa.mjs --strict"), "{refusal:?}");
}

#[test]
fn stale_head_sha_is_rejected_after_a_later_commit() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let later = Fixture { head: commit(&fx.repo, "fix.ts", 60), ..fx };
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
    assert!(refusal.problem.contains("stale") && refusal.problem.contains("cells[0]"), "{refusal:?}");
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
    fx.validate(&fx.notes()).expect("descendant build covers the delivery");
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
fn polish_proof_and_critique_floor_are_enforced() {
    let fx = Fixture::new();
    fx.write_bundle(|manifest| manifest["visual_qa_status"] = serde_json::json!("unavailable"));
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("unavailable"), "{refusal:?}");

    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    std::fs::write(fx.bundle_dir().join("visual-qa.stdout"), "FAIL\n").unwrap();
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("visual_qa_stdout"), "{refusal:?}");

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
    fx.write_bundle(|manifest| manifest["files"]["a11y"] = serde_json::json!(["a11y-forced-colors.png"]));
    let refusal = fx.validate(&fx.notes()).unwrap_err();
    assert!(refusal.problem.contains("reduced-motion"), "{refusal:?}");

    let fx = Fixture::new();
    fx.write_bundle(|manifest| {
        manifest["visual_change"] = serde_json::json!(false);
        manifest["files"]["a11y"] = serde_json::json!([]);
    });
    fx.validate(&fx.notes()).expect("non-visual change needs no a11y captures");
}

#[test]
fn citations_outside_the_task_or_from_an_independent_round_are_rejected() {
    let fx = Fixture::new();
    fx.write_bundle(|_| {});
    let outside = fx.task_dir.parent().unwrap().join("other");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::copy(fx.bundle_dir().join("bundle.json"), outside.join("bundle.json")).unwrap();
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
    std::fs::copy(fx.bundle_dir().join("bundle.json"), round.join("bundle.json")).unwrap();
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
    fx.write_bundle(|manifest| manifest["files"]["receipt"] = serde_json::json!("../../../repo/app.ts"));
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
    assert!(validate_ledger(&ctx).unwrap_err().problem.contains("missing"));
    let ledger = fx.task_dir.join("LEDGER.md");
    std::fs::write(&ledger, "| id | cell | expected | observed | verdict | label | evidence | defect |\n| M01 | cli | ok | ok | NOT EXERCISED | fixture | - | - |\n").unwrap();
    assert!(validate_ledger(&ctx).unwrap_err().problem.contains("no row with verdict PASS"));
    std::fs::write(&ledger, "| M01 | cli | ok | ok | PASS | real-build | qa/M01.txt | - |\n").unwrap();
    validate_ledger(&ctx).expect("fresh PASS row");
    set_mtime_secs_ago(&ledger, 3600);
    assert!(validate_ledger(&ctx).unwrap_err().problem.starts_with("stale"));
}

#[test]
fn trace_summary_counts_top_level_errors_as_failures() {
    let summary = expect_summary_from_events(&[PASSING[0], PASSING[1], r#"{"type":"error","message":"boom"}"#].join("\n"));
    assert_eq!(summary, ExpectSummary { passed: 1, failed: 1 });
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
    assert_eq!(markers[1].allowed.as_deref(), Some("WebKit lacks the clipboard permission this test needs"));
    assert_eq!(markers[2].marker, "test.only(");
    assert_eq!(markers[2].allowed, None, "the allow reason covers only the next line");
}

#[test]
fn skip_marker_matching_respects_word_boundaries_and_file_kinds() {
    assert_eq!(marker_in("process.exit(1)"), None);
    assert_eq!(marker_in("unit.skip(x)"), None);
    assert_eq!(marker_in("  xit('pending', () => {})"), Some("xit("));
    assert_eq!(marker_in("test.describe.skip('group')"), Some("test.describe.skip("));
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
    git_ok(&fx.repo, &["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", "heal"]);
    let head = git_ok(&fx.repo, &["rev-parse", "HEAD"]);
    let diff = delivery_test_diff(&fx.repo, &base, &head, &["e2e/a.spec.ts".into(), "notes.md".into()]).unwrap();
    let markers = added_skip_markers(&diff);
    assert_eq!(markers.len(), 1, "{diff}");
    assert_eq!(markers[0].file, "e2e/a.spec.ts");
    assert_eq!(delivery_test_diff(&fx.repo, &base, &head, &["notes.md".into()]).as_deref(), Some(""));
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
    assert!(error.starts_with("TASK CLOSE REJECTED: cas-test adds test skip/focus markers"), "{error}");
    assert!(error.contains("hub-web/e2e/generated/fleet.spec.ts:12 `test.fixme`"), "{error}");
    assert!(error.contains("hub-web/e2e/webkit.spec.ts:3 `test.only`"), "{error}");
    assert!(!error.contains("webkit.spec.ts:2"), "allowed skip must not be listed: {error}");
    assert!(error.contains(ALLOW_SKIP), "{error}");

    let allowed_only: Vec<SkipMarker> = markers.into_iter().filter(|marker| marker.allowed.is_some()).collect();
    let pass = run_close_gate(&ctx, EvidenceTier::Bundle, &["demo_statement".into()], &allowed_only).unwrap();
    assert!(pass.notes.iter().any(|note| note.contains("Allowed skip marker hub-web/e2e/webkit.spec.ts:2")), "{pass:?}");
    assert!(pass.notes.iter().any(|note| note.starts_with("QA evidence bundle accepted")), "{pass:?}");
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
    let error = run_close_gate(&ctx, EvidenceTier::Bundle, &["journeys:J03".into(), "demo_statement".into()], &[])
        .unwrap_err();
    assert!(
        error.starts_with("TASK CLOSE REJECTED: cas-test is user-facing (journeys:J03; demo_statement) and its QA evidence bundle is not cited"),
        "{error}"
    );
    assert!(error.contains("Next: produce the bundle under"), "{error}");
    assert!(error.contains(CONTRACT_REFERENCE), "{error}");
    let error = run_close_gate(&ctx, EvidenceTier::Ledger, &["demo_statement".into()], &[]).unwrap_err();
    assert!(error.contains("QA evidence ledger is missing"), "{error}");
    assert!(run_close_gate(&ctx, EvidenceTier::None, &[], &[]).unwrap().notes.is_empty());
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
    assert_eq!(range_paths(&fx.repo, &from, &to).unwrap(), vec!["ui.tsx".to_string()]);
    git_ok(&fx.repo, &["checkout", "-q", "main"]);
    commit(&fx.repo, "other.rs", 20);
    git_ok(&fx.repo, &["-c", "user.name=t", "-c", "user.email=t@example.com", "merge", "-q", "--no-ff", "-m", "merge", "factory/w"]);
    let (from, to) = delivery_range(&fx.repo, &tip, "main").unwrap();
    assert!(from.ends_with("^1"), "{from}");
    assert_eq!(range_paths(&fx.repo, &from, &to).unwrap(), vec!["ui.tsx".to_string()]);
}
