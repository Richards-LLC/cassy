//! cassy#1157: `cas violet` against a scripted hub double.
//!
//! The double answers each call with the next queued MCP tool result and
//! records every call and every direct upload, so the tests can prove which
//! route was taken and that file bytes never reach a call or stdout.

use super::files::{INLINE_LIMIT, LocalFile, Route, choose_route, encode_content};
use super::hub::{Hub, VioletError, decode_tool_result};
use super::*;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use tempfile::TempDir;

#[derive(Default)]
struct Script {
    replies: VecDeque<Value>,
    calls: Vec<(String, Value)>,
    /// (upload_url, sha256 of the bytes read from disk, length)
    uploads: Vec<(String, String, u64)>,
    connects: usize,
}

#[derive(Clone, Default)]
struct ScriptedHub(Rc<RefCell<Script>>);

impl ScriptedHub {
    fn with(replies: Vec<Value>) -> Self {
        let hub = Self::default();
        hub.0.borrow_mut().replies = replies.into_iter().map(wrapped).collect();
        hub
    }
    fn calls(&self) -> Vec<(String, Value)> {
        self.0.borrow().calls.clone()
    }
    fn uploads(&self) -> Vec<(String, String, u64)> {
        self.0.borrow().uploads.clone()
    }
    fn connects(&self) -> usize {
        self.0.borrow().connects
    }
}

impl Hub for ScriptedHub {
    fn call(&mut self, tool: &str, args: Value) -> Result<Value, VioletError> {
        let mut script = self.0.borrow_mut();
        script.calls.push((tool.to_string(), args));
        let reply = script
            .replies
            .pop_front()
            .ok_or_else(|| VioletError::local("test_unscripted", "no reply queued"))?;
        decode_tool_result(&reply)
    }

    fn upload(&mut self, upload_url: &str, file: &LocalFile) -> Result<(), VioletError> {
        let bytes = std::fs::read(&file.path).unwrap();
        self.0.borrow_mut().uploads.push((
            upload_url.to_string(),
            format!("{:x}", Sha256::digest(&bytes)),
            bytes.len() as u64,
        ));
        Ok(())
    }
}

/// The MCP tool-result wrapper a Violet receipt arrives in; a hub error
/// receipt also sets `isError`.
fn wrapped(receipt: Value) -> Value {
    let is_error = receipt.get("ok") == Some(&json!(false));
    json!({ "content": [{ "type": "text", "text": receipt.to_string() }], "isError": is_error })
}

