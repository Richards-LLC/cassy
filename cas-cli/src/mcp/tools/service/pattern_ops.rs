use crate::mcp::tools::service::imports::*;

impl CasService {
    pub(super) async fn pattern_create(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let content = req
            .content
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "content required"))?;

        let mut payload = serde_json::Map::new();
        payload.insert("content".to_string(), serde_json::json!(content));

        if let Some(category) = &req.category {
            payload.insert("category".to_string(), serde_json::json!(category));
        }
        if let Some(priority) = req.priority {
            payload.insert("priority".to_string(), serde_json::json!(priority));
        }
        if let Some(propagation) = &req.propagation {
            payload.insert("propagation".to_string(), serde_json::json!(propagation));
        }
        if let Some(tags) = &req.tags {
            let tag_list: Vec<&str> = tags.split(',').map(|t| t.trim()).collect();
            payload.insert("propagation_tags".to_string(), serde_json::json!(tag_list));
        }

        let pattern = self.pattern_api_call(
            "POST",
            "/api/patterns",
            Some(serde_json::Value::Object(payload)),
        )?;

        Ok(Self::success(format!(
            "Created pattern: {}\n\n{}",
            pattern
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
            format_pattern(&pattern),
        )))
    }

    pub(super) async fn pattern_list(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let mut query_params = Vec::new();
        if let Some(category) = &req.category {
            query_params.push(format!("category={category}"));
        }
        if let Some(status) = &req.status {
            query_params.push(format!("status={status}"));
        }

        let path = if query_params.is_empty() {
            "/api/patterns".to_string()
        } else {
            format!("/api/patterns?{}", query_params.join("&"))
        };

        let resp = self.pattern_api_call("GET", &path, None)?;

        let patterns = resp
            .get("patterns")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                Self::error(ErrorCode::INTERNAL_ERROR, "Missing patterns in response")
            })?;

        if patterns.is_empty() {
            return Ok(Self::success("No personal patterns found."));
        }

        let limit = req.limit.unwrap_or(50);
        let mut output = format!("Personal Patterns ({} total):\n\n", patterns.len());

        for pattern in patterns.iter().take(limit) {
            output.push_str(&format!("- {}\n", format_pattern_summary(pattern)));
        }

        if patterns.len() > limit {
            output.push_str(&format!("\n... and {} more", patterns.len() - limit));
        }

        Ok(Self::success(output))
    }

    pub(super) async fn pattern_show(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        let resp = self.pattern_api_call("GET", &format!("/api/patterns/{id}"), None)?;

        let pattern = pattern_response_row(&resp)?;

        Ok(Self::success(format_pattern(pattern)))
    }

    pub(super) async fn pattern_update(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        let mut payload = serde_json::Map::new();

        if let Some(content) = &req.content {
            payload.insert("content".to_string(), serde_json::json!(content));
        }
        if let Some(category) = &req.category {
            payload.insert("category".to_string(), serde_json::json!(category));
        }
        if let Some(priority) = req.priority {
            payload.insert("priority".to_string(), serde_json::json!(priority));
        }
        if let Some(propagation) = &req.propagation {
            payload.insert("propagation".to_string(), serde_json::json!(propagation));
        }
        if let Some(tags) = &req.tags {
            let tag_list: Vec<&str> = tags.split(',').map(|t| t.trim()).collect();
            payload.insert("propagation_tags".to_string(), serde_json::json!(tag_list));
        }

        if payload.is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "At least one field to update is required",
            ));
        }

        let resp = self.pattern_api_call(
            "PATCH",
            &format!("/api/patterns/{id}"),
            Some(serde_json::Value::Object(payload)),
        )?;

        let pattern = pattern_response_row(&resp)?;

        Ok(Self::success(format!(
            "Updated pattern: {}\n\n{}",
            id,
            format_pattern(pattern),
        )))
    }

    pub(super) async fn pattern_archive(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        let resp = self.pattern_api_call("DELETE", &format!("/api/patterns/{id}"), None)?;

        let pattern = pattern_response_row(&resp)?;
        let status = pattern
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("archived");

        Ok(Self::success(format!(
            "Pattern {id} archived (status: {status})"
        )))
    }

    pub(super) async fn pattern_adopt(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let rule_id = req
            .rule_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "rule_id required"))?;

        let mut payload = serde_json::Map::new();
        payload.insert("rule_id".to_string(), serde_json::json!(rule_id));

        if let Some(propagation) = &req.propagation {
            payload.insert("propagation".to_string(), serde_json::json!(propagation));
        }
        if let Some(tags) = &req.tags {
            let tag_list: Vec<&str> = tags.split(',').map(|t| t.trim()).collect();
            payload.insert("tags".to_string(), serde_json::json!(tag_list));
        }
        if let Some(priority) = req.priority {
            payload.insert("priority".to_string(), serde_json::json!(priority));
        }

        let resp = self.pattern_api_call(
            "POST",
            "/api/patterns/adopt-rule",
            Some(serde_json::Value::Object(payload)),
        )?;

        let pattern = pattern_response_row(&resp)?;

        Ok(Self::success(format!(
            "Adopted rule {} as personal pattern: {}\n\n{}",
            rule_id,
            pattern
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
            format_pattern(pattern),
        )))
    }

    pub(super) async fn pattern_helpful(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        // Update helpful_count by fetching current and incrementing
        let show_resp = self.pattern_api_call("GET", &format!("/api/patterns/{id}"), None)?;

        let pattern = pattern_response_row(&show_resp)?;

        let current_count = pattern
            .get("helpful_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let mut payload = serde_json::Map::new();
        payload.insert(
            "helpful_count".to_string(),
            serde_json::json!(current_count + 1),
        );

        let resp = self.pattern_api_call(
            "PATCH",
            &format!("/api/patterns/{id}"),
            Some(serde_json::Value::Object(payload)),
        )?;

        let updated = pattern_response_row(&resp)?;
        let new_count = updated
            .get("helpful_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(current_count + 1);

        Ok(Self::success(format!(
            "Pattern {id} marked as helpful (count: {new_count})"
        )))
    }

    pub(super) async fn pattern_harmful(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required"))?;

        let show_resp = self.pattern_api_call("GET", &format!("/api/patterns/{id}"), None)?;

        let pattern = pattern_response_row(&show_resp)?;

        let current_count = pattern
            .get("harmful_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let mut payload = serde_json::Map::new();
        payload.insert(
            "harmful_count".to_string(),
            serde_json::json!(current_count + 1),
        );

        let resp = self.pattern_api_call(
            "PATCH",
            &format!("/api/patterns/{id}"),
            Some(serde_json::Value::Object(payload)),
        )?;

        let updated = pattern_response_row(&resp)?;
        let new_count = updated
            .get("harmful_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(current_count + 1);

        Ok(Self::success(format!(
            "Pattern {id} marked as harmful (count: {new_count})"
        )))
    }

    // ========================================================================
    // Team pattern suggestion operations
    // ========================================================================

    pub(super) async fn team_suggestions(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let team_id = req
            .team_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;

        let mut query_params = Vec::new();
        if let Some(category) = &req.category {
            query_params.push(format!("category={category}"));
        }
        if req.include_dismissed.unwrap_or(false) {
            query_params.push("include_dismissed=true".to_string());
        }

        let path = if query_params.is_empty() {
            format!("/api/teams/{team_id}/suggestions")
        } else {
            format!(
                "/api/teams/{}/suggestions?{}",
                team_id,
                query_params.join("&")
            )
        };

        let resp = self.pattern_api_call("GET", &path, None)?;

        let suggestions = resp
            .get("suggestions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                Self::error(ErrorCode::INTERNAL_ERROR, "Missing suggestions in response")
            })?;

        if suggestions.is_empty() {
            return Ok(Self::success("No team suggestions found."));
        }

        let limit = req.limit.unwrap_or(50);
        let mut output = format!("Team Suggestions ({} total):\n\n", suggestions.len());

        for s in suggestions.iter().take(limit) {
            output.push_str(&format!("- {}\n", format_suggestion_summary(s)));
        }

        if suggestions.len() > limit {
            output.push_str(&format!("\n... and {} more", suggestions.len() - limit));
        }

        Ok(Self::success(output))
    }

    pub(super) async fn team_new_suggestions(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let team_id = req
            .team_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;

        let resp = self.pattern_api_call(
            "GET",
            &format!("/api/teams/{team_id}/suggestions/new"),
            None,
        )?;

        let suggestions = resp
            .get("suggestions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                Self::error(ErrorCode::INTERNAL_ERROR, "Missing suggestions in response")
            })?;

        if suggestions.is_empty() {
            return Ok(Self::success("No new team suggestions."));
        }

        let mut output = format!("New Team Suggestions ({}):\n\n", suggestions.len());

        for s in suggestions {
            output.push_str(&format!("- {}\n", format_suggestion_summary(s)));
        }

        Ok(Self::success(output))
    }

    pub(super) async fn team_create_suggestion(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let team_id = req
            .team_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;
        let content = req
            .content
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "content required"))?;

        let mut payload = serde_json::Map::new();
        let mut suggestion = serde_json::Map::new();
        suggestion.insert("content".to_string(), serde_json::json!(content));

        if let Some(category) = &req.category {
            suggestion.insert("category".to_string(), serde_json::json!(category));
        }
        if let Some(priority) = req.priority {
            suggestion.insert("priority".to_string(), serde_json::json!(priority));
        }

        payload.insert(
            "suggestion".to_string(),
            serde_json::Value::Object(suggestion),
        );

        let resp = self.pattern_api_call(
            "POST",
            &format!("/api/teams/{team_id}/suggestions"),
            Some(serde_json::Value::Object(payload)),
        )?;

        let s = resp.get("suggestion").ok_or_else(|| {
            Self::error(ErrorCode::INTERNAL_ERROR, "Missing suggestion in response")
        })?;

        Ok(Self::success(format!(
            "Created team suggestion: {}\n\n{}",
            s.get("id").and_then(|v| v.as_str()).unwrap_or("unknown"),
            format_suggestion(s),
        )))
    }

    pub(super) async fn team_share(&self, req: PatternRequest) -> Result<CallToolResult, McpError> {
        let team_id = req
            .team_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;
        let pattern_id = req
            .pattern_id
            .or(req.id)
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "pattern_id required"))?;

        let mut payload = serde_json::Map::new();
        payload.insert("pattern_id".to_string(), serde_json::json!(pattern_id));

        let resp = self.pattern_api_call(
            "POST",
            &format!("/api/teams/{team_id}/suggestions/share"),
            Some(serde_json::Value::Object(payload)),
        )?;

        let s = resp.get("suggestion").ok_or_else(|| {
            Self::error(ErrorCode::INTERNAL_ERROR, "Missing suggestion in response")
        })?;

        Ok(Self::success(format!(
            "Shared pattern {} as team suggestion: {}\n\n{}",
            pattern_id,
            s.get("id").and_then(|v| v.as_str()).unwrap_or("unknown"),
            format_suggestion(s),
        )))
    }

    pub(super) async fn team_adopt_suggestion(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let team_id = req
            .team_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;
        let suggestion_id = req
            .suggestion_id
            .or(req.id)
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "suggestion_id required"))?;

        let resp = self.pattern_api_call(
            "POST",
            &format!("/api/teams/{team_id}/suggestions/{suggestion_id}/adopt"),
            Some(serde_json::json!({})),
        )?;

        let pattern = resp.get("personal_pattern").ok_or_else(|| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                "Missing personal_pattern in response",
            )
        })?;

        Ok(Self::success(format!(
            "Adopted suggestion {} as personal pattern: {}\n\n{}",
            suggestion_id,
            pattern
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
            format_pattern(pattern),
        )))
    }

    pub(super) async fn team_dismiss_suggestion(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let team_id = req
            .team_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;
        let suggestion_id = req
            .suggestion_id
            .or(req.id)
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "suggestion_id required"))?;

        self.pattern_api_call(
            "POST",
            &format!("/api/teams/{team_id}/suggestions/{suggestion_id}/dismiss"),
            Some(serde_json::json!({})),
        )?;

        Ok(Self::success(format!(
            "Dismissed suggestion {suggestion_id} (hidden from future context)"
        )))
    }

    pub(super) async fn team_recommend_suggestion(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let team_id = req
            .team_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;
        let suggestion_id = req
            .suggestion_id
            .or(req.id)
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "suggestion_id required"))?;

        let recommended = req.recommended.unwrap_or(true);

        let mut payload = serde_json::Map::new();
        payload.insert("recommended".to_string(), serde_json::json!(recommended));

        let resp = self.pattern_api_call(
            "POST",
            &format!("/api/teams/{team_id}/suggestions/{suggestion_id}/recommend"),
            Some(serde_json::Value::Object(payload)),
        )?;

        let s = resp.get("suggestion").ok_or_else(|| {
            Self::error(ErrorCode::INTERNAL_ERROR, "Missing suggestion in response")
        })?;

        let label = if recommended {
            "recommended"
        } else {
            "unrecommended"
        };

        Ok(Self::success(format!(
            "Suggestion {} marked as {}\n\n{}",
            suggestion_id,
            label,
            format_suggestion(s),
        )))
    }

    pub(super) async fn team_archive_suggestion(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let team_id = req
            .team_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;
        let suggestion_id = req
            .suggestion_id
            .or(req.id)
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "suggestion_id required"))?;

        self.pattern_api_call(
            "DELETE",
            &format!("/api/teams/{team_id}/suggestions/{suggestion_id}"),
            None,
        )?;

        Ok(Self::success(format!(
            "Suggestion {suggestion_id} archived"
        )))
    }

    pub(super) async fn team_suggestion_analytics(
        &self,
        req: PatternRequest,
    ) -> Result<CallToolResult, McpError> {
        let team_id = req
            .team_id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "team_id required"))?;
        let suggestion_id = req
            .suggestion_id
            .or(req.id)
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "suggestion_id required"))?;

        let resp = self.pattern_api_call(
            "GET",
            &format!("/api/teams/{team_id}/suggestions/{suggestion_id}/analytics"),
            None,
        )?;

        let analytics = resp.get("analytics").ok_or_else(|| {
            Self::error(ErrorCode::INTERNAL_ERROR, "Missing analytics in response")
        })?;

        let adopted = analytics
            .get("adopted")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let dismissed = analytics
            .get("dismissed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let pending = analytics
            .get("pending")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let total = analytics.get("total").and_then(|v| v.as_u64()).unwrap_or(0);

        Ok(Self::success(format!(
            "Suggestion {suggestion_id} analytics:\n\
             - Adopted: {adopted}\n\
             - Dismissed: {dismissed}\n\
             - Pending: {pending}\n\
             - Total responses: {total}"
        )))
    }

    // ========================================================================
    // Cloud API helper
    // ========================================================================

    fn pattern_api_call(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, McpError> {
        {
            use crate::cloud::CloudConfig;

            let cloud_config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(
                &self.inner.cas_root,
            )
            .map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to load cloud config: {e}"),
                )
            })?;

            if !cloud_config.is_logged_in() {
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "Not logged in to Cassy Cloud. Use `cas login` to authenticate.",
                ));
            }

            let token = cloud_config
                .token
                .as_ref()
                .ok_or_else(|| Self::error(ErrorCode::INTERNAL_ERROR, "Missing auth token"))?;

            let url = format!("{}{}", cloud_config.endpoint, path);
            let timeout = pattern_api_timeout();
            let pattern_write = method == "POST"
                && matches!(path, "/api/patterns" | "/api/patterns/adopt-rule");
            let mut body_started = false;

            let response = match method {
                "GET" => ureq::get(&url)
                    .timeout(timeout)
                    .set("Authorization", &format!("Bearer {token}"))
                    .call(),
                "POST" => {
                    let req = ureq::post(&url)
                        .timeout(timeout)
                        .set("Authorization", &format!("Bearer {token}"))
                        .set("Content-Type", "application/json");
                    if pattern_write {
                        let payload = serde_json::to_vec(&body.unwrap_or_else(|| serde_json::json!({})))
                            .map_err(|e| Self::error(ErrorCode::INTERNAL_ERROR, format!("Failed to encode pattern request: {e}")))?;
                        let length = payload.len().to_string();
                        let mut reader = std::io::Cursor::new(payload);
                        let result = req.set("Content-Length", &length).send(&mut reader);
                        // ureq reads this body only after connecting and writing headers.
                        // This also distinguishes pre-send TLS/setup I/O from a write attempt.
                        body_started = reader.position() > 0;
                        result
                    } else if let Some(body) = body {
                        req.send_json(body)
                    } else {
                        req.send_json(serde_json::json!({}))
                    }
                }
                "PATCH" => {
                    let req = ureq::patch(&url)
                        .timeout(timeout)
                        .set("Authorization", &format!("Bearer {token}"))
                        .set("Content-Type", "application/json");
                    if let Some(body) = body {
                        req.send_json(body)
                    } else {
                        req.send_json(serde_json::json!({}))
                    }
                }
                "DELETE" => ureq::delete(&url)
                    .timeout(timeout)
                    .set("Authorization", &format!("Bearer {token}"))
                    .call(),
                _ => {
                    return Err(Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Unsupported HTTP method: {method}"),
                    ));
                }
            };

            match response {
                Ok(resp) if method == "POST" && path == "/api/patterns" => {
                    decode_pattern_create_response(resp)
                }
                Ok(resp) => resp.into_json::<serde_json::Value>().map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to parse response: {e}"),
                    )
                }),
                Err(ureq::Error::Status(404, _)) => {
                    Err(Self::error(ErrorCode::INVALID_PARAMS, "Pattern not found"))
                }
                Err(ureq::Error::Status(422, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    Err(Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!("Validation failed: {body}"),
                    ))
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    Err(Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("API error ({code}): {body}"),
                    ))
                }
                Err(ureq::Error::Transport(e)) if pattern_write && body_started => {
                    Err(pattern_write_uncertain(
                        if path == "/api/patterns/adopt-rule" { "adopt" } else { "create" },
                        None,
                        format!("local-{}", uuid::Uuid::new_v4()),
                        &format!("a network error prevented confirmation of the write: {e}"),
                    ))
                }
                Err(ureq::Error::Transport(e)) => Err(Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Network error: {e}"),
                )),
            }
        }
    }
}

