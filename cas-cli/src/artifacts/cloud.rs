//! Cloud client for the three-step artifact upload (petra-stella-cloud#84).
//!
//! `begin` → streaming `PUT` to the returned upload URL → `complete`.
//!
//! Two properties this module exists to hold:
//!
//! * **The bearer never leaves Cassy's own origin.** The URL `begin` returns
//!   points at an object store, is itself a credential, and must be treated as
//!   one: no `Authorization` header is attached to the `PUT`, the URL is never
//!   logged, stored, or included in a failure report, and it is checked to be
//!   `https` (or loopback, so tests can run) before a single byte is sent.
//! * **A failure names its step.** A partial upload is ambiguous unless the
//!   report says which of the three calls failed and what the server actually
//!   said, so [`UploadFailure`] carries both.
//!
//! The wire contract below is this client's half of #84. Until that endpoint
//! ships, `begin` answering 404 or 501 is not an error: it is the documented
//! "storage is not live yet" boundary, and the caller keeps the artifact local.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Read;
use std::time::Duration;

/// Matches the other cloud clients in this module tree.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// The upload itself is a file transfer, not an API round trip.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Which of the three calls a failure came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadStep {
    Begin,
    Upload,
    Complete,
}

impl fmt::Display for UploadStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UploadStep::Begin => "begin",
            UploadStep::Upload => "upload",
            UploadStep::Complete => "complete",
        })
    }
}

/// What the server was told about the bytes, and what it says it stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArtifactDigest {
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

impl fmt::Display for ArtifactDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.sha256, self.size_bytes) {
            (Some(sha), Some(size)) => write!(f, "{sha} ({size} bytes)"),
            (Some(sha), None) => write!(f, "{sha} (size not stated)"),
            (None, Some(size)) => write!(f, "digest not stated ({size} bytes)"),
            (None, None) => f.write_str("not stated"),
        }
    }
}

/// Why an upload did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadFailure {
    /// `begin` answered 404/501: this deployment has no artifact storage.
    /// A boundary to report, not an error to alarm about.
    NotLive { reason: String },
    /// `complete` answered 409: what the server stored is not what we declared.
    Mismatch {
        declared: ArtifactDigest,
        stored: ArtifactDigest,
        interaction: String,
    },
    /// Anything else, attributed to its step.
    Failed {
        step: UploadStep,
        reason: String,
        /// The exact request/response, for a bug report against the cloud
        /// server — which lives in a different repository.
        interaction: String,
    },
}

impl UploadFailure {
    /// True when the local record should simply stay `local` and the command
    /// should still succeed.
    pub fn is_boundary(&self) -> bool {
        matches!(self, UploadFailure::NotLive { .. })
    }

    fn failed(step: UploadStep, reason: impl Into<String>, interaction: impl Into<String>) -> Self {
        UploadFailure::Failed {
            step,
            reason: reason.into(),
            interaction: interaction.into(),
        }
    }
}

impl fmt::Display for UploadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UploadFailure::NotLive { reason } => write!(f, "{reason}"),
            UploadFailure::Mismatch {
                declared,
                stored,
                interaction,
            } => write!(
                f,
                "the server refused the completion: it stored {stored}, we declared {declared}. \
                 The artifact stays local and is not referenced as uploaded.\n  \
                 Failing interaction: {interaction}"
            ),
            UploadFailure::Failed {
                step,
                reason,
                interaction,
            } => write!(
                f,
                "artifact {step} failed: {reason}\n  Failing interaction: {interaction}"
            ),
        }
    }
}

impl std::error::Error for UploadFailure {}

/// What `begin` is told about the file.
#[derive(Debug, Clone, Serialize)]
pub struct BeginRequest {
    pub task_id: String,
    pub name: String,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// What `begin` answers.
#[derive(Clone, Deserialize)]
pub struct BeginResponse {
    /// The server's id for this artifact; the local row records it.
    pub artifact_id: String,
    /// Pre-signed, short-lived, credential-bearing. Never persisted or logged.
    pub upload_url: String,
    /// Headers the object store requires on the PUT (e.g. `x-amz-acl`).
    #[serde(default)]
    pub required_headers: std::collections::BTreeMap<String, String>,
}

/// What `complete` answers.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CompleteResponse {
    /// Durable location, safe to persist. Absent while the server keeps the
    /// object private.
    #[serde(default)]
    pub url: Option<String>,
}