fn write_file(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn channel() -> Value {
    json!({"id": "C1157", "name": "cas-internal"})
}

fn file_receipt(file_id: &str, name: &str, bytes: &[u8]) -> Value {
    json!({
        "ok": true, "schema_version": 1, "kind": "file", "channel": channel(),
        "message": {"message_id": "1710000000.000003", "thread_id": null, "permalink": "https://slack.test/f/3"},
        "file": {"file_id": file_id, "name": name, "size_bytes": bytes.len(), "sha256": sha(bytes),
                 "sha256_verified": true, "state": "attached", "image_validated": null,
                 "permalink": "https://slack.test/f/3"},
        "warning": null
    })
}

/// Run one parsed command line against `hub`, returning stdout and the result.
fn run_cli(
    argv: &[&str],
    hub: &ScriptedHub,
    json: bool,
    gate: &dyn Fn(&Value) -> Option<String>,
) -> (String, Result<(), VioletError>) {
    #[derive(clap::Parser)]
    struct Harness {
        #[command(subcommand)]
        command: VioletCommand,
    }
    let raw: Vec<String> = std::iter::once("violet".to_string())
        .chain(argv.iter().map(|arg| arg.to_string()))
        .collect();
    let parsed = <Harness as clap::Parser>::try_parse_from(&raw).expect("valid command line");
    let shared = hub.clone();
    let mut connect = move || -> Result<Box<dyn Hub>, VioletError> {
        shared.0.borrow_mut().connects += 1;
        Ok(Box::new(shared.clone()))
    };
    let mut context = RunContext {
        json,
        raw_args: &raw,
        gate,
        connect: &mut connect,
    };
    let mut out = Vec::new();
    let result = run_violet(&parsed.command, &mut context, &mut out);
    (String::from_utf8(out).unwrap(), result)
}

fn no_gate(_: &Value) -> Option<String> {
    None
}

// ---------------------------------------------------------------------------
// Route selection at the 1 MiB boundary
// ---------------------------------------------------------------------------

#[test]
fn exactly_one_mebibyte_goes_inline_in_one_call() {
    let dir = TempDir::new().unwrap();
    let bytes = vec![b'a'; INLINE_LIMIT as usize];
    let path = write_file(&dir, "edge.txt", &bytes);
    let hub = ScriptedHub::with(vec![file_receipt("F1", "edge.txt", &bytes)]);

    let (_, result) = run_cli(
        &[
            "post",
            "--channel",
            "cas-internal",
            "--file",
            path.to_str().unwrap(),
        ],
        &hub,
        false,
        &no_gate,
    );
    result.unwrap();

    let calls = hub.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let (tool, args) = &calls[0];
    assert_eq!(tool, "violet_post");
    assert_eq!(args["kind"], "file");
    assert_eq!(args["file"]["content_encoding"], "text");
    assert_eq!(args["file"]["size_bytes"], INLINE_LIMIT);
    assert_eq!(args["file"]["sha256"], sha(&bytes));
    assert!(hub.uploads().is_empty());
}

#[test]
fn one_byte_over_one_mebibyte_uses_file_external_with_bytes_from_disk() {
    let dir = TempDir::new().unwrap();
    let bytes = vec![b'b'; INLINE_LIMIT as usize + 1];
    let path = write_file(&dir, "big.txt", &bytes);
    let hub = ScriptedHub::with(vec![
        json!({"ok": true, "schema_version": 1, "kind": "file_external", "step": "begin",
               "upload_url": "https://files.slack.test/upload/F2", "file_id": "F2"}),
        file_receipt("F2", "big.txt", &bytes),
    ]);

    let (_, result) = run_cli(
        &[
            "post",
            "--channel",
            "cas-internal",
            "--text",
            "the log",
            "--reply-to",
            "1710000000.000001",
            "--file",
            path.to_str().unwrap(),
        ],
        &hub,
        false,
        &no_gate,
    );
    result.unwrap();

    let calls = hub.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    let begin = &calls[0].1;
    assert_eq!(begin["kind"], "file_external");
    assert_eq!(begin["step"], "begin");
    assert_eq!(begin["filename"], "big.txt");
    assert_eq!(begin["size_bytes"], INLINE_LIMIT + 1);
    assert_eq!(begin["sha256"], sha(&bytes));
    assert_eq!(begin["initial_comment"], "the log");
    assert_eq!(begin["reply_to"], "1710000000.000001");
    let complete = &calls[1].1;
    assert_eq!(complete["step"], "complete");
    assert_eq!(complete["file_id"], "F2");
    for (_, args) in &calls {
        assert!(
            args.get("content").is_none() && args.get("file").is_none(),
            "{args}"
        );
    }
    assert_eq!(
        hub.uploads(),
        vec![(
            "https://files.slack.test/upload/F2".to_string(),
            sha(&bytes),
            INLINE_LIMIT + 1
        )]
    );
}

#[test]
fn the_limit_is_the_aggregate_across_files() {
    let dir = TempDir::new().unwrap();
    let half = vec![b'c'; INLINE_LIMIT as usize / 2];
    let a = write_file(&dir, "a.txt", &half);
    let b = write_file(&dir, "b.txt", &half);
    let files = vec![
        LocalFile::inspect(&a).unwrap(),
        LocalFile::inspect(&b).unwrap(),
    ];
    assert_eq!(choose_route(&files), Route::Inline);

    let hub = ScriptedHub::with(vec![
        json!({"ok": true, "kind": "file", "channel": channel(),
        "message": {"message_id": "1.2", "permalink": "https://slack.test/p"},
        "files": [{"file_id": "F3"}, {"file_id": "F4"}]}),
    ]);
    let mut live = hub.clone();
    post_files(&mut live, "cas-internal", None, None, &files).unwrap();
    let args = &hub.calls()[0].1;
    assert_eq!(args["kind"], "file");
    assert_eq!(args["files"].as_array().unwrap().len(), 2);
    assert!(args.get("file").is_none());

    // One more byte anywhere moves the whole post to file_external.
    let c = write_file(&dir, "c.txt", &[b'c'; INLINE_LIMIT as usize / 2 + 1]);
    let files = vec![
        LocalFile::inspect(&a).unwrap(),
        LocalFile::inspect(&c).unwrap(),
    ];
    assert_eq!(choose_route(&files), Route::External);
    let hub = ScriptedHub::with(vec![
        json!({"ok": true, "kind": "file_external", "step": "begin", "files": [
            {"filename": "a.txt", "file_id": "F5", "upload_url": "https://files.slack.test/u/F5"},
            {"filename": "c.txt", "file_id": "F6", "upload_url": "https://files.slack.test/u/F6"}]}),
        json!({"ok": true, "kind": "file", "channel": channel(),
               "message": {"message_id": "1.3", "permalink": "https://slack.test/p3"},
               "files": [{"file_id": "F5"}, {"file_id": "F6"}]}),
    ]);
    let mut live = hub.clone();
    post_files(&mut live, "cas-internal", Some("two".into()), None, &files).unwrap();
    let calls = hub.calls();
    assert_eq!(calls[0].1["files"][1]["filename"], "c.txt");
    assert_eq!(calls[1].1["files"][0]["file_id"], "F5");
    assert_eq!(calls[1].1["files"][1]["file_id"], "F6");
    assert_eq!(hub.uploads().len(), 2);
}

#[test]
fn more_than_ten_files_is_refused_before_any_connection() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "x.txt", b"x");
    let path = path.to_str().unwrap();
    let mut argv = vec!["post", "--channel", "cas-internal"];
    for _ in 0..11 {
        argv.extend(["--file", path]);
    }
    let hub = ScriptedHub::default();
    let (_, result) = run_cli(&argv, &hub, false, &no_gate);
    assert_eq!(result.unwrap_err().code, "invalid_input");
    assert_eq!(hub.connects(), 0);
}