/// Both deployed bare rows and legacy wrappers must identify a persisted pattern.
fn pattern_response_row(response: &serde_json::Value) -> Result<&serde_json::Value, McpError> {
    let has_id = |row: &&serde_json::Value| {
        row.is_object()
            && row.get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| !id.trim().is_empty())
    };
    response.get("pattern").filter(has_id)
        .or_else(|| Some(response).filter(has_id))
        .ok_or_else(|| CasService::error(ErrorCode::INTERNAL_ERROR, "Missing pattern in response"))
}

/// A non-idempotent write without a usable result needs reconciliation, not a retry.
fn pattern_write_uncertain(operation: &str, status: Option<u16>, reference: String, reason: &str) -> McpError {
    let status_text = status.map(|status| format!("Cloud returned HTTP {status}, but ")).unwrap_or_default();
    let mut error = CasService::error(
        ErrorCode::INTERNAL_ERROR,
        format!(
            "Pattern {operation} outcome uncertain: {status_text}{reason}. \
             The pattern may already exist. Do not retry {operation}. Reconcile with \
             pattern action=list and match the original request before creating again. \
             Reconciliation reference: {reference}",
        ),
    );
    error.data = Some(serde_json::json!({
        "outcome": "uncertain",
        "http_status": status,
        "reconciliation_id": reference,
        "retry_safe": false,
    }));
    error
}

