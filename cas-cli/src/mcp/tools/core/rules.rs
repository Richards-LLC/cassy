use crate::config::{Config, parse_promotion_evidence};
use crate::mcp::tools::core::imports::*;
use crate::store::foreign_project_guard::ForeignProjectGuard;
use cas_store::{RetrievalAggregate, SqliteRetrievalStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromotionEvidence {
    Helpful,
    Retrieval,
}

// Verification verdicts are intentionally not guessed into this enum: the
// verification store currently keys verdicts by task_id and has no stable
// rule_id join. A future evidence source must use an explicit rule_id (or
// immutable source ID), otherwise a verdict could be attributed to the wrong
// rule. Retrieval outcomes are the available third-party signal today.

fn configured_promotion_evidence(config: &Config) -> Result<Vec<PromotionEvidence>, McpError> {
    parse_promotion_evidence(&config.sync.promotion_evidence.join(","))
        .map(|sources| {
            sources
                .into_iter()
                .map(|source| match source.as_str() {
                    "helpful" => PromotionEvidence::Helpful,
                    "retrieval" => PromotionEvidence::Retrieval,
                    // `parse_promotion_evidence` validates this list above.
                    _ => unreachable!("validated promotion evidence source"),
                })
                .collect()
        })
        .map_err(|error| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Invalid rule promotion configuration: {error}")),
            data: None,
        })
}

fn promotion_threshold(config: &Config) -> i32 {
    // A raw TOML edit can bypass Config::set validation. Keep the one-call
    // invariant true even for such configs; the supported setting rejects
    // values below two at the config surface.
    config.sync.promotion_threshold.max(2)
}

fn demotion_threshold(config: &Config) -> i32 {
    // Keep the one-event safety floor even when a raw TOML edit bypasses the
    // Config registry. A single self-reported negative event is not enough to
    // remove a rule that may already be relied upon by future sessions.
    config.sync.demotion_threshold.max(2)
}

impl CasCore {
    // ========================================================================
    // Rule Tools (10)
    // ========================================================================