// ---------------------------------------------------------------------------
// Text vs base64
// ---------------------------------------------------------------------------

#[test]
fn text_and_binary_are_detected_from_the_bytes() {
    let decode = |content: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(content)
            .unwrap()
    };
    for text in [
        "# Notes\n\n- one\n",
        "a,b\r\n1,2\r\n",
        "tab\tseparated",
        "{\"ok\":true}",
        "naïve — ✓",
    ] {
        assert_eq!(
            encode_content(text.as_bytes()),
            (text.to_string(), "text"),
            "{text:?}"
        );
    }
    let binaries: [&[u8]; 6] = [
        b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR",
        b"%PDF-1.4 all ascii but still a pdf",
        b"PK\x03\x04zip",
        b"plain then a NUL\0",
        b"\xff\xfe not utf-8",
        "C1 control \u{85} inside".as_bytes(),
    ];
    for bytes in binaries {
        let (content, encoding) = encode_content(bytes);
        assert_eq!(encoding, "base64", "{bytes:?}");
        assert_eq!(
            decode(&content),
            bytes,
            "base64 must preserve the bytes exactly"
        );
    }
}

#[test]
fn an_empty_or_missing_file_is_a_local_error() {
    let dir = TempDir::new().unwrap();
    let empty = write_file(&dir, "empty.txt", b"");
    assert_eq!(
        LocalFile::inspect(&empty).unwrap_err().code,
        "invalid_input"
    );
    assert_eq!(
        LocalFile::inspect(&dir.path().join("absent.png"))
            .unwrap_err()
            .code,
        "local_request_failed"
    );
    assert_eq!(
        LocalFile::inspect(dir.path()).unwrap_err().code,
        "invalid_input"
    );
}

// ---------------------------------------------------------------------------
// Thread
// ---------------------------------------------------------------------------

