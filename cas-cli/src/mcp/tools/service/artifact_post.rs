//! `artifact action=post` (cassy#1148): post a committed artifact to Slack
//! through Violet by its record id.
//!
//! It lives on [`CasService`] rather than `CasCore` because it needs the MCP
//! proxy and the session's registered [`cmcp_core::ProxyCaller`]: the Violet
//! calls go through the same policy as a direct `violet_post`.

use std::sync::Arc;

use rmcp::model::{CallToolResult, ErrorCode};
use serde_json::{Value, json};

use super::{ArtifactRequest, CasService, McpError};
use crate::artifacts::slack::{PostError, PostOptions, PostReceipt, VioletPoster};
use cas_store::SqliteArtifactStore;
use cmcp_core::config::VIOLET_SERVER;

/// `violet.violet_post` through the proxy, as the registered caller.
struct ProxyViolet {
    proxy: Arc<cmcp_core::ProxyEngine>,
    caller: cmcp_core::ProxyCaller,
}

impl VioletPoster for ProxyViolet {
    fn violet_post(
        &self,
        args: Value,
    ) -> impl std::future::Future<Output = Result<Value, PostError>> + Send {
        let proxy = Arc::clone(&self.proxy);
        let caller = self.caller.clone();
        async move {
            proxy
                .call_tool(
                    &caller,
                    VIOLET_SERVER,
                    "violet_post",
                    args.as_object().cloned(),
                )
                .await
                .map_err(|error| {
                    PostError::new(
                        "violet_unavailable",
                        format!(
                            "the Violet call failed: {}",
                            cmcp_core::describe_upstream_call_error(&error)
                        ),
                    )
                })
        }
    }
}

impl CasService {
    pub(super) async fn artifact_post(
        &self,
        req: ArtifactRequest,
    ) -> Result<CallToolResult, McpError> {
        let options = PostOptions {
            channel: req.channel.clone().unwrap_or_default(),
            reply_to: req.reply_to.clone(),
            title: req.title.clone(),
            initial_comment: req.initial_comment.clone(),
        };
        if options.channel.trim().is_empty() {
            return Err(post_error(PostError::new(
                "artifact_channel_missing",
                "artifact post requires 'channel', a Slack channel name or id",
            )));
        }
        let proxy = self.proxy.clone().ok_or_else(|| {
            post_error(PostError::new(
                "violet_unavailable",
                "no MCP proxy is configured, so Violet cannot be reached; run `cas integrate violet`",
            ))
        })?;
        let caller = self.proxy_caller()?;

        let cas_root = self.inner.cas_root.clone();
        let id = req.id.clone().unwrap_or_default();
        let (view, bytes) = tokio::task::spawn_blocking({
            let cas_root = cas_root.clone();
            move || {
                let store = SqliteArtifactStore::open(&cas_root).map_err(|error| {
                    PostError::new(
                        "artifact_store_failed",
                        format!("could not open the artifact store: {error}"),
                    )
                })?;
                let cloud = crate::artifacts::cloud_client(&cas_root);
                crate::artifacts::slack::fetch_verified(&store, cloud.as_ref(), &id)
            }
        })
        .await
        .map_err(|_| {
            post_error(PostError::new(
                "artifact_download_failed",
                "the download task stopped",
            ))
        })?
        .map_err(post_error)?;

        let violet = ProxyViolet { proxy, caller };
        let receipt = crate::artifacts::slack::post_verified(&violet, &view, bytes, &options)
            .await
            .map_err(post_error)?;

        // The permalink is what a release report cites; keep it on the record.
        let recorded = SqliteArtifactStore::open(&cas_root)
            .and_then(|store| store.set_slack_permalink(&receipt.artifact_id, &receipt.permalink))
            .is_ok();
        Ok(Self::success(render_post(&receipt, recorded)))
    }
}

