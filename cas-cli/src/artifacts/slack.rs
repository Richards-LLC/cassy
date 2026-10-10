//! Posting a published Cloud artifact to Slack through Violet (cassy#1148).
//!
//! The caller names an artifact record id and a channel; nothing else. This
//! module then, in order:
//!
//! 1. Resolves the record through Cloud with the installation's existing
//!    bearer ([`signed_view`], `GET /api/artifacts/{id}/url`).
//! 2. Downloads the bytes from the signed URL and checks their size and
//!    SHA-256 against what `cas artifact publish` recorded. A mismatch stops
//!    here, before anything reaches Slack.
//! 3. Uploads through Violet's `file_external` route: `begin` with the
//!    recorded metadata, the bytes sent straight to the returned `upload_url`,
//!    then `complete` with the same metadata and the `file_id`.
//!
//! Bytes, signed URLs and upload URLs never enter an MCP payload or an error
//! message: each is a short-lived capability.

use super::{SignedView, ViewError, cloud::ArtifactUploadClient, cloud::ViewFailure, signed_view};
use cas_store::SqliteArtifactStore;
use cas_types::ARTIFACT_MAX_BYTES;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::io::Read;
use std::time::Duration;

/// The hub hashes the Slack copy only up to this size; above it a receipt's
/// `sha256_verified: false` is expected rather than a warning sign.
pub const HUB_HASH_LIMIT: u64 = 16 * 1024 * 1024;

/// Where and how to post. `channel` is required; the rest are passed through
/// to Violet unchanged.
#[derive(Debug, Clone, Default)]
pub struct PostOptions {
    pub channel: String,
    pub reply_to: Option<String>,
    pub title: Option<String>,
    pub initial_comment: Option<String>,
}

/// What a successful post returns, for the release-report flow to record.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PostReceipt {
    pub artifact_id: String,
    pub channel_id: Option<String>,
    pub message_id: Option<String>,
    pub file_id: String,
    pub permalink: String,
    pub size_bytes: u64,
    pub sha256: String,
    /// Whether Violet hashed the Slack copy. Cassy's own source check has
    /// already passed; this is about the copy Slack holds.
    pub sha256_verified: bool,
}

/// A named failure. `code` is stable; `message` says what to do.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PostError {
    pub code: String,
    pub message: String,
    /// Set once `begin` allocated an upload: the post may be partly done, so
    /// the caller inspects Slack before trying again.
    pub file_id: Option<String>,
}

impl PostError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            file_id: None,
        }
    }
}

impl std::fmt::Display for PostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(file_id) = &self.file_id {
            write!(
                f,
                " (Slack upload {file_id} was allocated; check the channel before posting again)"
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for PostError {}

/// One `violet_post` call. The production implementation dispatches through
/// the MCP proxy with the session's registered caller, so the proxy's policy
/// applies exactly as it does to a direct `violet_post`.
pub trait VioletPoster {
    fn violet_post(&self, args: Value) -> impl Future<Output = Result<Value, PostError>> + Send;
}

/// Resolve and download a committed artifact, refusing bytes that differ from
/// the published record. Blocking: run it off the async runtime.
pub fn fetch_verified(
    store: &SqliteArtifactStore,
    cloud: Option<&ArtifactUploadClient>,
    id: &str,
) -> Result<(SignedView, Vec<u8>), PostError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(PostError::new(
            "artifact_id_missing",
            "artifact post requires 'id', the art-… record id from publish",
        ));
    }
    let view = signed_view(store, cloud, id).map_err(resolve_error)?;
    let bytes = download(&view)?;
    Ok((view, bytes))
}

fn resolve_error(error: ViewError) -> PostError {
    match error {
        ViewError::Unknown(id) => PostError::new(
            "artifact_not_found",
            format!(
                "no published artifact {id} in this project; use the id `artifact publish` returned"
            ),
        ),
        ViewError::NotLoggedIn => PostError::new(
            "cloud_auth_missing",
            "this installation is not logged in to Cassy Cloud, so the artifact cannot be resolved; run `cas cloud login`",
        ),
        ViewError::NotInCloud { status } => PostError::new(
            "artifact_not_in_cloud",
            format!(
                "the artifact is {status}, not committed to Cloud; publish it again while logged in"
            ),
        ),
        ViewError::Cloud(ViewFailure::NotFound { .. }) => PostError::new(
            "artifact_not_found",
            "Cloud has no artifact by that id for this account",
        ),
        ViewError::Cloud(failure) => PostError::new("cloud_resolve_failed", failure.to_string()),
        ViewError::Store(detail) => PostError::new(
            "artifact_store_failed",
            format!("could not read the artifact record: {detail}"),
        ),
    }
}

