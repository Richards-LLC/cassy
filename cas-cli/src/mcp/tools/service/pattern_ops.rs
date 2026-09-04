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

        let resp = self.pattern_api_call(
            "POST",
            "/api/patterns",
            Some(serde_json::Value::Object(payload)),
        )?;

        let pattern = resp
            .get("pattern")
            .ok_or_else(|| Self::error(ErrorCode::INTERNAL_ERROR, "Missing pattern in response"))?;

        Ok(Self::success(format!(
            "Created pattern: {}\n\n{}",
            pattern
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
            format_pattern(pattern),
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

        let pattern = resp
            .get("pattern")
            .ok_or_else(|| Self::error(ErrorCode::INTERNAL_ERROR, "Missing pattern in response"))?;

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

        let pattern = resp
            .get("pattern")
            .ok_or_else(|| Self::error(ErrorCode::INTERNAL_ERROR, "Missing pattern in response"))?;

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

        let pattern = resp.get("pattern");
        let status = pattern
            .and_then(|p| p.get("status"))
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

        let pattern = resp
            .get("pattern")
            .ok_or_else(|| Self::error(ErrorCode::INTERNAL_ERROR, "Missing pattern in response"))?;

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

        let pattern = show_resp
            .get("pattern")
            .ok_or_else(|| Self::error(ErrorCode::INTERNAL_ERROR, "Missing pattern in response"))?;

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

        let updated = resp.get("pattern");
        let new_count = updated
            .and_then(|p| p.get("helpful_count"))
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

        let pattern = show_resp
            .get("pattern")
            .ok_or_else(|| Self::error(ErrorCode::INTERNAL_ERROR, "Missing pattern in response"))?;

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

        let updated = resp.get("pattern");
        let new_count = updated
            .and_then(|p| p.get("harmful_count"))
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
            let timeout = std::time::Duration::from_secs(30);

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
                    if let Some(body) = body {
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
                Err(ureq::Error::Transport(e)) => Err(Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Network error: {e}"),
                )),
            }
        }
    }
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