/// Blocking client for the artifact endpoints, matching the rest of `cloud/`.
#[derive(Clone)]
pub struct ArtifactUploadClient {
    endpoint: String,
    token: String,
    timeout: Duration,
    upload_timeout: Duration,
}

// The derived Debug would print the bearer token; every other cloud client in
// this tree has that defect and this one does not.
impl fmt::Debug for ArtifactUploadClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArtifactUploadClient")
            .field("endpoint", &self.endpoint)
            .field("token", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl ArtifactUploadClient {
    /// Endpoint and token are parameters so tests can point the client at a
    /// mock server without touching process environment.
    pub fn new(endpoint: &str, token: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token: token.to_string(),
            timeout: DEFAULT_TIMEOUT,
            upload_timeout: UPLOAD_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.upload_timeout = timeout;
        self
    }

    /// Reserve an artifact id and get a place to put the bytes.
    pub fn begin(&self, request: &BeginRequest) -> Result<BeginResponse, UploadFailure> {
        let url = format!("{}/api/artifacts/begin", self.endpoint);
        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .timeout(self.timeout)
            .send_json(request);

        let (status, body) = match classify(response) {
            Ok(ok) => ok,
            Err(transport) => {
                return Err(UploadFailure::failed(
                    UploadStep::Begin,
                    transport,
                    format!("POST {url}"),
                ));
            }
        };

        if status == 404 || status == 501 {
            return Err(UploadFailure::NotLive {
                reason: format!(
                    "Cloud artifact storage is not live on this endpoint yet (POST /api/artifacts/begin answered {status})"
                ),
            });
        }
        if !(200..300).contains(&status) {
            return Err(UploadFailure::failed(
                UploadStep::Begin,
                format!("the server answered {status}"),
                format!("POST {url} -> {status}: {}", body_excerpt(&body)),
            ));
        }

        let begun: BeginResponse = serde_json::from_str(&body).map_err(|error| {
            UploadFailure::failed(
                UploadStep::Begin,
                format!("could not read the server's response: {error}"),
                format!("POST {url} -> {status}: {}", body_excerpt(&body)),
            )
        })?;
        if begun.artifact_id.trim().is_empty() {
            return Err(UploadFailure::failed(
                UploadStep::Begin,
                "the server returned no artifact id",
                format!("POST {url} -> {status}"),
            ));
        }
        check_upload_url(&begun.upload_url)?;
        Ok(begun)
    }

    /// Stream `body` to the pre-signed URL. No Cassy credential is attached:
    /// the URL carries its own authority and belongs to a different origin.
    pub fn upload(
        &self,
        begun: &BeginResponse,
        mime: &str,
        size_bytes: u64,
        body: impl Read,
    ) -> Result<(), UploadFailure> {
        check_upload_url(&begun.upload_url)?;
        let mut request = ureq::put(&begun.upload_url)
            .set("Content-Type", mime)
            // ureq streams chunked unless the length is declared; object
            // stores reject a chunked pre-signed PUT.
            .set("Content-Length", &size_bytes.to_string())
            .timeout(self.upload_timeout);
        for (name, value) in &begun.required_headers {
            request = request.set(name, value);
        }

        // The URL is a credential: the interaction line names the step and the
        // artifact, never the location.
        let redacted = format!("PUT <upload url for artifact {}>", begun.artifact_id);
        let (status, response_body) = match classify(request.send(body)) {
            Ok(ok) => ok,
            Err(transport) => {
                return Err(UploadFailure::failed(
                    UploadStep::Upload,
                    transport,
                    redacted,
                ));
            }
        };
        if !(200..300).contains(&status) {
            let hint = if status == 403 {
                " (the upload URL may have expired; publish again to get a fresh one)"
            } else {
                ""
            };
            return Err(UploadFailure::failed(
                UploadStep::Upload,
                format!("the object store answered {status}{hint}"),
                format!("{redacted} -> {status}: {}", body_excerpt(&response_body)),
            ));
        }
        Ok(())
    }

    /// Ask the server to verify and adopt the object.
    pub fn complete(
        &self,
        artifact_id: &str,
        declared: &ArtifactDigest,
    ) -> Result<CompleteResponse, UploadFailure> {
        let url = format!("{}/api/artifacts/{artifact_id}/complete", self.endpoint);
        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .timeout(self.timeout)
            .send_json(declared);

        let (status, body) = match classify(response) {
            Ok(ok) => ok,
            Err(transport) => {
                return Err(UploadFailure::failed(
                    UploadStep::Complete,
                    transport,
                    format!("POST {url}"),
                ));
            }
        };

        if status == 409 {
            return Err(UploadFailure::Mismatch {
                declared: declared.clone(),
                stored: stored_digest(&body),
                interaction: format!("POST {url} -> 409: {}", body_excerpt(&body)),
            });
        }
        if !(200..300).contains(&status) {
            return Err(UploadFailure::failed(
                UploadStep::Complete,
                format!("the server answered {status}"),
                format!("POST {url} -> {status}: {}", body_excerpt(&body)),
            ));
        }

        // A completion with an unreadable body still committed the object;
        // treat the missing URL as "private", not as a failure.
        Ok(serde_json::from_str(&body).unwrap_or_default())
    }
}

/// Normalise ureq's split of non-2xx between `Ok` and `Err(Status)` into one
/// `(status, body)` pair. `Err` here means the request never got a response.
fn classify(response: Result<ureq::Response, ureq::Error>) -> Result<(u16, String), String> {
    match response {
        Ok(resp) => {
            let status = resp.status();
            Ok((status, resp.into_string().unwrap_or_default()))
        }
        Err(ureq::Error::Status(status, resp)) => {
            Ok((status, resp.into_string().unwrap_or_default()))
        }
        Err(ureq::Error::Transport(error)) => Err(format!("network error: {error}")),
    }
}

/// Server bodies can be full HTML error pages; keep the excerpt readable.
fn body_excerpt(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "<empty body>".to_string();
    }
    if trimmed.chars().count() <= 300 {
        return trimmed.replace('\n', " ");
    }
    let head: String = trimmed.chars().take(300).collect();
    format!("{}…", head.replace('\n', " "))
}