fn render_post(receipt: &PostReceipt, recorded: bool) -> String {
    let mut out = format!(
        "Posted {} to Slack\n\nartifact_id: {}\nchannel_id: {}\nmessage_id: {}\nfile_id: {}\npermalink: {}\nsize_bytes: {}\nsha256: {}\nsha256_verified: {}\n",
        receipt.artifact_id,
        receipt.artifact_id,
        receipt.channel_id.as_deref().unwrap_or("unknown"),
        receipt.message_id.as_deref().unwrap_or("unresolved"),
        receipt.file_id,
        receipt.permalink,
        receipt.size_bytes,
        receipt.sha256,
        receipt.sha256_verified,
    );
    if !receipt.sha256_verified {
        let why = if receipt.size_bytes > crate::artifacts::slack::HUB_HASH_LIMIT {
            "the file is over Violet's 16 MiB hashing limit"
        } else {
            "Violet did not report hashing it"
        };
        out.push_str(&format!(
            "\nThe source bytes matched the published SHA-256, but the Slack copy is unverified: {why}. \
             Check it with `violet_read file_id={}` before citing it as verified.\n",
            receipt.file_id
        ));
    }
    if !recorded {
        out.push_str("\nThe permalink could not be saved on the artifact record; cite it from this receipt.\n");
    }
    out
}

/// A named failure as an MCP error: the code leads the message and is also in
/// `data`, with any allocated Slack upload.
fn post_error(error: PostError) -> McpError {
    let code = match error.code.as_str() {
        "artifact_id_missing" | "artifact_channel_missing" | "artifact_not_found" => {
            ErrorCode::INVALID_PARAMS
        }
        _ => ErrorCode::INVALID_REQUEST,
    };
    McpError {
        code,
        message: std::borrow::Cow::Owned(error.to_string()),
        data: Some(json!({ "error_code": error.code, "file_id": error.file_id })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(verified: bool, size_bytes: u64) -> PostReceipt {
        PostReceipt {
            artifact_id: "art-1148".to_string(),
            channel_id: Some("C123".to_string()),
            message_id: Some("1710000000.000003".to_string()),
            file_id: "F1148".to_string(),
            permalink: "https://slack.test/p/3".to_string(),
            size_bytes,
            sha256: "b".repeat(64),
            sha256_verified: verified,
        }
    }

    #[test]
    fn the_receipt_leads_with_the_ids_a_release_report_records() {
        let text = render_post(&receipt(true, 11), true);
        assert!(text.starts_with("Posted art-1148 to Slack"), "{text}");
        for line in [
            "message_id: 1710000000.000003",
            "file_id: F1148",
            "permalink: https://slack.test/p/3",
            "sha256_verified: true",
        ] {
            assert!(text.contains(line), "{line} missing: {text}");
        }
        assert!(!text.contains("unverified"), "{text}");
    }

    #[test]
    fn an_unverified_slack_copy_says_why_and_how_to_check() {
        let small = render_post(&receipt(false, 11), true);
        assert!(small.contains("did not report hashing"), "{small}");
        assert!(small.contains("violet_read file_id=F1148"), "{small}");
        let large = render_post(&receipt(false, 20 * 1024 * 1024), false);
        assert!(large.contains("16 MiB"), "{large}");
        assert!(large.contains("could not be saved"), "{large}");
    }

    #[test]
    fn a_named_failure_keeps_its_code_and_upload() {
        let mut error = PostError::new("file_integrity_mismatch", "sha256 differs");
        error.file_id = Some("F1".to_string());
        let mcp = post_error(error);
        assert_eq!(mcp.code, ErrorCode::INVALID_REQUEST);
        assert!(
            mcp.message.starts_with("file_integrity_mismatch:"),
            "{}",
            mcp.message
        );
        let data = mcp.data.unwrap();
        assert_eq!(data["error_code"], "file_integrity_mismatch");
        assert_eq!(data["file_id"], "F1");
        assert_eq!(
            post_error(PostError::new("artifact_not_found", "x")).code,
            ErrorCode::INVALID_PARAMS
        );
    }
}
