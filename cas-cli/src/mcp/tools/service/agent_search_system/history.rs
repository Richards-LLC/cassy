//! `mcp__cas__search action=history` — the code-history query surface
//! (EPIC cas-6212 / cas-7f40, spec §6.1).
//!
//! This handler is deliberately thin. It maps request fields onto
//! [`crate::history::search::HistorySearchRequest`], calls the one production
//! entry point, and serializes the result. Every filter decision, every
//! honesty declaration and the whole `index_status` block live in
//! `history::search`, shared byte-for-byte with `cas history search`.
//!
//! Keeping it thin is the point of §6.3: if this handler built its own ranker,
//! an integration test could exercise the MCP surface and still miss the CLI's
//! wiring (or vice versa), which is exactly how the knowledge channel shipped
//! inert.

use crate::mcp::tools::service::imports::*;

impl CasService {
    pub(in crate::mcp::tools::service) async fn history_search_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        let request = crate::history::search::HistorySearchRequest {
            query: req.query.filter(|q| !q.trim().is_empty()),
            path: req.path,
            symbol: req.symbol,
            since: req.since,
            until: req.until,
            kind: req.kind,
            task_id: req.task_id,
            session_id: req.session_id,
            limit: req.limit.unwrap_or(10),
            include_provenance: req.include_provenance.unwrap_or(false),
            include_merges: req.include_merges.unwrap_or(false),
        };

        let cas_root = self.inner.cas_root.clone();
        let response = crate::history::search::run(&cas_root, &request).map_err(|e| {
            // A history query that cannot run must say so, not return an empty
            // result set that reads as "this never happened".
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("history search failed: {e}"),
            )
        })?;

        let payload = serde_json::to_string_pretty(&response).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("serializing history response: {e}"),
            )
        })?;
        Ok(Self::success(payload))
    }
}
