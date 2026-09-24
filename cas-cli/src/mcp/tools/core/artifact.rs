//! MCP handlers for published artifacts (cassy#910).
//!
//! The harness-agnostic half of the feature: a worker on Claude, Codex or Grok
//! calls `artifact` with `action=publish` and a local path, and the runtime
//! does the rest. Nothing here ever hands a harness a digest to compute, a
//! size to check, or an upload URL to hold — those are exactly the steps an
//! agent would get subtly wrong, so they live in [`crate::artifacts`].
//!
//! These handlers are deliberately thin: the CLI at `cas artifact …` and this
//! tool call the same `publish`/store functions and render through the same
//! `render_artifact_line`, so the two surfaces cannot drift.

use crate::mcp::tools::core::imports::*;

use cas_mcp::ArtifactRequest;
use cas_store::{PublishedArtifact, SqliteArtifactStore};
use std::path::PathBuf;

use crate::artifacts::{
    PublishContext, PublishDisposition, PublishOutcome, cloud::ArtifactUploadClient,
    paths::PublishRoots, publish,
};
use crate::cli::render_artifact_line;

impl CasCore {
    fn open_artifact_store(&self) -> Result<SqliteArtifactStore, McpError> {
        SqliteArtifactStore::open(&self.cas_root).map_err(|error| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open artifact store: {error}")),
            data: None,
        })
    }

    /// Publish a local file. The response names the record id first, because
    /// that is the only part a caller carries forward.
    pub async fn artifact_publish(
        &self,
        Parameters(req): Parameters<ArtifactRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_id = required(&req.task_id, "artifact publish requires 'task_id'")?;
        let path = required(&req.path, "artifact publish requires 'path'")?;

        let store = self.open_artifact_store()?;
        let config = crate::config::Config::load(&self.cas_root).unwrap_or_default();
        let artifacts_root = crate::config::resolved_factory_artifacts_root(
            config.factory().artifacts_root.as_deref(),
        );
        let context = PublishContext {
            store: &store,
            roots: PublishRoots::new(&self.cas_root, &artifacts_root, &task_id),
            task_id: task_id.clone(),
            cloud: upload_client(&self.cas_root),
        };

        let outcome = publish(&context, &PathBuf::from(&path)).map_err(|error| {
            // A refused path or an oversized file is the caller's mistake to
            // fix, not an internal fault; the message already says what to do.
            Self::error(ErrorCode::INVALID_PARAMS, error.to_string())
        })?;

        Ok(Self::success(render_publish(&outcome)))
    }

    pub async fn artifact_show(
        &self,
        Parameters(req): Parameters<ArtifactRequest>,
    ) -> Result<CallToolResult, McpError> {
        let id = required(&req.id, "artifact show requires 'id'")?;
        let store = self.open_artifact_store()?;
        let Some(artifact) = store.get(&id).map_err(internal)? else {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("No artifact {id}"),
            ));
        };
        Ok(Self::success(render_detail(&artifact)))
    }

    pub async fn artifact_list(
        &self,
        Parameters(req): Parameters<ArtifactRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_id = required(&req.task_id, "artifact list requires 'task_id'")?;
        let store = self.open_artifact_store()?;
        let artifacts = store.list_for_task(&task_id).map_err(internal)?;

        if artifacts.is_empty() {
            return Ok(Self::success(format!(
                "No artifacts published for {task_id}"
            )));
        }
        let mut out = format!("Artifacts for {task_id} ({})\n\n", artifacts.len());
        for artifact in &artifacts {
            out.push_str(&format!("- {}\n", render_artifact_line(artifact)));
        }
        Ok(Self::success(out))
    }
}

fn required(value: &Option<String>, message: &str) -> Result<String, McpError> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CasCore::error(ErrorCode::INVALID_PARAMS, message.to_string()))
}

fn internal(error: impl std::fmt::Display) -> McpError {
    CasCore::error(ErrorCode::INTERNAL_ERROR, error.to_string())
}

/// Build the upload client, or `None` when this installation has no cloud
/// credentials — a supported state, reported in the response rather than
/// failing the call.
fn upload_client(cas_root: &std::path::Path) -> Option<ArtifactUploadClient> {
    crate::artifacts::cloud_client(cas_root)
}