#[test]
fn replies_keep_their_order_and_their_files() {
    let raw: Vec<String> = [
        "cas",
        "violet",
        "thread",
        "--channel",
        "cas-internal",
        "--text",
        "--reply-like root",
        "--reply",
        "first",
        "--reply-file",
        "a.png",
        "--reply-file=b.pdf",
        "--reply=second",
        "--idempotency-key",
        "k",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let replies = order_replies(
        &raw,
        &["first".to_string(), "second".to_string()],
        &[PathBuf::from("a.png"), PathBuf::from("b.pdf")],
    )
    .unwrap();
    assert_eq!(
        replies,
        vec![
            ReplySpec {
                text: Some("first".into()),
                files: vec!["a.png".into(), "b.pdf".into()]
            },
            ReplySpec {
                text: Some("second".into()),
                files: vec![]
            },
        ]
    );

    // A file before any --reply is its own file-only reply.
    let raw: Vec<String> = ["thread", "--reply-file", "a.png", "--reply", "x"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let replies = order_replies(&raw, &["x".into()], &["a.png".into()]).unwrap();
    assert_eq!(
        replies[0],
        ReplySpec {
            text: None,
            files: vec!["a.png".into()]
        }
    );

    // A parse that disagrees with clap's is refused, not guessed.
    let err = order_replies(&raw, &["y".into()], &["a.png".into()]).unwrap_err();
    assert_eq!(err.code, "invalid_input");
}

#[test]
fn a_thread_is_one_ordered_call_with_inline_reply_files() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "notes.md", b"# hello\n");
    let hub = ScriptedHub::with(vec![json!({
        "ok": true, "schema_version": 1, "kind": "thread", "channel": channel(),
        "idempotency_key": "2026-10-09-user",
        "posted": [
            {"index": 0, "message_id": "1.0", "thread_id": "1.0", "permalink": "https://slack.test/0"},
            {"index": 1, "message_id": "1.1", "thread_id": "1.0", "permalink": "https://slack.test/1",
             "files": [{"file_id": "F7", "name": "notes.md", "size_bytes": 8, "sha256_verified": true}]},
            {"index": 2, "message_id": "1.2", "thread_id": "1.0", "permalink": "https://slack.test/2"}
        ],
        "failed_index": null, "resume_safe": true
    })]);
    let (out, result) = run_cli(
        &[
            "thread",
            "--channel",
            "cas-internal",
            "--text",
            "Staging · User — notes",
            "--reply",
            "Was → Now",
            "--reply-file",
            path.to_str().unwrap(),
            "--reply",
            "more",
            "--idempotency-key",
            "2026-10-09-user",
        ],
        &hub,
        false,
        &no_gate,
    );
    result.unwrap();
    let calls = hub.calls();
    assert_eq!(calls.len(), 1);
    let args = &calls[0].1;
    assert_eq!(args["kind"], "thread");
    assert_eq!(args["idempotency_key"], "2026-10-09-user");
    assert_eq!(args["replies"][0]["text"], "Was → Now");
    assert_eq!(args["replies"][0]["files"][0]["content"], "# hello\n");
    assert_eq!(args["replies"][0]["files"][0]["content_encoding"], "text");
    assert_eq!(args["replies"][1]["text"], "more");
    assert!(args["replies"][1].get("files").is_none());
    assert!(
        out.starts_with("[OK] posted thread to #cas-internal (C1157) - 3 messages"),
        "{out}"
    );
    assert!(out.contains("file F7 notes.md"), "{out}");
}

#[test]
fn a_stopped_thread_reports_failed_index_and_exits_non_zero() {
    let failure = json!({
        "ok": false, "schema_version": 1, "kind": "thread", "channel": channel(),
        "idempotency_key": "k1",
        "posted": [{"index": 0, "message_id": "1.0", "thread_id": "1.0", "permalink": "https://slack.test/0"}],
        "failed_index": 1, "resume_safe": false,
        "error": {"code": "slack_error", "message": "msg_too_long", "retryable": false}
    });
    let argv = [
        "thread",
        "--channel",
        "cas-internal",
        "--text",
        "root",
        "--reply",
        "r1",
        "--reply",
        "r2",
        "--idempotency-key",
        "k1",
    ];

    let hub = ScriptedHub::with(vec![failure.clone()]);
    let (out, result) = run_cli(&argv, &hub, false, &no_gate);
    let error = result.unwrap_err();
    assert_eq!(error.code, "slack_error");
    assert!(
        out.contains("stopped at index 1; resume_safe false"),
        "{out}"
    );
    assert!(out.contains("0 root  1.0"), "{out}");
    assert!(out.contains("cas violet read --thread"), "{out}");

    let hub = ScriptedHub::with(vec![failure.clone()]);
    let (out, result) = run_cli(&argv, &hub, true, &no_gate);
    assert!(result.is_err());
    let printed: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        printed, failure,
        "--json prints the hub's error receipt verbatim"
    );
}

