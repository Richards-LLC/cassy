use crate::mcp::tools::core::imports::*;

impl CasCore {
    // ========================================================================
    // Rule Tools (10)
    // ========================================================================

    /// List proven rules
    pub async fn cas_rules_list(&self) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;

        let rules = rule_store.list().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list rules: {e}")),
            data: None,
        })?;

        let proven_rules: Vec<_> = rules
            .iter()
            .filter(|r| r.status == RuleStatus::Proven)
            .collect();

        if proven_rules.is_empty() {
            return Ok(Self::success("No proven rules."));
        }

        let mut output = format!("Active Rules ({}):\n\n", proven_rules.len());
        for rule in proven_rules {
            output.push_str(&format!("- [{}] {}\n", rule.id, rule.preview(80)));
            if !rule.paths.is_empty() {
                output.push_str(&format!("  Paths: {}\n", rule.paths));
            }
        }

        Ok(Self::success(output))
    }

    /// Mark rule as helpful
    pub async fn cas_rule_helpful(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;

        let mut rule = rule_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Rule not found: {e}")),
            data: None,
        })?;

        rule.helpful_count += 1;
        rule.last_accessed = Some(chrono::Utc::now());

        let promoted = matches!(rule.status, RuleStatus::Draft | RuleStatus::Stale);
        if promoted {
            rule.status = RuleStatus::Proven;
        }

        rule_store.update(&rule).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        if promoted {
            let _ = self.sync_rules();
        }

        let mut msg = format!("Marked {} as helpful", req.id);
        if promoted {
            msg.push_str(" (promoted to Proven, synced to Claude Code)");
        }

        Ok(Self::success(msg))
    }

    /// Show rule details
    pub async fn cas_rule_show(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;

        let rule = rule_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Rule not found: {e}")),
            data: None,
        })?;

        let output = format!(
            "Rule: {}\n{}\n\nStatus: {:?}\nPaths: {}\nTags: {}\nSource entries: {}\nFeedback: +{} -{}\nCreated: {}\n\nContent:\n{}",
            rule.id,
            "=".repeat(rule.id.len() + 6),
            rule.status,
            if rule.paths.is_empty() {
                "all".to_string()
            } else {
                rule.paths.clone()
            },
            if rule.tags.is_empty() {
                "none".to_string()
            } else {
                rule.tags.join(", ")
            },
            if rule.source_ids.is_empty() {
                "none".to_string()
            } else {
                rule.source_ids.join(", ")
            },
            rule.helpful_count,
            rule.harmful_count,
            rule.created.format("%Y-%m-%d %H:%M"),
            rule.content
        );

        Ok(Self::success(output))
    }

    /// Create a new rule
    pub async fn cas_rule_create(
        &self,
        Parameters(req): Parameters<RuleCreateRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;

        let id = rule_store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

        let tags: Vec<String> = req
            .tags
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // Validate auto_approve_tools if provided
        if let Some(ref tools) = req.auto_approve_tools {
            let tool_list: Vec<&str> = tools.split(',').map(|t| t.trim()).collect();
            for tool in &tool_list {
                if Rule::DANGEROUS_TOOLS
                    .iter()
                    .any(|d| d.eq_ignore_ascii_case(tool))
                {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Cannot auto-approve dangerous tool '{}'. Dangerous tools ({}) require explicit approval.",
                            tool,
                            Rule::DANGEROUS_TOOLS.join(", ")
                        )),
                        data: None,
                    });
                }
            }
        }

        let rule = Rule {
            id: id.clone(),
            scope: Scope::default(),
            content: req.content,
            paths: req.paths.unwrap_or_default(),
            tags,
            status: RuleStatus::Draft,
            helpful_count: 0,
            harmful_count: 0,
            created: chrono::Utc::now(),
            last_accessed: None,
            source_ids: req
                .source_ids
                .map(|ids| {
                    ids.split(',')
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            review_after: None,
            hook_command: None,
            category: crate::types::RuleCategory::default(),
            priority: 2,
            surface_count: 0,
            auto_approve_tools: req.auto_approve_tools,
            auto_approve_paths: req.auto_approve_paths,
            team_id: None,
            share: None,
        };

        rule_store.add(&rule).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to create rule: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!("Created rule: {id}")))
    }

    /// Mark rule as harmful
    pub async fn cas_rule_harmful(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;

        let mut rule = rule_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Rule not found: {e}")),
            data: None,
        })?;

        rule.harmful_count += 1;

        rule_store.update(&rule).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!(
            "Marked {} as harmful (score: {})",
            req.id,
            rule.helpful_count - rule.harmful_count
        )))
    }

    /// Delete a rule
    pub async fn cas_rule_delete(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;

        rule_store.delete(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to delete: {e}")),
            data: None,
        })?;

        Ok(Self::success(format!("Deleted rule: {}", req.id)))
    }

    /// Sync rules to Claude Code
    pub async fn cas_rule_sync(&self) -> Result<CallToolResult, McpError> {
        let synced = self.sync_rules()?;
        Ok(Self::success(format!(
            "Synced {synced} rules to Claude Code"
        )))
    }

    // ========================================================================
    // Additional Rule Tools
    // ========================================================================

    /// Update a rule
    pub async fn cas_rule_update(
        &self,
        Parameters(req): Parameters<RuleUpdateRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;

        let mut rule = rule_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Rule not found: {e}")),
            data: None,
        })?;

        let mut changes = Vec::new();

        if let Some(content) = req.content {
            rule.content = content;
            changes.push("content");
        }

        if let Some(paths) = req.paths {
            rule.paths = paths;
            changes.push("paths");
        }

        if let Some(tags) = req.tags {
            rule.tags = tags
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            changes.push("tags");
        }

        if let Some(ref tools) = req.auto_approve_tools {
            // Validate tools before setting
            let tool_list: Vec<&str> = tools.split(',').map(|t| t.trim()).collect();
            for tool in &tool_list {
                if Rule::DANGEROUS_TOOLS
                    .iter()
                    .any(|d| d.eq_ignore_ascii_case(tool))
                {
                    return Err(McpError {
                        code: ErrorCode::INVALID_PARAMS,
                        message: Cow::from(format!(
                            "Cannot auto-approve dangerous tool '{}'. Dangerous tools ({}) require explicit approval.",
                            tool,
                            Rule::DANGEROUS_TOOLS.join(", ")
                        )),
                        data: None,
                    });
                }
            }
            rule.auto_approve_tools = req.auto_approve_tools;
            changes.push("auto_approve_tools");
        }

        if req.auto_approve_paths.is_some() {
            rule.auto_approve_paths = req.auto_approve_paths;
            changes.push("auto_approve_paths");
        }

        if changes.is_empty() {
            return Ok(Self::success("No changes specified"));
        }

        rule_store.update(&rule).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        // Re-sync if proven
        if rule.status == RuleStatus::Proven {
            let _ = self.sync_rules();
        }

        Ok(Self::success(format!(
            "Updated rule {}: {}",
            req.id,
            changes.join(", ")
        )))
    }

    /// List all rules (not just proven)
    pub async fn cas_rule_list_all(
        &self,
        Parameters(req): Parameters<LimitRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;

        let rules = rule_store.list().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list: {e}")),
            data: None,
        })?;

        if rules.is_empty() {
            return Ok(Self::success("No rules found"));
        }

        let limit = req.limit.unwrap_or(20);
        let mut output = format!(
            "All rules ({} total, showing {}):\n\n",
            rules.len(),
            rules.len().min(limit)
        );
        for rule in rules.iter().take(limit) {
            output.push_str(&format!(
                "- [{}] {:?} (+{} -{}) {}\n",
                rule.id,
                rule.status,
                rule.helpful_count,
                rule.harmful_count,
                rule.preview(60)
            ));
        }

        if rules.len() > limit {
            output.push_str(&format!("\n... and {} more", rules.len() - limit));
        }

        Ok(Self::success(output))
    }
}
