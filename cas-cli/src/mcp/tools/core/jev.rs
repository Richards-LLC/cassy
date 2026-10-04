//! All harnesses share this MCP handler and the CLI/library decision client.
use crate::jev::{FilesOptions, JevClient, JevError};
use crate::mcp::tools::core::imports::*;
use cas_mcp::JevRequest;

impl CasCore {
    pub async fn jev_evaluate(&self, req: JevRequest) -> Result<CallToolResult, McpError> {
        // Validate before scheduling blocking I/O; all other MCP tools remain
        // responsive during the bounded HTTP retries and log writes.
        match req.action.as_str() {
            "ask"
                if req.state.is_some()
                    && req.records.is_none()
                    && req.paths.is_none()
                    && req.globs.is_none() => {}
            "batch"
                if req.state.is_none()
                    && req.paths.is_none()
                    && req.globs.is_none()
                    && req
                        .records
                        .as_ref()
                        .is_some_and(|r| (1..=50).contains(&r.len())) => {}
            "files"
                if req.state.is_none()
                    && req.records.is_none()
                    && (req.paths.as_ref().is_some_and(|p| !p.is_empty())
                        || req.globs.as_ref().is_some_and(|p| !p.is_empty())) => {}
            _ => {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "Jev ask requires state; batch requires 1–50 records; files requires paths/globs; do not mix inputs",
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
                "files" => serde_json::to_value(
                    client.files(
                        root.parent()
                            .ok_or_else(|| JevError::InvalidInput("Missing project root".into()))?,
                        &FilesOptions {
                            paths: req.paths.unwrap_or_default(),
                            globs: req.globs.unwrap_or_default(),
                            recursive: req.recursive,
                            max_files: req.max_files.unwrap_or(50),
                            max_bytes: req.max_bytes.unwrap_or(crate::jev::DEFAULT_FILE_BYTES),
                            offset: req.offset.unwrap_or(0),
                            rev: req.rev,
                        },
                        &req.questions,
                        "mcp:jev.files",
                        req.advisory,
                    )?,
                ),
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
            paths: None,
            globs: None,
            recursive: false,
            max_files: None,
            max_bytes: None,
            offset: None,
            rev: None,
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
            json!(["ask", "batch", "files"])
        );
    }
    #[tokio::test]
    async fn jev_mcp_files_mock_http_and_input_parity() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.remove("TYPESAFE_API_KEY");
        let server = MockServer::start().await;
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(dir.path().join("source.rs"), "mcp-private-content").unwrap();
        std::fs::write(dir.path().join(".env.local"), "mcp-secret").unwrap();
        crate::cloud::CloudConfig {
            endpoint: server.uri(),
            token: Some("mock-token".into()),
            ..Default::default()
        }
        .save_to_cas_dir(&root)
        .unwrap();
        Mock::given(method("POST")).and(path("/api/jev"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"model":"jev-1.13.0", "answers":{"urgent":{"type":"noul","noul":0.8}}, "usage":{"input_tokens":10,"output_tokens":1}})))
            .expect(1).mount(&server).await;
        let core = CasCore::with_daemon(root.clone(), None, None);
        #[cfg(feature = "mcp-proxy")]
        let service = crate::mcp::tools::CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = crate::mcp::tools::CasService::new(core);
        let req: JevRequest = serde_json::from_value(json!({"action":"files", "paths":["source.rs", ".env.local"], "questions":{"urgent":{"type":"noul","instructions":"Urgent?"}}, "max_files":2})).unwrap();
        let result = service.jev(Parameters(req.clone())).await.unwrap();
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(encoded.contains("available") && encoded.contains("secret path"));
        assert!(!encoded.contains("mcp-private-content") && !encoded.contains("mcp-secret"));
        let log = std::fs::read_to_string(root.join("jev-decisions.jsonl")).unwrap();
        assert_eq!(log.lines().count(), 1);
        assert!(log.contains("mcp:jev.files"));
        assert!(!log.contains("mcp-private-content"));
        let mut invalid = req.clone();
        invalid.state = Some(json!("mixed"));
        assert!(service.jev(Parameters(invalid)).await.is_err());
        let mut invalid = req.clone();
        invalid.max_files = Some(51);
        assert!(service.jev(Parameters(invalid)).await.is_err());
        let mut invalid = req;
        invalid.paths = None;
        assert!(service.jev(Parameters(invalid)).await.is_err());
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            body["state"],
            json!({"path":"source.rs","content":"mcp-private-content"})
        );
    }
}