#[test]
fn an_external_thread_uploads_first_and_sends_only_metadata() {
    let dir = TempDir::new().unwrap();
    let big = vec![0u8; INLINE_LIMIT as usize + 10];
    let path = write_file(&dir, "trace.bin", &big);
    let replies = vec![ReplyPlan {
        text: Some("trace".into()),
        files: vec![LocalFile::inspect(&path).unwrap()],
    }];
    let hub = ScriptedHub::with(vec![
        json!({"ok": true, "kind": "file_external", "step": "begin",
               "upload_url": "https://files.slack.test/u/F8", "file_id": "F8"}),
        json!({"ok": true, "kind": "thread", "channel": channel(), "posted": [], "failed_index": null}),
    ]);
    let mut live = hub.clone();
    post_thread(&mut live, "cas-internal", "root", "k2", &replies).unwrap();
    let calls = hub.calls();
    assert_eq!(calls[0].1["step"], "begin");
    let reply_file = &calls[1].1["replies"][0]["files"][0];
    assert_eq!(reply_file["file_id"], "F8");
    assert_eq!(reply_file["size_bytes"], big.len());
    assert!(reply_file.get("content").is_none());
    assert_eq!(hub.uploads()[0].1, sha(&big));
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[test]
fn hub_error_codes_pass_through_and_fail_the_run() {
    let refusal = json!({"ok": false, "error": {
        "code": "not_member", "message": "Invite @Violet to #secret and retry.", "retryable": false}});
    let hub = ScriptedHub::with(vec![refusal.clone()]);
    let (out, result) = run_cli(
        &["post", "--channel", "secret", "--text", "hi"],
        &hub,
        false,
        &no_gate,
    );
    let error = result.unwrap_err();
    assert_eq!(error.code, "not_member");
    assert_eq!(
        error.to_string(),
        "not_member: Invite @Violet to #secret and retry."
    );
    assert!(
        out.is_empty(),
        "a refused post prints no success line: {out}"
    );
    // The CLI's own error path turns this into a non-zero exit.
    assert!(
        anyhow::Error::new(error)
            .to_string()
            .starts_with("not_member")
    );

    let hub = ScriptedHub::with(vec![refusal.clone()]);
    let (out, result) = run_cli(
        &["post", "--channel", "secret", "--text", "hi"],
        &hub,
        true,
        &no_gate,
    );
    assert!(result.is_err());
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), refusal);
}

#[test]
fn a_failed_completion_names_the_upload_it_began() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "big.txt", &vec![b'z'; INLINE_LIMIT as usize + 1]);
    let hub = ScriptedHub::with(vec![
        json!({"ok": true, "upload_url": "https://files.slack.test/u/F9", "file_id": "F9"}),
        json!({"ok": false, "error": {"code": "file_integrity_mismatch", "message": "sha256", "retryable": false}}),
    ]);
    let (out, result) = run_cli(
        &[
            "post",
            "--channel",
            "cas-internal",
            "--file",
            path.to_str().unwrap(),
        ],
        &hub,
        true,
        &no_gate,
    );
    let error = result.unwrap_err();
    assert_eq!(error.code, "file_integrity_mismatch");
    assert_eq!(error.file_ids, vec!["F9".to_string()]);
    assert!(
        error
            .to_string()
            .contains("check the channel before posting again")
    );
    let printed: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(printed["error"]["code"], "file_integrity_mismatch");
    assert_eq!(printed["uploads_begun"], json!(["F9"]));
}

#[test]
fn malformed_receipts_are_hub_bad_response() {
    for result in [
        json!({"content": [{"type": "text", "text": "<html>protected</html>"}], "isError": true}),
        json!({"content": [{"type": "text", "text": "{\"no\":\"flag\"}"}]}),
        json!({"content": []}),
    ] {
        assert_eq!(
            decode_tool_result(&result).unwrap_err().code,
            "hub_bad_response",
            "{result}"
        );
    }
    let bare = json!({"ok": true, "kind": "message"});
    assert_eq!(decode_tool_result(&bare).unwrap(), bare);
    let structured = json!({"structuredContent": {"ok": false, "error": {"code": "x", "message": "m", "retryable": true}}});
    assert_eq!(
        decode_tool_result(&structured).unwrap()["error"]["code"],
        "x"
    );
}