/// Signed URLs carry no Cassy credential, so they get a plain agent.
fn byte_agent(redirects: u32) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(redirects)
        .timeout(Duration::from_secs(60))
        .build()
}

/// `https`, or loopback `http` so a test double can stand in.
pub(crate) fn safe_url(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|url| {
        url.username().is_empty()
            && url.password().is_none()
            && (url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))))
    })
}

fn download(view: &SignedView) -> Result<Vec<u8>, PostError> {
    let artifact = &view.artifact;
    if artifact.size_bytes == 0 || artifact.size_bytes > ARTIFACT_MAX_BYTES {
        return Err(PostError::new(
            "artifact_size_invalid",
            format!(
                "the record says {} bytes, outside 1..={ARTIFACT_MAX_BYTES}",
                artifact.size_bytes
            ),
        ));
    }
    if view
        .view
        .size_bytes
        .is_some_and(|size| size != artifact.size_bytes)
    {
        return Err(PostError::new(
            "artifact_size_mismatch",
            format!(
                "Cloud reports {} bytes but the published record says {}; nothing was posted",
                view.view.size_bytes.unwrap_or_default(),
                artifact.size_bytes
            ),
        ));
    }
    if !safe_url(&view.view.url) {
        return Err(PostError::new(
            "cloud_resolve_failed",
            "Cloud returned a download location that is not https",
        ));
    }
    let failed = |detail: String| {
        PostError::new(
            "artifact_download_failed",
            format!("could not download the artifact from its signed URL: {detail}"),
        )
    };
    let response = match byte_agent(3).get(&view.view.url).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(status, _)) => {
            return Err(failed(format!("the store answered {status}")));
        }
        Err(ureq::Error::Transport(_)) => return Err(failed("network error".to_string())),
    };
    let mut bytes = Vec::with_capacity(artifact.size_bytes as usize);
    response
        .into_reader()
        .take(artifact.size_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failed("the transfer was interrupted".to_string()))?;
    if bytes.len() as u64 != artifact.size_bytes {
        return Err(PostError::new(
            "artifact_size_mismatch",
            format!(
                "downloaded {} bytes but the published record says {}; nothing was posted",
                bytes.len(),
                artifact.size_bytes
            ),
        ));
    }
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(PostError::new(
            "artifact_checksum_mismatch",
            format!(
                "downloaded SHA-256 {actual} differs from the published {}; nothing was posted",
                artifact.sha256
            ),
        ));
    }
    Ok(bytes)
}

/// Upload already-verified bytes through Violet's `file_external` route.
pub async fn post_verified(
    violet: &(impl VioletPoster + Sync),
    view: &SignedView,
    bytes: Vec<u8>,
    options: &PostOptions,
) -> Result<PostReceipt, PostError> {
    let channel = options.channel.trim();
    if channel.is_empty() {
        return Err(PostError::new(
            "artifact_channel_missing",
            "artifact post requires 'channel', a Slack channel name or id",
        ));
    }
    let artifact = &view.artifact;
    let sha256 = artifact.sha256.to_ascii_lowercase();
    let mut metadata = json!({
        "channel": channel,
        "kind": "file_external",
        "step": "begin",
        "filename": artifact.name,
        "size_bytes": artifact.size_bytes,
        "sha256": sha256,
    });
    for (key, value) in [
        ("reply_to", &options.reply_to),
        ("title", &options.title),
        ("initial_comment", &options.initial_comment),
    ] {
        if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            metadata[key] = json!(value);
        }
    }

    let begin = envelope(violet.violet_post(metadata.clone()).await?)?;
    let file_id = field(&begin, "/file_id")?;
    let upload_url = field(&begin, "/upload_url")?;

    let finish = async {
        if !safe_url(&upload_url) {
            return Err(PostError::new(
                "violet_invalid_receipt",
                "Violet returned an upload location that is not https",
            ));
        }
        tokio::task::spawn_blocking(move || send_bytes(&upload_url, &bytes))
            .await
            .map_err(|_| PostError::new("violet_upload_failed", "the upload task stopped"))??;

        metadata["step"] = json!("complete");
        metadata["file_id"] = json!(file_id);
        let complete = envelope(violet.violet_post(metadata).await?)?;
        receipt(
            &complete,
            artifact.id.clone(),
            &file_id,
            artifact.size_bytes,
            &sha256,
        )
    }
    .await;
    finish.map_err(|mut error| {
        error.file_id = Some(file_id.clone());
        error
    })
}