fn render_publish(outcome: &PublishOutcome) -> String {
    let artifact = &outcome.artifact;
    let mut out = format!(
        "Published {}\n\nartifact_id: {}\nname: {}\nmime: {}\nsize_bytes: {}\nsha256: {}\nstatus: {}\nstorage: {}\n",
        artifact.id,
        artifact.id,
        artifact.name,
        artifact.mime,
        artifact.size_bytes,
        artifact.sha256,
        artifact.status,
        outcome.disposition.summary(),
    );
    if let Some(url) = &artifact.cloud_url {
        out.push_str(&format!("cloud_url: {url}\n"));
    }
    if let PublishDisposition::UploadFailed { reason } = &outcome.disposition {
        out.push_str(&format!(
            "\nThe record is usable; the upload is not.\n{reason}\n"
        ));
    }
    out
}

fn render_detail(artifact: &PublishedArtifact) -> String {
    let mut out = format!(
        "{}\n\ntask_id: {}\nname: {}\nmime: {}\nsize_bytes: {}\nsha256: {}\nstatus: {}\ncreated_at: {}\n",
        artifact.id,
        artifact.task_id,
        artifact.name,
        artifact.mime,
        artifact.size_bytes,
        artifact.sha256,
        artifact.status,
        artifact.created_at,
    );
    if let Some(cloud_id) = &artifact.cloud_artifact_id {
        out.push_str(&format!("cloud_artifact_id: {cloud_id}\n"));
    }
    if let Some(url) = &artifact.cloud_url {
        out.push_str(&format!("cloud_url: {url}\n"));
    }
    if let Some(permalink) = &artifact.slack_permalink {
        out.push_str(&format!("slack_permalink: {permalink}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::PublishDisposition;

    fn artifact() -> PublishedArtifact {
        PublishedArtifact {
            id: "art-7f3a9c21".to_string(),
            task_id: "cas-b72a".to_string(),
            name: "brief.pdf".to_string(),
            mime: "application/pdf".to_string(),
            size_bytes: 812_345,
            sha256: "b".repeat(64),
            cloud_artifact_id: None,
            status: "local".to_string(),
            cloud_url: None,
            slack_permalink: None,
            created_at: "2026-09-18T20:00:00Z".to_string(),
        }
    }

    #[test]
    fn the_publish_response_leads_with_the_id_a_caller_carries_forward() {
        let outcome = PublishOutcome {
            artifact: artifact(),
            disposition: PublishDisposition::NotLoggedIn,
            source: PathBuf::from("/tmp/brief.pdf"),
        };
        let rendered = render_publish(&outcome);
        assert!(rendered.starts_with("Published art-7f3a9c21"), "{rendered}");
        assert!(rendered.contains("artifact_id: art-7f3a9c21"), "{rendered}");
        assert!(rendered.contains("sha256: bbbb"), "{rendered}");
        assert!(
            rendered.contains("not logged in"),
            "the storage outcome must be stated: {rendered}"
        );
    }

    #[test]
    fn an_upload_failure_is_reported_without_losing_the_record() {
        let outcome = PublishOutcome {
            artifact: artifact(),
            disposition: PublishDisposition::UploadFailed {
                reason: "artifact upload failed: the object store answered 403".to_string(),
            },
            source: PathBuf::from("/tmp/brief.pdf"),
        };
        let rendered = render_publish(&outcome);
        assert!(rendered.contains("art-7f3a9c21"), "{rendered}");
        assert!(
            rendered.contains("The record is usable; the upload is not."),
            "{rendered}"
        );
        assert!(rendered.contains("403"), "{rendered}");
    }

    #[test]
    fn a_committed_artifact_reports_its_durable_location() {
        let mut committed = artifact();
        committed.status = "committed".to_string();
        committed.cloud_artifact_id = Some("cloud-42".to_string());
        committed.cloud_url = Some("https://cloud.example/a/cloud-42".to_string());
        let detail = render_detail(&committed);
        assert!(detail.contains("cloud_artifact_id: cloud-42"), "{detail}");
        assert!(
            detail.contains("cloud_url: https://cloud.example/a/cloud-42"),
            "{detail}"
        );
        assert!(detail.contains("status: committed"), "{detail}");
    }

    #[test]
    fn a_missing_required_field_names_the_field() {
        let error = required(&None, "artifact publish requires 'task_id'").unwrap_err();
        assert!(error.message.contains("task_id"), "{}", error.message);
        assert!(
            required(&Some("  ".to_string()), "m").is_err(),
            "blank is missing"
        );
        assert_eq!(required(&Some(" x ".to_string()), "m").unwrap(), "x");
    }
}