#[test]
fn a_begin_receipt_that_does_not_match_the_files_uploads_nothing() {
    let dir = TempDir::new().unwrap();
    let a = write_file(&dir, "a.bin", &vec![1u8; INLINE_LIMIT as usize]);
    let b = write_file(&dir, "b.bin", &[2u8; 8]);
    let files = vec![
        LocalFile::inspect(&a).unwrap(),
        LocalFile::inspect(&b).unwrap(),
    ];
    let hub = ScriptedHub::with(vec![json!({"ok": true, "files": [
        {"filename": "b.bin", "file_id": "F1", "upload_url": "https://files.slack.test/1"},
        {"filename": "a.bin", "file_id": "F2", "upload_url": "https://files.slack.test/2"}]})]);
    let mut live = hub.clone();
    let error = post_files(&mut live, "cas-internal", None, None, &files).unwrap_err();
    assert_eq!(error.code, "hub_bad_response");
    assert!(hub.uploads().is_empty());
}

#[test]
fn connection_failures_are_named_without_values() {
    assert_eq!(
        hub::connect_error("authentication_required").code,
        "invalid_token"
    );
    let missing = hub::connect_error("missing_credential_env:VIOLET_SLACK_TOKEN");
    assert_eq!(missing.code, "missing_credential");
    assert!(missing.message.contains("VIOLET_SLACK_TOKEN"));
    assert_eq!(hub::connect_error("timeout").code, "hub_unreachable");
    assert_eq!(
        hub::connect_error("unexpected_content_type").code,
        "hub_bad_response"
    );
}

#[test]
fn the_publication_gate_refuses_a_file_post_before_connecting() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "report.pdf", b"%PDF-1.4\n");
    let seen = RefCell::new(Vec::new());
    let gate = |post: &Value| {
        seen.borrow_mut().push(post.clone());
        Some("PUBLICATION BLOCKED (verification_pending)".to_string())
    };
    let hub = ScriptedHub::default();
    let (_, result) = run_cli(
        &[
            "post",
            "--channel",
            "cas-internal",
            "--file",
            path.to_str().unwrap(),
        ],
        &hub,
        false,
        &gate,
    );
    assert_eq!(result.unwrap_err().code, "publication_blocked");
    assert_eq!(hub.connects(), 0);
    assert_eq!(seen.borrow()[0]["kind"], "file");

    // A plain message is not a deliverable share.
    let hub = ScriptedHub::with(vec![
        json!({"ok": true, "kind": "message", "channel": channel(),
        "message": {"message_id": "1.9", "permalink": "https://slack.test/9"}}),
    ]);
    let (out, result) = run_cli(
        &["post", "--channel", "cas-internal", "--text", "hi"],
        &hub,
        false,
        &gate,
    );
    result.unwrap();
    assert!(
        out.starts_with("[OK] posted to #cas-internal (C1157) - message"),
        "{out}"
    );
    assert_eq!(seen.borrow().len(), 1);
}

// ---------------------------------------------------------------------------
// No bytes on stdout
// ---------------------------------------------------------------------------

#[test]
fn no_file_bytes_reach_stdout() {
    const MARKER: &str = "FILE-BYTES-MUST-NOT-PRINT-7f3a9c21";
    let dir = TempDir::new().unwrap();
    let text = format!("{MARKER}\n");
    let small = write_file(&dir, "small.txt", text.as_bytes());
    let mut binary = b"\x89PNG\r\n\x1a\n".to_vec();
    binary.extend(MARKER.as_bytes());
    let png = write_file(&dir, "shot.png", &binary);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&binary);
    let receipt = json!({"ok": true, "kind": "file", "channel": channel(),
        "message": {"message_id": "1.4", "permalink": "https://slack.test/4"},
        "files": [{"file_id": "F10", "name": "small.txt", "size_bytes": text.len(), "sha256_verified": true},
                  {"file_id": "F11", "name": "shot.png", "size_bytes": binary.len(), "sha256_verified": true}]});

    for json_mode in [false, true] {
        let hub = ScriptedHub::with(vec![receipt.clone()]);
        let (out, result) = run_cli(
            &[
                "post",
                "--channel",
                "cas-internal",
                "--file",
                small.to_str().unwrap(),
                "--file",
                png.to_str().unwrap(),
            ],
            &hub,
            json_mode,
            &no_gate,
        );
        result.unwrap();
        // The bytes did go to the hub, inline...
        let sent = hub.calls()[0].1.to_string();
        assert!(sent.contains(MARKER) && sent.contains(&encoded));
        // ...and never to stdout.
        assert!(!out.contains(MARKER) && !out.contains(&encoded), "{out}");
        assert!(out.contains("F11"), "{out}");
    }

    // A read that downloads a file prints its metadata, not its content.
    let read = json!({"ok": true, "schema_version": 1, "channel": channel(),
        "messages": [{"message_id": "1.5", "thread_id": "1.5", "author": {"id": "U1", "name": "pat"},
                      "created_at": "2026-10-09T19:00:00Z", "text": "see attached", "file_ids": ["F11"]}],
        "files": [{"file_id": "F11", "name": "shot.png", "size_bytes": binary.len(), "sha256": sha(&binary),
                   "content_base64": encoded}],
        "complete": true});
    for json_mode in [false, true] {
        let hub = ScriptedHub::with(vec![read.clone()]);
        let (out, result) = run_cli(
            &[
                "read",
                "--channel",
                "cas-internal",
                "--since",
                "2026-10-09T00:00:00Z",
            ],
            &hub,
            json_mode,
            &no_gate,
        );
        result.unwrap();
        assert!(!out.contains(&encoded), "{out}");
        assert!(out.contains("F11"), "{out}");
        assert_eq!(hub.calls()[0].1["include_channels"], false);
    }
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

