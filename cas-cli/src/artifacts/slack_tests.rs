//! cassy#1148: posting a published artifact through Violet by id.
//!
//! Cloud, the blob store and Slack's upload URL are one wiremock double;
//! Violet is a scripted [`VioletPoster`] that records every call.

use super::*;
use cas_store::NewArtifact;
use std::sync::Mutex;
use tempfile::TempDir;
use wiremock::matchers::{body_bytes, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// sha256("hello world")
const HELLO_SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
const ART: &str = "art-1148aaaa";
const CLOUD_ID: &str = "cloud-1148";

/// A project store holding one record, committed to Cloud unless `status`
/// says otherwise.
fn store_with(dir: &TempDir, status: &str, sha256: &str, size_bytes: u64) -> SqliteArtifactStore {
    let store = SqliteArtifactStore::open(dir.path()).unwrap();
    store
        .record_local(&NewArtifact {
            id: ART.to_string(),
            task_id: "cas-7e51".to_string(),
            name: "report.pdf".to_string(),
            mime: "application/pdf".to_string(),
            size_bytes,
            sha256: sha256.to_string(),
        })
        .unwrap();
    if status != "local" {
        store.mark_uploaded(ART, CLOUD_ID).unwrap();
    }
    if status == "committed" {
        store.mark_committed(ART, None).unwrap();
    }
    store
}

/// Cloud answers the view URL; the blob store serves `blob`.
async fn cloud_serving(blob: &'static [u8]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/artifacts/{CLOUD_ID}/url")))
        .and(header("Authorization", "Bearer test-tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "artifact_id": CLOUD_ID,
            "url": format!("{}/blob/report.pdf?token=view-sig", server.uri()),
            "name": "report.pdf",
            "mime": "application/pdf",
            "size_bytes": 11
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/blob/report.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(blob))
        .mount(&server)
        .await;
    server
}

/// Resolve and download off the runtime, as the MCP handler does.
async fn fetch(
    server: Option<&MockServer>,
    status: &'static str,
    sha256: &'static str,
    id: &'static str,
) -> Result<(SignedView, Vec<u8>), PostError> {
    let endpoint = server.map(MockServer::uri);
    tokio::task::spawn_blocking(move || {
        let dir = TempDir::new().unwrap();
        let store = store_with(&dir, status, sha256, 11);
        let client = endpoint.map(|endpoint| ArtifactUploadClient::new(&endpoint, "test-tok"));
        fetch_verified(&store, client.as_ref(), id)
    })
    .await
    .unwrap()
}

/// Scripted Violet: answers each call with the next queued envelope.
struct ScriptedViolet {
    replies: Mutex<Vec<Value>>,
    calls: Mutex<Vec<Value>>,
}

impl ScriptedViolet {
    fn new(replies: Vec<Value>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().rev().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<Value> {
        self.calls.lock().unwrap().clone()
    }
}

impl VioletPoster for ScriptedViolet {
    fn violet_post(&self, args: Value) -> impl Future<Output = Result<Value, PostError>> + Send {
        self.calls.lock().unwrap().push(args);
        let reply = self.replies.lock().unwrap().pop();
        async move { reply.ok_or_else(|| PostError::new("test_unscripted", "no reply queued")) }
    }
}

/// The MCP tool-result wrapper Violet's envelope arrives in.
fn wrapped(envelope: Value) -> Value {
    json!({ "content": [{ "type": "text", "text": envelope.to_string() }], "isError": false })
}

fn begin_reply(server: &MockServer) -> Value {
    wrapped(json!({
        "ok": true, "schema_version": 1, "kind": "file_external", "step": "begin",
        "upload_url": format!("{}/slack/upload/F1148", server.uri()),
        "file_id": "F1148"
    }))
}

fn complete_reply(sha256: &str, verified: bool) -> Value {
    wrapped(json!({
        "ok": true, "schema_version": 1, "kind": "file",
        "channel": { "id": "C123", "name": "cas-internal" },
        "message": { "message_id": "1710000000.000003", "thread_id": null, "permalink": "https://slack.test/p/3" },
        "file": { "file_id": "F1148", "name": "report.pdf", "size_bytes": 11, "sha256": sha256,
                  "sha256_verified": verified, "state": "attached", "permalink": "https://slack.test/f/3" },
        "warning": null
    }))
}

fn options() -> PostOptions {
    PostOptions {
        channel: "cas-internal".to_string(),
        reply_to: Some("1710000000.000001".to_string()),
        title: Some("Release report".to_string()),
        initial_comment: None,
    }
}

#[tokio::test]
async fn a_committed_artifact_posts_by_id_and_returns_its_slack_receipt() {
    let server = cloud_serving(b"hello world").await;
    Mock::given(method("POST"))
        .and(path("/slack/upload/F1148"))
        .and(body_bytes(b"hello world".to_vec()))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let (view, bytes) = fetch(Some(&server), "committed", HELLO_SHA256, ART)
        .await
        .expect("the bytes match the published record");
    let violet = ScriptedViolet::new(vec![
        begin_reply(&server),
        complete_reply(HELLO_SHA256, true),
    ]);
    let receipt = post_verified(&violet, &view, bytes, &options())
        .await
        .expect("the post succeeds");

    assert_eq!(
        receipt,
        PostReceipt {
            artifact_id: ART.to_string(),
            channel_id: Some("C123".to_string()),
            message_id: Some("1710000000.000003".to_string()),
            file_id: "F1148".to_string(),
            permalink: "https://slack.test/p/3".to_string(),
            size_bytes: 11,
            sha256: HELLO_SHA256.to_string(),
            sha256_verified: true,
        }
    );
    let calls = violet.calls();
    assert_eq!(calls.len(), 2);
    for (call, step) in calls.iter().zip(["begin", "complete"]) {
        assert_eq!(call["kind"], "file_external");
        assert_eq!(call["step"], step);
        assert_eq!(call["channel"], "cas-internal");
        assert_eq!(call["filename"], "report.pdf");
        assert_eq!(call["size_bytes"], 11);
        assert_eq!(call["sha256"], HELLO_SHA256);
        assert_eq!(call["reply_to"], "1710000000.000001");
        assert_eq!(call["title"], "Release report");
        assert!(
            call.get("initial_comment").is_none(),
            "unset options are omitted"
        );
        let text = call.to_string();
        assert!(
            !text.contains("view-sig")
                && !text.contains("/slack/upload")
                && !text.contains("hello world"),
            "no URL or byte enters a Violet payload: {text}"
        );
    }
    assert!(calls[0].get("file_id").is_none());
    assert_eq!(calls[1]["file_id"], "F1148");
}

#[tokio::test]
async fn an_unverified_slack_copy_is_reported_not_hidden() {
    let server = cloud_serving(b"hello world").await;
    Mock::given(method("POST"))
        .and(path("/slack/upload/F1148"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (view, bytes) = fetch(Some(&server), "committed", HELLO_SHA256, ART)
        .await
        .unwrap();
    let violet = ScriptedViolet::new(vec![
        begin_reply(&server),
        complete_reply(HELLO_SHA256, false),
    ]);
    let receipt = post_verified(&violet, &view, bytes, &options())
        .await
        .unwrap();
    assert!(!receipt.sha256_verified);
}

#[tokio::test]
async fn a_checksum_mismatch_is_refused_before_anything_reaches_slack() {
    let server = cloud_serving(b"hello world").await;
    let recorded: &'static str = "0000000000000000000000000000000000000000000000000000000000000000";
    let error = fetch(Some(&server), "committed", recorded, ART)
        .await
        .expect_err("tampered or stale bytes are refused");
    assert_eq!(error.code, "artifact_checksum_mismatch");
    assert!(error.message.contains(HELLO_SHA256), "{error}");
    assert!(error.file_id.is_none());
}

#[tokio::test]
async fn a_short_download_is_a_size_mismatch() {
    let server = cloud_serving(b"hello").await;
    let error = fetch(Some(&server), "committed", HELLO_SHA256, ART)
        .await
        .unwrap_err();
    assert_eq!(error.code, "artifact_size_mismatch", "{error}");
}

#[tokio::test]
async fn a_missing_artifact_is_named() {
    let server = cloud_serving(b"hello world").await;
    let unknown = fetch(Some(&server), "committed", HELLO_SHA256, "art-nope")
        .await
        .unwrap_err();
    assert_eq!(unknown.code, "artifact_not_found", "{unknown}");
    assert!(unknown.message.contains("art-nope"), "{unknown}");

    let blank = fetch(Some(&server), "committed", HELLO_SHA256, "  ")
        .await
        .unwrap_err();
    assert_eq!(blank.code, "artifact_id_missing");

    // Known locally, but Cloud has no such artifact for this account.
    let gone = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/artifacts/{CLOUD_ID}/url")))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"error": "Artifact not found"})),
        )
        .mount(&gone)
        .await;
    let cloud = fetch(Some(&gone), "committed", HELLO_SHA256, ART)
        .await
        .unwrap_err();
    assert_eq!(cloud.code, "artifact_not_found", "{cloud}");
}

#[tokio::test]
async fn an_artifact_never_committed_to_cloud_is_named() {
    let server = cloud_serving(b"hello world").await;
    let error = fetch(Some(&server), "local", HELLO_SHA256, ART)
        .await
        .unwrap_err();
    assert_eq!(error.code, "artifact_not_in_cloud", "{error}");
}

#[tokio::test]
async fn missing_cloud_auth_is_named() {
    let error = fetch(None, "committed", HELLO_SHA256, ART)
        .await
        .unwrap_err();
    assert_eq!(error.code, "cloud_auth_missing", "{error}");
    assert!(error.message.contains("cas cloud login"), "{error}");
}

#[tokio::test]
async fn a_rejected_cloud_bearer_is_a_resolve_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/artifacts/{CLOUD_ID}/url")))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "unauthorized"})))
        .mount(&server)
        .await;
    let error = fetch(Some(&server), "committed", HELLO_SHA256, ART)
        .await
        .unwrap_err();
    assert_eq!(error.code, "cloud_resolve_failed", "{error}");
    assert!(error.message.contains("401"), "{error}");
}

#[tokio::test]
async fn a_missing_channel_is_refused_before_violet_is_called() {
    let server = cloud_serving(b"hello world").await;
    let (view, bytes) = fetch(Some(&server), "committed", HELLO_SHA256, ART)
        .await
        .unwrap();
    let violet = ScriptedViolet::new(vec![]);
    let mut options = options();
    options.channel = " ".to_string();
    let error = post_verified(&violet, &view, bytes, &options)
        .await
        .unwrap_err();
    assert_eq!(error.code, "artifact_channel_missing");
    assert!(violet.calls().is_empty());
}

#[tokio::test]
async fn violets_refusal_keeps_its_code_and_the_allocated_upload() {
    let server = cloud_serving(b"hello world").await;
    Mock::given(method("POST"))
        .and(path("/slack/upload/F1148"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (view, bytes) = fetch(Some(&server), "committed", HELLO_SHA256, ART)
        .await
        .unwrap();
    let refusal = wrapped(json!({
        "ok": false,
        "error": { "code": "file_integrity_mismatch", "message": "sha256 differs", "retryable": false }
    }));
    let violet = ScriptedViolet::new(vec![begin_reply(&server), refusal]);
    let error = post_verified(&violet, &view, bytes, &options())
        .await
        .unwrap_err();
    assert_eq!(error.code, "file_integrity_mismatch");
    assert_eq!(error.file_id.as_deref(), Some("F1148"));
    assert!(error.to_string().contains("check the channel"), "{error}");
}

#[tokio::test]
async fn a_failed_byte_upload_never_completes() {
    let server = cloud_serving(b"hello world").await;
    Mock::given(method("POST"))
        .and(path("/slack/upload/F1148"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let (view, bytes) = fetch(Some(&server), "committed", HELLO_SHA256, ART)
        .await
        .unwrap();
    let violet = ScriptedViolet::new(vec![begin_reply(&server)]);
    let error = post_verified(&violet, &view, bytes, &options())
        .await
        .unwrap_err();
    assert_eq!(error.code, "violet_upload_failed", "{error}");
    assert_eq!(error.file_id.as_deref(), Some("F1148"));
    assert_eq!(
        violet.calls().len(),
        1,
        "complete is not called after a failed upload"
    );
}

#[tokio::test]
async fn a_slack_copy_that_differs_from_the_artifact_is_refused() {
    let server = cloud_serving(b"hello world").await;
    Mock::given(method("POST"))
        .and(path("/slack/upload/F1148"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (view, bytes) = fetch(Some(&server), "committed", HELLO_SHA256, ART)
        .await
        .unwrap();
    let violet = ScriptedViolet::new(vec![
        begin_reply(&server),
        complete_reply(&"f".repeat(64), true),
    ]);
    let error = post_verified(&violet, &view, bytes, &options())
        .await
        .unwrap_err();
    assert_eq!(error.code, "slack_integrity_mismatch");
    assert_eq!(error.file_id.as_deref(), Some("F1148"));
}

#[test]
fn envelopes_decode_from_every_wrapper() {
    let bare = json!({"ok": true, "file_id": "F1"});
    assert_eq!(envelope(bare.clone()).unwrap(), bare);
    assert_eq!(envelope(wrapped(bare.clone())).unwrap(), bare);
    assert_eq!(
        envelope(json!({"structuredContent": bare.clone(), "content": []})).unwrap(),
        bare
    );
    let refused = envelope(json!({"ok": false})).unwrap_err();
    assert_eq!(refused.code, "violet_post_failed");
    let garbage = envelope(json!({"content": [{"type": "text", "text": "not json"}]})).unwrap_err();
    assert_eq!(garbage.code, "violet_invalid_receipt");
}

#[test]
fn only_https_or_loopback_urls_are_followed() {
    assert!(safe_url("https://files.slack.com/upload/v1/abc"));
    assert!(safe_url("http://127.0.0.1:9000/x"));
    assert!(!safe_url("http://files.slack.com/upload"));
    assert!(!safe_url("https://user:pw@files.slack.com/upload"));
    assert!(!safe_url("file:///etc/passwd"));
}
