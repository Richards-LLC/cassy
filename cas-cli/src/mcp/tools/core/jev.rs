//! All harnesses share this MCP handler and the CLI/library decision client.
use crate::jev::{JevClient, JevError};
use crate::mcp::tools::core::imports::*;
use cas_mcp::JevRequest;

impl CasCore {
    pub async fn jev_evaluate(&self, req: JevRequest) -> Result<CallToolResult, McpError> {
        // Validate before scheduling blocking I/O; all other MCP tools remain
        // responsive during the bounded HTTP retries and log writes.
        match req.action.as_str() {
            "ask" if req.state.is_some() && req.records.is_none() => {}
            "batch"
                if req.state.is_none()
                    && req
                        .records
                        .as_ref()
                        .is_some_and(|r| (1..=50).contains(&r.len())) => {}
            _ => {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "Jev ask requires state; batch requires 1–50 records; use exactly one input",
                ));
            }
        }
        let root = self.cas_root.clone();
        let value = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, JevError> {
            let client = JevClient::from_project(&root)?;
            let value = match req.action.as_str() {
                "ask" => serde_json::to_value(client.ask(
                    req.state.as_ref().expect("validated state"),
                    &req.questions,
                    "mcp:jev.ask",
                    req.advisory,
                )?),
                "batch" => serde_json::to_value(client.batch(
                    req.records.as_ref().expect("validated records"),
                    &req.questions,
                    "mcp:jev.batch",
                    req.advisory,
                )?),
                _ => unreachable!("validated action"),
            };
            value.map_err(|_| JevError::Unavailable("Could not encode Jev result".into()))
        })
        .await
        .map_err(|_| Self::error(ErrorCode::INTERNAL_ERROR, "Jev worker unavailable"))?
        .map_err(|error| {
            Self::error(
                if matches!(error, JevError::InvalidInput(_)) {
                    ErrorCode::INVALID_PARAMS
                } else {
                    ErrorCode::INTERNAL_ERROR
                },
                error.to_string(),
            )
        })?;
        Ok(Self::success(value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn jev_mcp_dispatches_ask_batch_and_unavailable() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[jev]\nenabled = false\n").unwrap();
        let core = CasCore::with_daemon(dir.path().to_path_buf(), None, None);
        #[cfg(feature = "mcp-proxy")]
        let service = crate::mcp::tools::CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = crate::mcp::tools::CasService::new(core);
        let req = JevRequest {
            action: "ask".into(),
            state: Some(json!("secret-state")),
            records: None,
            questions: json!({"urgent":{"type":"noul","instructions":"Urgent?"}}),
            advisory: true,
        };
        let result = service.jev(Parameters(req.clone())).await.unwrap();
        assert_eq!(result.is_error, Some(false));
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(encoded.contains("unavailable"));
        assert!(!encoded.contains("secret-state"));
        let mut batch = req.clone();
        batch.action = "batch".into();
        batch.state = None;
        batch.records = Some(vec![json!("one"), json!("two")]);
        assert!(service.jev(Parameters(batch.clone())).await.is_ok());
        batch.records = Some(vec![json!("x"); 51]);
        assert!(service.jev(Parameters(batch)).await.is_err());
        let mut invalid = req.clone();
        invalid.action = "unknown".into();
        assert!(service.jev(Parameters(invalid)).await.is_err());
        let mut strict = req;
        strict.advisory = false;
        assert!(service.jev(Parameters(strict)).await.is_err());
        let log = std::fs::read_to_string(dir.path().join("jev-decisions.jsonl")).unwrap();
        assert_eq!(log.lines().count(), 4);
        assert!(!log.contains("secret-state"));
        let tool = crate::mcp::tools::CasService::tool_definitions_for_build()
            .into_iter()
            .find(|t| t.name == "jev")
            .unwrap();
        assert_eq!(
            tool.input_schema["properties"]["action"]["enum"],
            json!(["ask", "batch"])
        );
    }
}