    fn retrieval_aggregates_for_rule(
        &self,
        rule_id: &str,
    ) -> Result<Vec<RetrievalAggregate>, McpError> {
        let retrieval_store =
            SqliteRetrievalStore::open(&self.cas_root).map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open retrieval feedback: {error}")),
                data: None,
            })?;
        retrieval_store
            .aggregate_for_result(rule_id)
            .map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to read retrieval feedback: {error}")),
                data: None,
            })
    }

    fn retrieval_is_negative(aggregates: &[RetrievalAggregate]) -> bool {
        aggregates.iter().any(|aggregate| {
            aggregate.document_type == "rule"
                && (aggregate.corrected > 0
                    || aggregate.harmful > 0
                    || aggregate.correction_rate > 0.0)
        })
    }

    fn retrieval_meets_promotion_threshold(
        aggregates: &[RetrievalAggregate],
        threshold: i32,
    ) -> bool {
        let threshold = threshold as u64;
        aggregates.iter().any(|aggregate| {
            if aggregate.document_type != "rule" {
                return false;
            }
            let useful = aggregate.used.saturating_add(aggregate.helpful);
            // Session diversity is a cheap, privacy-preserving proxy for
            // independent validation. Same-session self-report can be gamed,
            // so third-party retrieval outcomes must come from multiple
            // sessions before they outweigh a single caller's feedback.
            useful >= threshold
                && aggregate.distinct_sessions >= 2
                && aggregate.usefulness_rate >= 0.5
                && aggregate.correction_rate == 0.0
                && aggregate.harmful == 0
        })
    }

    fn retrieval_meets_demotion_threshold(
        aggregates: &[RetrievalAggregate],
        threshold: i32,
    ) -> bool {
        let threshold = threshold as u64;
        aggregates.iter().any(|aggregate| {
            if aggregate.document_type != "rule" {
                return false;
            }
            // Negative retrieval evidence follows the same independent-session
            // guard as positive evidence. Repeated self-report from one
            // session is too easy to game to demote a Proven rule by itself.
            aggregate.distinct_sessions >= 2
                && aggregate.corrected.saturating_add(aggregate.harmful) >= threshold
        })
    }

    /// Apply negative retrieval evidence at an explicit sync boundary. The
    /// retrieval store remains append-only and observational; only this rule
    /// decision path changes Rule state.
    fn refresh_retrieval_demotions(&self) -> Result<usize, McpError> {
        let config = self.load_config();
        let threshold = demotion_threshold(&config);
        let retrieval_store =
            SqliteRetrievalStore::open(&self.cas_root).map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open retrieval feedback: {error}")),
                data: None,
            })?;
        let rule_store = self.open_rule_store()?;
        let rules = rule_store.list().map_err(|error| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list rules: {error}")),
            data: None,
        })?;

        let mut demoted = 0;
        for mut rule in rules {
            if rule.status != RuleStatus::Proven {
                continue;
            }
            let aggregates = retrieval_store
                .aggregate_for_result(&rule.id)
                .map_err(|error| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to read retrieval feedback: {error}")),
                    data: None,
                })?;
            if Self::retrieval_meets_demotion_threshold(&aggregates, threshold) {
                rule.status = RuleStatus::Stale;
                rule_store
                    .update_with_metadata(
                        &rule,
                        None,
                        Some("demoted after retrieval evidence threshold"),
                    )
                    .map_err(|error| McpError {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: Cow::from(format!("Failed to demote rule: {error}")),
                        data: None,
                    })?;
                demoted += 1;
            }
        }
        Ok(demoted)
    }

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
            output.push_str(&format!(
                "  Impact: surfaced {} | feedback: +{} helpful, -{} harmful\n",
                rule.surface_count, rule.helpful_count, rule.harmful_count
            ));
        }

        Ok(Self::success(output))
    }

    /// Mark rule as helpful
    pub async fn cas_rule_helpful(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;
        let config = self.load_config();
        let evidence_sources = configured_promotion_evidence(&config)?;

        let mut rule = rule_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Rule not found: {e}")),
            data: None,
        })?;

        let retrieval_aggregates = if evidence_sources.contains(&PromotionEvidence::Retrieval)
            || rule.status == RuleStatus::Proven
        {
            self.retrieval_aggregates_for_rule(&rule.id)?
        } else {
            Vec::new()
        };
        let retrieval_negative = Self::retrieval_is_negative(&retrieval_aggregates);
        let retrieval_demotion = Self::retrieval_meets_demotion_threshold(
            &retrieval_aggregates,
            demotion_threshold(&config),
        );

        rule.helpful_count += 1;
        rule.last_accessed = Some(chrono::Utc::now());

        let threshold = promotion_threshold(&config);
        let helpful_evidence = rule.helpful_count >= threshold && rule.harmful_count == 0;
        let retrieval_evidence =
            Self::retrieval_meets_promotion_threshold(&retrieval_aggregates, threshold);
        let has_evidence = evidence_sources.iter().any(|source| match source {
            PromotionEvidence::Helpful => helpful_evidence,
            PromotionEvidence::Retrieval => retrieval_evidence,
        });
        let demoted = rule.status == RuleStatus::Proven && retrieval_demotion;
        if demoted {
            rule.status = RuleStatus::Stale;
        }
        let promoted = !demoted
            && !retrieval_negative
            && matches!(rule.status, RuleStatus::Draft | RuleStatus::Stale)
            && has_evidence;
        if promoted {
            rule.status = RuleStatus::Proven;
        }

        rule_store
            .update_with_metadata(
                &rule,
                None,
                demoted.then_some("demoted after retrieval evidence threshold"),
            )
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to update: {e}")),
                data: None,
            })?;

        if promoted || demoted {
            let _ = self.sync_rules();
        }

        let mut msg = format!("Marked {} as helpful", req.id);
        if promoted {
            msg.push_str(&format!(
                " (promoted to Proven after {threshold} evidence events, synced to Claude Code)"
            ));
        } else if demoted {
            msg.push_str(
                " (negative retrieval evidence demoted it to Stale, removed from Claude Code)",
            );
        } else if rule.status != RuleStatus::Proven {
            msg.push_str(&format!(
                " ({}/{threshold} evidence events; remains {:?})",
                rule.helpful_count, rule.status
            ));
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
            "Rule: {}\n{}\n\nStatus: {:?}\nPaths: {}\nTags: {}\nSource entries: {}\nImpact: surfaced {} | feedback: +{} helpful, -{} harmful\nCreated: {}\n\nContent:\n{}",
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
            rule.surface_count,
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
        let tags: Vec<String> = req
            .tags
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // cas-caae (skills audit M27/M83): a rule naming another registered
        // project belongs in that project's store, unless tagged
        // `project:<slug>` as a deliberate cross-project rule.
        let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
        if let Some(refusal) =
            ForeignProjectGuard::for_project_root(project_root).check_rule(&req.content, &tags)
        {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(refusal.to_string()),
                data: None,
            });
        }

        let rule_store = self.open_rule_store()?;

        let id = rule_store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

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

        let mut rule = Rule {
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
            category: crate::types::RuleCategory::default(),
            priority: 2,
            surface_count: 0,
            auto_approve_tools: req.auto_approve_tools,
            auto_approve_paths: req.auto_approve_paths,
            team_id: None,
            share: None,
            operator_authority: None,
        };

        // cas-5372 (GH #990): only the operator or a registered supervisor
        // can make a hard rule take effect before promotion. Anyone else's
        // "HARD RULE" text is an ordinary draft.
        let hard_rule_author = rule
            .looks_like_hard_rule()
            .then(|| self.operator_hard_rule_author());
        if let Some(Ok(author)) = &hard_rule_author {
            rule.authorize_operator_hard_rule(author);
        }

        rule_store.add(&rule).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to create rule: {e}")),
            data: None,
        })?;

        match hard_rule_author {
            Some(Ok(author)) => {
                let _ = self.sync_rules();
                Ok(Self::success(format!(
                    "Created rule: {id} (operator hard rule authorised by {author}: synced to Claude Code and surfaced at session start, labelled DRAFT until promoted)"
                )))
            }
            Some(Err(refusal)) => Ok(Self::success(format!(
                "Created rule: {id} as an ordinary draft. It reads like a hard rule, but {refusal}, so it waits for promotion like any other draft."
            ))),
            None => Ok(Self::success(format!("Created rule: {id}"))),
        }
    }

    /// cas-5372: who may authorise an operator hard rule — a registered
    /// supervisor, or the operator's own registered session outside a
    /// factory. Returns the author label, or why this caller may not.
    fn operator_hard_rule_author(&self) -> Result<String, String> {
        let unregistered =
            || "the operator hard-rule fast path is only for the operator or a registered supervisor, and this caller is not registered".to_string();
        let id = self.get_registered_agent_id_read_only().map_err(|_| unregistered())?;
        let agent = self
            .open_agent_store()
            .ok()
            .and_then(|store| store.get(&id).ok())
            .ok_or_else(unregistered)?;
        match agent.role {
            crate::types::AgentRole::Supervisor => Ok(format!("supervisor:{}", agent.name)),
            crate::types::AgentRole::Standard
                if agent.factory_session.is_none()
                    && std::env::var_os("CAS_FACTORY_MODE").is_none() =>
            {
                Ok(format!("operator:{}", agent.name))
            }
            role => Err(format!(
                "the operator hard-rule fast path is only for the operator or a registered supervisor, and this caller is a {role} ({})",
                agent.name
            )),
        }
    }

    /// Promote a rule to Proven on a reviewer's recorded decision (audit
    /// D11). `helpful` is a vote: it promotes only once votes reach
    /// `sync.promotion_threshold`, and a reviewer voting on rules it judges
    /// inflates the very metric it judges by. `promote` is the explicit
    /// decision instead: it needs a reason, which rule history records, and it
    /// never adds a helpful vote. It raises `helpful_count` only to the
    /// `sync.min_helpful` floor rule-file sync requires, so the rule reaches
    /// Claude Code.
    pub async fn cas_rule_promote(
        &self,
        id: String,
        change_note: Option<String>,
        changed_by: Option<String>,
    ) -> Result<CallToolResult, McpError> {
        let invalid = |message: String| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(message),
            data: None,
        };
        let reason = change_note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .ok_or_else(|| {
                invalid(format!(
                    "rule action=promote requires change_note: say why {id} deserves Proven (it is recorded in rule history)"
                ))
            })?
            .to_string();

        let rule_store = self.open_rule_store()?;
        let config = self.load_config();
        let mut rule = rule_store
            .get(&id)
            .map_err(|e| invalid(format!("Rule not found: {e}")))?;

        match rule.status {
            RuleStatus::Proven => {
                return Ok(Self::success(format!("{id} is already Proven; nothing changed")));
            }
            RuleStatus::Retired => {
                return Err(invalid(format!(
                    "{id} is retired; restore it with rule action=restore before promoting"
                )));
            }
            RuleStatus::Draft | RuleStatus::Stale => {}
        }
        if rule.harmful_count > 0 {
            return Err(invalid(format!(
                "{id} has {} harmful report(s); rewrite or retire it instead of promoting",
                rule.harmful_count
            )));
        }
        let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
        if let Some(refusal) =
            ForeignProjectGuard::for_project_root(project_root).check_rule(&rule.content, &rule.tags)
        {
            return Err(invalid(refusal.to_string()));
        }

        let floor = config.sync.min_helpful.max(0);
        let raised_to_floor = rule.helpful_count < floor;
        rule.helpful_count = rule.helpful_count.max(floor);
        rule.status = RuleStatus::Proven;
        rule.last_accessed = Some(chrono::Utc::now());
        let note = format!("promoted: {reason}");
        rule_store
            .update_with_metadata(&rule, changed_by.as_deref(), Some(&note))
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to update: {e}")),
                data: None,
            })?;
        let _ = self.sync_rules();

        let mut msg = format!("Promoted {id} to Proven (synced to Claude Code)");
        if raised_to_floor {
            msg.push_str(&format!(
                "; helpful_count raised to the sync floor of {floor}"
            ));
        }
        Ok(Self::success(msg))
    }

    /// Mark rule as harmful
    pub async fn cas_rule_harmful(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;
        let config = self.load_config();

        let mut rule = rule_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Rule not found: {e}")),
            data: None,
        })?;

        rule.harmful_count += 1;
        let demoted = rule.status == RuleStatus::Proven
            && rule.harmful_count >= demotion_threshold(&config);
        if demoted {
            rule.status = RuleStatus::Stale;
        }

        rule_store
            .update_with_metadata(
                &rule,
                None,
                demoted.then_some("demoted after harmful evidence threshold"),
            )
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to update: {e}")),
                data: None,
            })?;

        if demoted {
            let _ = self.sync_rules();
        }

        let suffix = if demoted {
            "; demoted to Stale and removed from Claude Code"
        } else {
            " (negative evidence below demotion threshold)"
        };
        Ok(Self::success(format!(
            "Marked {} as harmful (score: {}){}",
            req.id,
            rule.helpful_count - rule.harmful_count,
            suffix
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

        Ok(Self::success(format!(
            "Retired rule: {} (history retained)",
            req.id
        )))
    }

    /// Sync rules to Claude Code
    pub async fn cas_rule_sync(&self) -> Result<CallToolResult, McpError> {
        self.refresh_retrieval_demotions()?;
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

        // cas-caae: the M27 rule reached its store as an edit of an existing
        // row, so a content or tag edit gets the same guard as create.
        if changes.contains(&"content") || changes.contains(&"tags") {
            let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
            if let Some(refusal) = ForeignProjectGuard::for_project_root(project_root)
                .check_rule(&rule.content, &rule.tags)
            {
                return Err(McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(refusal.to_string()),
                    data: None,
                });
            }
        }

        // cas-5372: an authorised caller's edit re-authorises the hard rule
        // for its new text. Anyone else's content edit leaves the old
        // authorisation bound to the old text, which no longer matches.
        let was_hard_rule = rule.is_operator_hard_rule();
        if rule.looks_like_hard_rule()
            && (changes.contains(&"content") || changes.contains(&"tags"))
            && let Ok(author) = self.operator_hard_rule_author()
        {
            rule.authorize_operator_hard_rule(&author);
        }

        rule_store
            .update_with_metadata(&rule, req.changed_by.as_deref(), req.change_note.as_deref())
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to update: {e}")),
                data: None,
            })?;

        // Re-sync if proven, or if the rule is or was an operator hard rule.
        if rule.status == RuleStatus::Proven || was_hard_rule || rule.is_operator_hard_rule() {
            let _ = self.sync_rules();
        }

        Ok(Self::success(format!(
            "Updated rule {}: {}",
            req.id,
            changes.join(", ")
        )))
    }

    /// List prior rule states, newest first.
    pub async fn cas_rule_history(
        &self,
        Parameters(req): Parameters<VersionRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;
        let versions = rule_store.list_versions(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list rule history: {e}")),
            data: None,
        })?;
        if versions.is_empty() {
            return Ok(Self::success(format!("No history for rule {}", req.id)));
        }

        let mut output = format!(
            "Rule history for {} ({} versions):\n\n",
            req.id,
            versions.len()
        );
        for version in versions {
            let preview: String = version.content.chars().take(120).collect();
            output.push_str(&format!(
                "- v{} [{}: {}] {} by {} at {}\n  {}\n",
                version.version,
                version.status,
                version.operation,
                version.change_note,
                version.changed_by.as_deref().unwrap_or("unknown actor"),
                version.changed_at.format("%Y-%m-%d %H:%M:%S UTC"),
                preview,
            ));
        }
        Ok(Self::success(output))
    }

    /// Restore a prior rule state, or un-retire to the newest prior state.
    pub async fn cas_rule_restore(
        &self,
        Parameters(req): Parameters<VersionRequest>,
    ) -> Result<CallToolResult, McpError> {
        let rule_store = self.open_rule_store()?;
        let version = req.version.or(req.version_id);
        rule_store
            .restore_version(
                &req.id,
                version,
                req.changed_by.as_deref(),
                req.change_note.as_deref(),
            )
            .map_err(|e| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Failed to restore rule: {e}")),
                data: None,
            })?;
        let restored = rule_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to read restored rule: {e}")),
            data: None,
        })?;
        let _ = self.sync_rules();
        Ok(Self::success(format!(
            "Restored rule {}{} (status: {})",
            req.id,
            version
                .map(|v| format!(" to version {v}"))
                .unwrap_or_default(),
            restored.status
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
                "- [{}] {:?} (surfaced: {}, feedback: +{} -{}) {}\n",
                rule.id,
                rule.status,
                rule.surface_count,
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