/// Cloud returns the persisted row directly or in a legacy pattern wrapper.
/// An unreadable result after a successful HTTP write does not mean the
/// mutation failed.
fn decode_pattern_create_response(response: ureq::Response) -> Result<serde_json::Value, McpError> {
    let status = response.status();
    let reference = response
        .header("x-request-id")
        .or_else(|| response.header("x-vercel-id"))
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.chars().take(200).collect::<String>())
        .unwrap_or_else(|| format!("local-{}", uuid::Uuid::new_v4()));
    let uncertain = |reason: &str| pattern_write_uncertain("create", Some(status), reference.clone(), reason);
    let pattern: serde_json::Value = response
        .into_json()
        .map_err(|_| uncertain("the response is not valid JSON"))?;
    pattern_response_row(&pattern).cloned()
        .map_err(|_| uncertain("the response has no pattern ID"))
}

// ============================================================================
// Formatting helpers
// ============================================================================

fn format_pattern(pattern: &serde_json::Value) -> String {
    let id = pattern.get("id").and_then(|v| v.as_str()).unwrap_or("?");
    let content = pattern
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let category = pattern
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let priority = pattern
        .get("priority")
        .and_then(|v| v.as_u64())
        .unwrap_or(2);
    let propagation = pattern
        .get("propagation")
        .and_then(|v| v.as_str())
        .unwrap_or("all_projects");
    let status = pattern
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("active");
    let helpful = pattern
        .get("helpful_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let harmful = pattern
        .get("harmful_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let tags = pattern
        .get("propagation_tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let priority_label = match priority {
        0 => "Critical",
        1 => "High",
        2 => "Medium",
        _ => "Low",
    };

    let mut output = format!(
        "Pattern: {id}\n\
         Status: {status} | Category: {category} | Priority: {priority} ({priority_label})\n\
         Propagation: {propagation}\n"
    );

    if !tags.is_empty() {
        output.push_str(&format!("Tags: {tags}\n"));
    }

    output.push_str(&format!(
        "Feedback: +{helpful} helpful / -{harmful} harmful\n\n\
         Content:\n{content}\n"
    ));

    output
}

fn format_pattern_summary(pattern: &serde_json::Value) -> String {
    let id = pattern.get("id").and_then(|v| v.as_str()).unwrap_or("?");
    let content = pattern
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let category = pattern
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let priority = pattern
        .get("priority")
        .and_then(|v| v.as_u64())
        .unwrap_or(2);

    let truncated = if content.len() > 80 {
        format!("{}...", &content[..77])
    } else {
        content.to_string()
    };

    format!("[P{priority}] [{category}] {id} — {truncated}")
}

fn format_suggestion(s: &serde_json::Value) -> String {
    let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("?");
    let content = s.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let category = s
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let priority = s.get("priority").and_then(|v| v.as_u64()).unwrap_or(2);
    let recommended = s
        .get("recommended")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let status = s.get("status").and_then(|v| v.as_str()).unwrap_or("active");
    let adoption_count = s
        .get("adoption_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let user_response = s
        .get("user_response")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let priority_label = match priority {
        0 => "Critical",
        1 => "High",
        2 => "Medium",
        _ => "Low",
    };

    let rec_label = if recommended { " [RECOMMENDED]" } else { "" };

    format!(
        "Suggestion: {id}{rec_label}\n\
         Status: {status} | Category: {category} | Priority: {priority} ({priority_label})\n\
         Adoptions: {adoption_count} | Your response: {user_response}\n\n\
         Content:\n{content}\n"
    )
}

fn format_suggestion_summary(s: &serde_json::Value) -> String {
    let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("?");
    let content = s.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let category = s
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let priority = s.get("priority").and_then(|v| v.as_u64()).unwrap_or(2);
    let recommended = s
        .get("recommended")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let user_response = s
        .get("user_response")
        .and_then(|v| v.as_str())
        .unwrap_or("pending");

    let truncated = if content.len() > 80 {
        format!("{}...", &content[..77])
    } else {
        content.to_string()
    };

    let rec = if recommended { " *" } else { "" };

    format!("[P{priority}] [{category}] [{user_response}] {id} — {truncated}{rec}")
}

/// Client deadline for every Cloud pattern request.
const PATTERN_API_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[cfg(test)]
thread_local! {
    static PATTERN_API_TIMEOUT_OVERRIDE: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

fn pattern_api_timeout() -> std::time::Duration {
    #[cfg(test)]
    if let Some(timeout) = PATTERN_API_TIMEOUT_OVERRIDE.with(std::cell::Cell::get) {
        return timeout;
    }
    PATTERN_API_TIMEOUT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, std::sync::Arc<tiny_http::Server>, CasService) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let server = std::sync::Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        crate::cloud::CloudConfig {
            endpoint: format!("http://{}", server.server_addr()),
            token: Some("pattern-fixture-token".into()),
            ..Default::default()
        }
        .save_to_cas_dir(&root)
        .unwrap();
        let core = CasCore::with_daemon(root, None, None);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        (dir, server, service)
    }

    #[tokio::test]
    async fn pattern_create_reports_cloud_write_outcome_cas_d16e() {
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        // Production deployment 7133052455, Cloud 9ed61999: POST /api/patterns
        // returns the inserted row directly with HTTP 201 (route.ts:90).
        for (response, created, request_id) in [
            (
                r#"{"id":"pat-fixture-1","content":"Fixture guidance","category":"general","priority":1,"propagation":"all_projects"}"#,
                true,
                Some("create-fixture-request"),
            ),
            (r#"{"accepted":true}"#, false, Some("create-fixture-request")),
            ("not-json", false, Some("create-fixture-request")),
            ("not-json", false, None),
            (
                r#"{"pattern":{"id":"pat-fixture-1","content":"Fixture guidance","category":"general","priority":1,"propagation":"all_projects"}}"#,
                true,
                Some("create-fixture-request"),
            ),
        ] {
            let (_dir, server, service) = fixture();
            let http = server.clone();
            let reply = std::thread::spawn(move || {
                let mut request = http
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap()
                    .expect("one create request");
                assert_eq!(request.method().as_str(), "POST");
                assert_eq!(request.url(), "/api/patterns");
                let payload: serde_json::Value =
                    serde_json::from_reader(request.as_reader()).unwrap();
                assert_eq!(payload["content"], "Fixture guidance");
                assert_eq!(
                    payload["propagation_tags"],
                    serde_json::json!(["cas-d16e-fixture"])
                );
                let mut response = tiny_http::Response::from_string(response).with_status_code(201);
                if let Some(id) = request_id {
                    response.add_header(tiny_http::Header::from_bytes("x-request-id", id).unwrap());
                }
                request.respond(response).unwrap();
            });
            let result = service
                .pattern(Parameters(
                    serde_json::from_value(serde_json::json!({
                        "action": "create", "content": "Fixture guidance",
                        "category": "general", "priority": 1,
                        "propagation": "all_projects", "tags": "cas-d16e-fixture",
                    }))
                    .unwrap(),
                ))
                .await;
            reply.join().unwrap();
            assert!(
                server.try_recv().unwrap().is_none(),
                "create must not retry a write"
            );
            if created {
                let result = result.expect("Cloud returned the persisted pattern row");
                assert_ne!(result.is_error, Some(true));
                let text = result
                    .content
                    .iter()
                    .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(text.contains("Created pattern: pat-fixture-1"), "{text}");
                assert!(text.contains("Fixture guidance"), "{text}");
            } else {
                let error = result.expect_err("an unreadable post-write result is uncertain");
                assert_eq!(error.data.as_ref().unwrap()["outcome"], "uncertain");
                assert_eq!(error.data.as_ref().unwrap()["retry_safe"], false);
                assert!(error.message.contains("Do not retry"), "{error}");
                assert!(error.message.contains("pattern action=list"), "{error}");
                let reference = error.data.as_ref().unwrap()["reconciliation_id"].as_str().unwrap();
                if let Some(id) = request_id {
                    assert_eq!(reference, id);
                } else {
                    uuid::Uuid::parse_str(reference.strip_prefix("local-").unwrap()).unwrap();
                }
                assert!(error.message.contains(reference), "{error}");
            }
        }
    }

    #[tokio::test]
    async fn pattern_actions_accept_cloud_rows_and_legacy_wrappers_cas_5f69() {
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        // Cloud Production 7264056744 / 58f72237: [id]/route.ts GET:33,
        // PATCH:81 and adopt-rule/route.ts:82 return bare persisted rows.
        for wrapped in [false, true] {
            for (action, methods, expected) in [
                ("show", vec!["GET"], "Pattern: pat-fixture-1"),
                ("update", vec!["PATCH"], "Updated pattern: pat-fixture-1"),
                ("adopt", vec!["POST"], "Adopted rule rule-fixture as personal pattern: pat-fixture-1"),
                ("archive", vec!["DELETE"], "Pattern pat-fixture-1 archived (status: active)"),
                ("helpful", vec!["GET", "PATCH"], "marked as helpful (count: 9)"),
                ("harmful", vec!["GET", "PATCH"], "marked as harmful (count: 5)"),
            ] {
                let (_dir, server, service) = fixture();
                let http = server.clone();
                let reply = std::thread::spawn(move || {
                    for method in methods {
                        let mut request = http.recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap().expect("expected pattern action request");
                        assert_eq!(request.method().as_str(), method);
                        assert_eq!(request.url(), if action == "adopt" {
                            "/api/patterns/adopt-rule"
                        } else { "/api/patterns/pat-fixture-1" });
                        if matches!(method, "POST" | "PATCH") {
                            let body: serde_json::Value = serde_json::from_reader(request.as_reader()).unwrap();
                            match action {
                                "update" => assert_eq!(body["content"], "Fixture guidance"),
                                "adopt" => assert_eq!(body["rule_id"], "rule-fixture"),
                                "helpful" => assert_eq!(body["helpful_count"], 8),
                                "harmful" => assert_eq!(body["harmful_count"], 4),
                                _ => unreachable!(),
                            }
                        }
                        let row = serde_json::json!({
                            "id":"pat-fixture-1", "content":"Fixture guidance",
                            // Distinct from archive's default: the wire row is authoritative.
                            "status":"active",
                            "helpful_count": if method == "PATCH" { 9 } else { 7 },
                            "harmful_count": if method == "PATCH" { 5 } else { 3 },
                        });
                        let response = if wrapped { serde_json::json!({"pattern":row}) } else { row };
                        request.respond(tiny_http::Response::from_string(response.to_string())
                            .with_status_code(if action == "adopt" { 201 } else { 200 })).unwrap();
                    }
                });
                let result = service.pattern(Parameters(serde_json::from_value(serde_json::json!({
                    "action":action, "id":"pat-fixture-1", "rule_id":"rule-fixture",
                    "content":"Fixture guidance",
                })).unwrap())).await;
                reply.join().unwrap();
                let result = result.unwrap_or_else(|error| panic!("{action}, wrapped={wrapped}: {error}"));
                let text = result.content.iter().filter_map(|c| c.as_text().map(|t| t.text.as_str()))
                    .collect::<Vec<_>>().join("\n");
                assert!(text.contains(expected), "{action}, wrapped={wrapped}: {text}");
                assert!(server.try_recv().unwrap().is_none(), "no extra action request");
            }
        }
    }

    #[tokio::test]
    async fn pattern_post_send_timeouts_are_uncertain_without_retry_cas_5818() {
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        // pin: the injected test deadline must not lower the production 30 s one.
        assert_eq!(PATTERN_API_TIMEOUT, std::time::Duration::from_secs(30));
        // The request runs synchronously on this thread, so the override reaches it.
        PATTERN_API_TIMEOUT_OVERRIDE.with(|t| t.set(Some(std::time::Duration::from_secs(1))));
        let mut results = Vec::new();
        for (action, path, key, value) in [
            ("create", "/api/patterns", "content", "Fixture guidance"),
            ("adopt", "/api/patterns/adopt-rule", "rule_id", "rule-fixture"),
        ] {
        let (_dir, server, service) = fixture();
        let url = format!("http://{}{path}", server.server_addr());
        let http = server.clone();
        let (release, held) = std::sync::mpsc::channel();
        let reply = std::thread::spawn(move || {
            let mut request = http.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap().expect("one non-idempotent pattern write");
            assert_eq!(request.method().as_str(), "POST");
            assert_eq!(request.url(), path);
            let body: serde_json::Value = serde_json::from_reader(request.as_reader()).unwrap();
            assert_eq!(body[key], value);
            // Hold the fully read write past the client deadline (1 s here).
            // A channel releases the request after the client returns; no sleeps.
            held.recv_timeout(std::time::Duration::from_secs(40)).unwrap();
        });
        let result = service.pattern(Parameters(serde_json::from_value(serde_json::json!({
            "action":action, "content":"Fixture guidance", "rule_id":"rule-fixture",
        })).unwrap())).await;
        release.send(()).unwrap();
        reply.join().unwrap();
        assert!(server.try_recv().unwrap().is_none(), "an uncertain write must not retry");
        results.push((action, url, result));
        }
        // Finish both real requests before checking failures, so RED exercises adopt too.
        for (action, url, result) in results {
        let error = result.expect_err("request was sent, but no response arrived");
        let data = error.data.as_ref().expect("structured uncertain write outcome");
        assert_eq!(data["outcome"], "uncertain");
        assert_eq!(data["retry_safe"], false);
        let reference = data["reconciliation_id"].as_str().unwrap();
        uuid::Uuid::parse_str(reference.strip_prefix("local-").unwrap()).unwrap();
        assert!(error.message.contains(reference), "{error}");
        assert!(error.message.contains("Do not retry"), "{error}");
        assert!(error.message.contains("pattern action=list"), "{error}");
        assert!(error.message.contains("timed out reading response"), "{action}: {error}");
        assert!(error.message.contains(&url), "retain the underlying transport error: {error}");
        assert!(error.message.starts_with(&format!("Pattern {action} outcome uncertain:")), "{error}");
        }
    }

    #[tokio::test]
    async fn pattern_create_pre_send_failure_keeps_network_error_cas_5818() {
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        // Port zero cannot be a listening endpoint; no DNS or external server is involved.
        for (endpoint, cause) in [("http://127.0.0.1:0", "Connection Failed"), ("://bad", "Bad URL")] {
            let (dir, _server, service) = fixture();
            crate::cloud::CloudConfig {
                endpoint: endpoint.into(), token: Some("pattern-fixture-token".into()),
                ..Default::default()
            }.save_to_cas_dir(&dir.path().join(".cas")).unwrap();
            let error = service.pattern(Parameters(serde_json::from_value(serde_json::json!({
                "action":"create", "content":"Fixture guidance",
            })).unwrap())).await.expect_err("no request can reach Cloud");
            assert!(error.message.starts_with("Network error: "), "{error}");
            assert!(error.message.contains(cause), "retain transport detail: {error}");
            assert!(error.data.is_none(), "pre-send failure is not an uncertain write");
        }
    }
}