#[test]
fn read_requests_are_bounded_and_scoped() {
    let args = |since: Option<&str>, thread: Option<&str>| ReadArgs {
        channel: "#cas-internal".into(),
        since: since.map(str::to_string),
        thread: thread.map(str::to_string),
        message: None,
        cursor: None,
        max_messages: None,
    };
    assert_eq!(
        read_request(&args(None, None)).unwrap_err().code,
        "invalid_input"
    );
    assert_eq!(
        read_request(&args(Some("yesterday"), None))
            .unwrap_err()
            .code,
        "invalid_input"
    );
    let request = read_request(&args(Some("2026-10-09T00:00:00Z"), None)).unwrap();
    assert_eq!(
        request,
        json!({"channel": "#cas-internal", "include_channels": false, "since": "2026-10-09T00:00:00Z"})
    );
    let request = read_request(&args(None, Some("1710000000.000002"))).unwrap();
    assert_eq!(request["thread_id"], "1710000000.000002");
    let mut bad = args(Some("2026-10-09T00:00:00Z"), None);
    bad.max_messages = Some(501);
    assert_eq!(read_request(&bad).unwrap_err().code, "invalid_input");
}

#[test]
fn a_partial_read_names_its_cursor() {
    let hub = ScriptedHub::with(vec![
        json!({"ok": true, "channel": channel(), "messages": [],
        "files": [], "complete": false, "cursor": "opaque-cursor"}),
    ]);
    let (out, result) = run_cli(
        &[
            "read",
            "--channel",
            "cas-internal",
            "--since",
            "2026-10-09T00:00:00Z",
        ],
        &hub,
        false,
        &no_gate,
    );
    result.unwrap();
    assert!(out.contains("0 messages, partial"), "{out}");
    assert!(out.contains("--cursor opaque-cursor"), "{out}");
}

// ---------------------------------------------------------------------------
// Documentation
// ---------------------------------------------------------------------------

/// The violet skill documents the command and every code it raises itself,
/// so an agent can act on any failure without reading this source.
#[test]
fn every_local_code_is_documented_in_the_violet_skill() {
    let skill = include_str!("../../builtins/skills/violet/SKILL.md");
    let contract = include_str!("../../builtins/skills/violet/references/contract.md");
    assert!(
        skill.contains("cas violet post --channel"),
        "SKILL.md must point at the command"
    );
    assert!(
        skill.len() < 12_288,
        "violet SKILL.md is {} bytes",
        skill.len()
    );
    // pin: every error code the command raises must be documented for agents; the codes are string literals in these sources, so the doc-coverage check reads them.
    let sources = [
        include_str!("mod.rs"),
        include_str!("hub.rs"),
        include_str!("files.rs"),
    ];
    let pattern = regex::Regex::new(r#"VioletError::local\(\s*"([a-z_]+)""#).unwrap();
    let mut codes: Vec<&str> = sources
        .iter()
        .flat_map(|source| {
            pattern
                .captures_iter(source)
                .map(|c| c.get(1).unwrap().as_str())
        })
        .collect();
    codes.sort_unstable();
    codes.dedup();
    assert!(
        codes.len() >= 8,
        "expected the command's own codes, found {codes:?}"
    );
    for code in codes {
        assert!(
            contract.contains(&format!("`{code}`")),
            "violet references/contract.md does not document `cas violet` code {code:?}"
        );
    }
}