/// Slack's external upload URL takes the raw bytes as a POST, which is what
/// the hub itself sends for inline files.
fn send_bytes(upload_url: &str, bytes: &[u8]) -> Result<(), PostError> {
    match byte_agent(0)
        .post(upload_url)
        .set("Content-Type", "application/octet-stream")
        .send_bytes(bytes)
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, _)) => Err(PostError::new(
            "violet_upload_failed",
            format!("the Slack upload URL answered {status}; nothing was shared"),
        )),
        Err(ureq::Error::Transport(_)) => Err(PostError::new(
            "violet_upload_failed",
            "network error sending bytes to the Slack upload URL; nothing was shared",
        )),
    }
}

fn receipt(
    complete: &Value,
    artifact_id: String,
    file_id: &str,
    size_bytes: u64,
    sha256: &str,
) -> Result<PostReceipt, PostError> {
    let file = complete.get("file").ok_or_else(|| missing("/file"))?;
    if file.get("file_id").and_then(Value::as_str) != Some(file_id) {
        return Err(PostError::new(
            "violet_invalid_receipt",
            "Violet's receipt names a different file than the one uploaded",
        ));
    }
    let reported_size = file.get("size_bytes").and_then(Value::as_u64);
    let reported_sha = file.get("sha256").and_then(Value::as_str);
    if reported_size.is_some_and(|size| size != size_bytes)
        || reported_sha.is_some_and(|digest| !digest.eq_ignore_ascii_case(sha256))
    {
        return Err(PostError::new(
            "slack_integrity_mismatch",
            "Violet reports a Slack copy whose size or SHA-256 differs from the artifact",
        ));
    }
    let message_permalink = complete
        .pointer("/message/permalink")
        .and_then(Value::as_str)
        .filter(|p| !p.trim().is_empty());
    let permalink = message_permalink
        .or_else(|| file.get("permalink").and_then(Value::as_str))
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| missing("/message/permalink"))?;
    Ok(PostReceipt {
        artifact_id,
        channel_id: complete
            .pointer("/channel/id")
            .and_then(Value::as_str)
            .map(str::to_string),
        message_id: complete
            .pointer("/message/message_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        file_id: file_id.to_string(),
        permalink: permalink.to_string(),
        size_bytes,
        sha256: sha256.to_string(),
        sha256_verified: file.get("sha256_verified").and_then(Value::as_bool) == Some(true),
    })
}

/// Decode a Violet envelope from either the bare JSON or the MCP tool result
/// that wraps it (`structuredContent`, or JSON text in `content`).
pub fn envelope(result: Value) -> Result<Value, PostError> {
    let value = if result.get("ok").is_some() {
        result
    } else if let Some(structured) = result
        .get("structuredContent")
        .filter(|v| v.get("ok").is_some())
    {
        structured.clone()
    } else {
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|content| {
                content
                    .iter()
                    .find_map(|item| item.get("text").and_then(Value::as_str))
            })
            .ok_or_else(|| {
                PostError::new("violet_invalid_receipt", "Violet returned no receipt")
            })?;
        serde_json::from_str(text).map_err(|_| {
            PostError::new(
                "violet_invalid_receipt",
                format!("Violet returned a non-JSON receipt: {}", excerpt(text)),
            )
        })?
    };
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(value);
    }
    let code = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .filter(|code| !code.trim().is_empty())
        .unwrap_or("violet_post_failed");
    let message = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .map(excerpt)
        .unwrap_or_else(|| "Violet refused the post".to_string());
    Err(PostError::new(code, message))
}

fn field(value: &Value, path: &str) -> Result<String, PostError> {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| missing(path))
}

fn missing(path: &str) -> PostError {
    PostError::new(
        "violet_invalid_receipt",
        format!("Violet's receipt lacks {path}"),
    )
}

fn excerpt(text: &str) -> String {
    let text = text.trim().replace('\n', " ");
    if text.chars().count() <= 300 {
        return text;
    }
    format!("{}…", text.chars().take(300).collect::<String>())
}

#[cfg(test)]
#[path = "slack_tests.rs"]
mod tests;