/// Pull the server's view of the stored object out of a 409 body, accepting
/// both a nested `stored` object and flat fields.
fn stored_digest(body: &str) -> ArtifactDigest {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return ArtifactDigest::default();
    };
    let scope = value.get("stored").unwrap_or(&value);
    ArtifactDigest {
        sha256: scope
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        size_bytes: scope.get("size_bytes").and_then(serde_json::Value::as_u64),
    }
}

/// The server hands back a URL we then send bytes to; that URL is not covered
/// by the endpoint allowlist, so it is checked here. Loopback `http` is
/// permitted so a mock server can stand in, matching `is_acceptable_endpoint`.
fn check_upload_url(url: &str) -> Result<(), UploadFailure> {
    let lowered = url.trim().to_ascii_lowercase();
    let acceptable = lowered.starts_with("https://")
        || lowered.starts_with("http://127.0.0.1")
        || lowered.starts_with("http://localhost")
        || lowered.starts_with("http://0.0.0.0");
    if acceptable {
        return Ok(());
    }
    Err(UploadFailure::failed(
        UploadStep::Begin,
        "the server returned an upload location that is not https",
        // Deliberately not the URL: it may carry a signature.
        format!(
            "upload URL scheme {}",
            lowered
                .split_once("://")
                .map(|(s, _)| s)
                .unwrap_or("<none>")
        ),
    ))
}

// The upload URL is a short-lived credential; the derived Debug would print it
// into any error chain or tracing field that formats this response.
impl fmt::Debug for BeginResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BeginResponse")
            .field("artifact_id", &self.artifact_id)
            .field("upload_url", &"[redacted]")
            .field("required_headers", &self.required_headers)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn begin_request() -> BeginRequest {
        BeginRequest {
            task_id: "cas-b72a".to_string(),
            name: "brief.pdf".to_string(),
            mime: "application/pdf".to_string(),
            size_bytes: 11,
            sha256: "c".repeat(64),
        }
    }

    #[tokio::test]
    async fn the_three_steps_succeed_and_the_bearer_never_reaches_the_object_store() {
        let server = MockServer::start().await;
        let upload_url = format!("{}/object/put?X-Amz-Signature=deadbeef", server.uri());

        Mock::given(method("POST"))
            .and(path("/api/artifacts/begin"))
            .and(header("Authorization", "Bearer test-tok"))
            .and(body_string_contains("cas-b72a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "artifact_id": "cloud-42",
                "upload_url": upload_url,
                "required_headers": { "x-amz-acl": "private" }
            })))
            .expect(1)
            .mount(&server)
            .await;

        // The absence matcher is the assertion: a PUT carrying Authorization
        // does not match this mock, so the test fails on an unmatched request.
        Mock::given(method("PUT"))
            .and(path("/object/put"))
            .and(header("content-type", "application/pdf"))
            .and(header("content-length", "11"))
            .and(header("x-amz-acl", "private"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/artifacts/cloud-42/complete"))
            .and(header("Authorization", "Bearer test-tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "url": "https://cloud.example/a/cloud-42"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let endpoint = server.uri();
        let outcome = tokio::task::spawn_blocking(move || {
            let client = ArtifactUploadClient::new(&endpoint, "test-tok");
            let begun = client.begin(&begin_request())?;
            client.upload(&begun, "application/pdf", 11, Cursor::new(b"hello world"))?;
            let completed = client.complete(
                &begun.artifact_id,
                &ArtifactDigest {
                    sha256: Some("c".repeat(64)),
                    size_bytes: Some(11),
                },
            )?;
            Ok::<_, UploadFailure>((begun.artifact_id, completed.url))
        })
        .await
        .unwrap()
        .expect("the happy path must succeed");

        assert_eq!(outcome.0, "cloud-42");
        assert_eq!(
            outcome.1.as_deref(),
            Some("https://cloud.example/a/cloud-42")
        );

        let uploads = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/object/put")
            .collect::<Vec<_>>();
        assert_eq!(uploads.len(), 1, "exactly one PUT");
        assert!(
            uploads[0].headers.get("authorization").is_none(),
            "the Cassy bearer must never be sent to the object store"
        );
        assert_eq!(
            uploads[0].body, b"hello world",
            "the streamed body must arrive intact"
        );
    }

    #[tokio::test]
    async fn a_404_from_begin_is_a_boundary_not_a_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/artifacts/begin"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .expect(1)
            .mount(&server)
            .await;

        let endpoint = server.uri();
        let failure = tokio::task::spawn_blocking(move || {
            ArtifactUploadClient::new(&endpoint, "test-tok")
                .begin(&begin_request())
                .unwrap_err()
        })
        .await
        .unwrap();

        assert!(
            failure.is_boundary(),
            "404 means storage is not live yet: {failure}"
        );
        assert!(
            failure.to_string().contains("not live"),
            "the message must say so plainly: {failure}"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn a_501_from_begin_is_also_a_boundary() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/artifacts/begin"))
            .respond_with(ResponseTemplate::new(501))
            .mount(&server)
            .await;

        let endpoint = server.uri();
        let failure = tokio::task::spawn_blocking(move || {
            ArtifactUploadClient::new(&endpoint, "t")
                .begin(&begin_request())
                .unwrap_err()
        })
        .await
        .unwrap();
        assert!(failure.is_boundary(), "{failure}");
    }

    #[tokio::test]
    async fn a_500_from_begin_is_a_failure_attributed_to_its_step() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/artifacts/begin"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let endpoint = server.uri();
        let failure = tokio::task::spawn_blocking(move || {
            ArtifactUploadClient::new(&endpoint, "t")
                .begin(&begin_request())
                .unwrap_err()
        })
        .await
        .unwrap();

        assert!(!failure.is_boundary());
        assert!(matches!(
            failure,
            UploadFailure::Failed {
                step: UploadStep::Begin,
                ..
            }
        ));
        assert!(failure.to_string().contains("boom"), "{failure}");
    }

    #[tokio::test]
    async fn a_409_at_complete_reports_declared_against_stored() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/artifacts/cloud-42/complete"))
            .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
                "error": "digest_mismatch",
                "stored": { "sha256": "d".repeat(64), "size_bytes": 9 }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let endpoint = server.uri();
        let failure = tokio::task::spawn_blocking(move || {
            ArtifactUploadClient::new(&endpoint, "t")
                .complete(
                    "cloud-42",
                    &ArtifactDigest {
                        sha256: Some("c".repeat(64)),
                        size_bytes: Some(11),
                    },
                )
                .unwrap_err()
        })
        .await
        .unwrap();

        let UploadFailure::Mismatch {
            declared, stored, ..
        } = &failure
        else {
            panic!("expected a mismatch, got {failure}");
        };
        assert_eq!(declared.size_bytes, Some(11));
        assert_eq!(stored.size_bytes, Some(9));
        assert_eq!(stored.sha256.as_deref(), Some("d".repeat(64).as_str()));
        let rendered = failure.to_string();
        assert!(
            rendered.contains("stays local"),
            "the operator must be told the record did not advance: {rendered}"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn an_expired_upload_url_is_reported_as_expired() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/object/put"))
            .respond_with(ResponseTemplate::new(403).set_body_string("<Error>Expired</Error>"))
            .expect(1)
            .mount(&server)
            .await;

        let upload_url = format!("{}/object/put?X-Amz-Signature=abc", server.uri());
        let failure = tokio::task::spawn_blocking(move || {
            let begun = BeginResponse {
                artifact_id: "cloud-42".to_string(),
                upload_url,
                required_headers: Default::default(),
            };
            ArtifactUploadClient::new("http://127.0.0.1:1", "t")
                .upload(&begun, "application/pdf", 4, Cursor::new(b"data"))
                .unwrap_err()
        })
        .await
        .unwrap();

        assert!(matches!(
            failure,
            UploadFailure::Failed {
                step: UploadStep::Upload,
                ..
            }
        ));
        let rendered = failure.to_string();
        assert!(rendered.contains("expired"), "{rendered}");
        assert!(
            !rendered.contains("X-Amz-Signature"),
            "the signed URL must never appear in a failure report: {rendered}"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn a_non_https_upload_location_is_refused_before_any_bytes_are_sent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/artifacts/begin"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "artifact_id": "cloud-42",
                "upload_url": "http://evil.example/put?X-Amz-Signature=abc"
            })))
            .mount(&server)
            .await;

        let endpoint = server.uri();
        let failure = tokio::task::spawn_blocking(move || {
            ArtifactUploadClient::new(&endpoint, "t")
                .begin(&begin_request())
                .unwrap_err()
        })
        .await
        .unwrap();

        let rendered = failure.to_string();
        assert!(rendered.contains("not https"), "{rendered}");
        assert!(
            !rendered.contains("evil.example") && !rendered.contains("Signature"),
            "a rejected URL is still a credential: {rendered}"
        );
    }

    #[test]
    fn the_client_debug_form_never_prints_the_token() {
        let rendered = format!(
            "{:?}",
            ArtifactUploadClient::new("https://cloud.example", "super-secret-bearer")
        );
        assert!(
            !rendered.contains("super-secret-bearer"),
            "Debug must redact the bearer: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
        assert!(
            rendered.contains("https://cloud.example"),
            "the endpoint is diagnostic, not secret: {rendered}"
        );
    }

    #[test]
    fn a_409_body_without_a_stored_block_still_yields_a_report() {
        let digest = stored_digest("{\"error\":\"digest_mismatch\"}");
        assert_eq!(digest, ArtifactDigest::default());
        assert_eq!(digest.to_string(), "not stated");

        let flat = stored_digest("{\"sha256\":\"ab\",\"size_bytes\":3}");
        assert_eq!(flat.sha256.as_deref(), Some("ab"));
        assert_eq!(flat.size_bytes, Some(3));

        assert_eq!(stored_digest("not json"), ArtifactDigest::default());
    }

    #[test]
    fn long_server_bodies_are_excerpted() {
        assert_eq!(body_excerpt("   "), "<empty body>");
        assert_eq!(body_excerpt("a\nb"), "a b");
        let long = "x".repeat(400);
        let excerpt = body_excerpt(&long);
        assert!(excerpt.ends_with('…') && excerpt.chars().count() == 301);
    }

    #[test]
    fn loopback_upload_locations_are_permitted_for_tests_only() {
        assert!(check_upload_url("https://store.example/o").is_ok());
        assert!(check_upload_url("http://127.0.0.1:8080/o").is_ok());
        assert!(check_upload_url("http://localhost:8080/o").is_ok());
        assert!(check_upload_url("http://store.example/o").is_err());
        assert!(check_upload_url("ftp://store.example/o").is_err());
    }
}

#[cfg(test)]
mod credential_redaction_tests {
    use super::*;

    #[test]
    fn the_begin_response_debug_never_prints_the_upload_url() {
        let response = BeginResponse {
            artifact_id: "cloud-42".to_string(),
            upload_url: "https://store.example/o?X-Amz-Signature=SECRET-tok-9f3a1c".to_string(),
            required_headers: Default::default(),
        };
        let rendered = format!("{response:?}");
        assert!(!rendered.contains("SECRET-tok-9f3a1c"), "{rendered}");
        assert!(rendered.contains("[redacted]"), "{rendered}");
        assert!(
            rendered.contains("cloud-42"),
            "non-secret fields stay useful: {rendered}"
        );
    }
}
