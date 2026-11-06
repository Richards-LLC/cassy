use crate::config::{Config, parse_promotion_evidence};
use crate::mcp::tools::core::imports::*;
use crate::store::foreign_project_guard::ForeignProjectGuard;
use cas_store::{RetrievalAggregate, SqliteRetrievalStore};
use std::collections::HashSet;

/// Explicit attribution prevents an agent's interpretation from becoming the evidence.
fn apply_stakeholder_attribution(
    rule: &mut Rule,
    name: Option<String>,
    quote: Option<String>,
    scope: Option<String>,
    review_after: Option<String>,
) -> Result<(), McpError> {
    let invalid = |message| McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: Cow::from(message),
        data: None,
    };
    let tagged_name = rule
        .tags
        .iter()
        .find_map(|tag| tag.strip_prefix("stakeholder:"))
        .map(str::to_owned);
    let has_input = name.is_some() || quote.is_some() || scope.is_some() || tagged_name.is_some();
    if has_input {
        let old = rule.stakeholder.take();
        let attribution = cas_types::StakeholderAttribution {
            name: name
                .or(tagged_name)
                .or_else(|| old.as_ref().map(|s| s.name.clone()))
                .unwrap_or_default(),
            quote: quote
                .or_else(|| old.as_ref().map(|s| s.quote.clone()))
                .unwrap_or_default(),
            scope: scope
                .or_else(|| old.as_ref().map(|s| s.scope.clone()))
                .unwrap_or_default(),
        };
        if attribution.name.trim().is_empty()
            || attribution.quote.trim().is_empty()
            || attribution.scope.trim().is_empty()
        {
            return Err(invalid(
                "Stakeholder attribution requires stakeholder, stakeholder_quote (verbatim words), and stakeholder_scope",
            ));
        }
        rule.stakeholder = Some(attribution);
    }
    if let Some(timestamp) = review_after {
        rule.review_after = Some(
            chrono::DateTime::parse_from_rfc3339(&timestamp)
                .map_err(|_| invalid("review_after must be an RFC3339 timestamp"))?
                .with_timezone(&chrono::Utc),
        );
    }
    if rule.stakeholder.is_some()
        && rule.looks_like_hard_rule()
        && !rule
            .review_after
            .is_some_and(|deadline| deadline > chrono::Utc::now())
    {
        return Err(invalid(
            "Stakeholder-attributed hard rules require a future review_after",
        ));
    }
    Ok(())
}

fn enforceable_mechanism(rule: &Rule) -> Option<&'static str> {
    if rule.status == RuleStatus::Retired || rule.harmful_count > 0 {
        return None;
    }
    ["lint", "test", "hook", "gate", "type"]
        .into_iter()
        .find(|mechanism| {
            rule.tags
                .iter()
                .any(|tag| tag == &format!("enforceable:{mechanism}"))
        })
}

fn distinct_rule_sources(rule: &Rule) -> usize {
    rule.source_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .collect::<HashSet<_>>()
        .len()
}

/// Encode chores belong to the project backlog rather than an arbitrary epic.
/// Show them at the supervisor's assembly boundary even when they are not children.
pub(crate) fn render_pending_encode_chores(tasks: &[Task]) -> String {
    let mut pending: Vec<_> = tasks
        .iter()
        .filter(|task| {
            !task.is_terminal()
                && task.task_type == TaskType::Chore
                && task.external_ref.as_deref().is_some_and(|reference| {
                    reference.starts_with("rule-encode:")
                        || reference.starts_with("rule-candidate:")
                })
        })
        .collect();
    if pending.is_empty() {
        return String::new();
    }
    pending.sort_by(|a, b| a.id.cmp(&b.id));
    let mut output = format!(
        "\nPending rule encode chores (project backlog: {}):\n",
        pending.len()
    );
    for task in pending.iter().take(20) {
        output.push_str(&format!(
            "- {} [{}] {} (assignee: {})\n",
            task.id,
            task.status,
            task.title.replace(['\n', '\r'], " "),
            task.assignee.as_deref().unwrap_or("unassigned")
        ));
    }
    if pending.len() > 20 {
        output.push_str(&format!(
            "... {} more; task action=list task_type=chore\n",
            pending.len() - 20
        ));
    }
    output
}

/// A rule candidate is raised for a promote decision once it has been
/// observed in this many different tasks (cas-f0a1).
pub(crate) const CANDIDATE_DECISION_TASKS: usize = 2;

fn encode_chore_ref(rule_id: &str) -> String {
    format!("rule-encode:{rule_id}")
}

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

    /// A durable external reference makes repeat creation/tagging/promotion
    /// and a retry after partial failure resolve to the same backlog item,
    /// including after the original chore has closed.
    fn ensure_encode_chore(&self, rule: &Rule) -> Result<Option<String>, McpError> {
        let Some(mechanism) = enforceable_mechanism(rule) else {
            return Ok(None);
        };
        // Reuse the HARD RULE caller check, but its supervisor fast path
        // does not authorise turning a rule into implementation work.
        if !self
            .operator_hard_rule_author()
            .is_ok_and(|author| author.starts_with("operator:"))
        {
            return Ok(None);
        }
        let task_store = self.open_task_store()?;
        let external_ref = encode_chore_ref(&rule.id);
        if let Some(existing) = task_store
            .list(None)
            .map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to list encode chores: {error}")),
                data: None,
            })?
            .into_iter()
            .find(|task| task.external_ref.as_deref() == Some(external_ref.as_str()))
        {
            return Ok(Some(existing.id));
        }

        let id = task_store.generate_id().map_err(|error| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate encode chore ID: {error}")),
            data: None,
        })?;
        let mut chore = Task::new(id.clone(), format!("encode {} as {mechanism}", rule.id));
        chore.task_type = TaskType::Chore;
        chore.origin_project = task_store.project_id().map(str::to_owned);
        chore.external_ref = Some(external_ref);
        let sources = if rule.source_ids.is_empty() {
            format!(
                "{} (legacy rule record; recover incident provenance from rule history)",
                rule.id
            )
        } else {
            rule.source_ids.join(", ")
        };
        chore.description = format!(
            "Encode rule {} as a {mechanism} mechanism. Rule content: {}\nSource entry IDs: {}",
            rule.id, rule.content, sources
        );
        chore.acceptance_criteria = format!(
            "The {mechanism} enforces rule {} and has a regression check covering the cited sources.",
            rule.id
        );
        task_store
            .create_atomic(&chore, &[], None, Some("rule-encode"))
            .map_err(|error| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Failed to create encode chore for {}: {error}",
                    rule.id
                )),
                data: None,
            })?;
        Ok(Some(id))
    }

    /// Every rule-write response explains how the operator can request encoding.
    fn encode_chore_suffix(&self, rule: &Rule) -> Result<String, McpError> {
        Ok(match self.ensure_encode_chore(rule)? {
            Some(id) => format!("; encode chore {id} exists"),
            None if enforceable_mechanism(rule).is_some() => format!(
                "; no encode chore created: operator authorisation required. Operator command in your registered session outside the factory: rule action=update id={} tags={:?}",
                rule.id, rule.tags.join(",")
            ),
            None => String::new(),
        })
    }

    /// cas-56266: list the cited sites a rule change leaves for review in one
    /// linked cleanup task. The rule change is already stored, so a listing
    /// failure is reported rather than undoing it.
    fn rule_cleanup_suffix(&self, rule: &Rule, changes: &[crate::rule_refs::RuleChange]) -> String {
        if changes.is_empty() {
            return String::new();
        }
        let filed = (|| -> anyhow::Result<Option<String>> {
            let message = |error: McpError| anyhow::anyhow!(error.message.into_owned());
            let version = self
                .open_rule_store()
                .map_err(message)?
                .list_versions(&rule.id)?
                .iter()
                .map(|version| version.version)
                .max()
                .unwrap_or_default();
            let tasks = self.open_task_store().map_err(message)?;
            crate::rule_refs::file_cleanup(&self.cas_root, tasks.as_ref(), rule, changes, &format!("v{version}"))
        })();
        match filed {
            Ok(Some(id)) => format!("; cleanup task {id} lists its cited sites"),
            Ok(None) => String::new(),
            Err(error) => format!("; cited-site cleanup listing failed: {error:#}"),
        }
    }

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

        let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
        let config = self.load_config();
        let target = project_root.join(&config.sync.target);
        let mut output = String::from("Rule sources:\n\n");
        for rule in &rules {
            let held = crate::store::SyncingRuleStore::is_held(project_root, &rule.id)
                && rule.status == RuleStatus::Stale;
            if rule.status != RuleStatus::Proven && !rule.is_active_operator_hard_rule() && !held {
                continue;
            }
            output.push_str(&format!("- [{}] {}\n  source: store (.cas; projection: {})\n",
                rule.id, rule.preview(80), config.sync.target));
            if held {
                output.push_str("  held: file absent; excluded from prompts and sync. Use rule action=restore to keep, or delete to retire.\n");
            }
            if !rule.paths.is_empty() {
                output.push_str(&format!("  Paths: {}\n", rule.paths));
            }
            output.push_str(&format!(
                "  Impact: surfaced {} | feedback: +{} helpful, -{} harmful\n",
                rule.surface_count, rule.helpful_count, rule.harmful_count
            ));
        }
        // Repo-authored files are policy too. A stored rule's generated file
        // is one projection of that source, not a duplicate repository rule.
        let generated: HashSet<_> = rules.iter().map(|rule| target.join(format!("{}.md", rule.id))).collect();
        let repo_rules = project_root.join(".claude/rules");
        let mut files: Vec<_> = walkdir::WalkDir::new(&repo_rules).into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "md"))
            .map(|entry| entry.into_path()).filter(|path| !generated.contains(path)).collect();
        files.sort();
        for file in files {
            output.push_str(&format!("- source: repo ({})\n", file.strip_prefix(project_root).unwrap_or(&file).display()));
        }
        if output == "Rule sources:\n\n" { output.push_str("No active or held project rules.\n"); }

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

        let chore_suffix = self.encode_chore_suffix(&rule)?;
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

        msg.push_str(&chore_suffix);
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

        let mut output = output;
        if let Some(attribution) = &rule.stakeholder {
            output.push_str(&format!(
                "\n\nStakeholder: {}\nVerbatim words:\n{}\nScope:\n{}\nReview after: {}",
                attribution.name,
                attribution.quote,
                attribution.scope,
                rule.review_after
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| "not set".into())
            ));
        }
        Ok(Self::success(output))
    }

    /// Create a new rule
    pub async fn cas_rule_create(
        &self,
        Parameters(req): Parameters<RuleCreateRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.scope != "project" {
            return Err(Self::error(ErrorCode::INVALID_PARAMS,
                "Project rule creation requires scope=project; personal rules use the unified rule action=create scope=global with operator provenance"));
        }
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
            origin_project: None,
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
            stakeholder: None,
        };

        apply_stakeholder_attribution(
            &mut rule,
            req.stakeholder,
            req.stakeholder_quote,
            req.stakeholder_scope,
            req.review_after,
        )?;

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

        let chore_suffix = self.encode_chore_suffix(&rule)?;
        match hard_rule_author {
            Some(Ok(author)) => {
                let _ = self.sync_rules();
                Ok(Self::success(format!(
                    "Created rule: {id} (operator hard rule authorised by {author}: synced to Claude Code and surfaced at session start, labelled DRAFT until promoted){chore_suffix}"
                )))
            }
            Some(Err(refusal)) => Ok(Self::success(format!(
                "Created rule: {id} as an ordinary draft. It reads like a hard rule, but {refusal}, so it waits for promotion like any other draft.{chore_suffix}"
            ))),
            None => Ok(Self::success(format!("Created rule: {id}{chore_suffix}"))),
        }
    }

    /// cas-5372: who may authorise an operator hard rule — a registered
    /// supervisor, or the operator's own registered session outside a
    /// factory. Returns the author label, or why this caller may not.
    pub(crate) fn operator_hard_rule_author(&self) -> Result<String, String> {
        let unregistered = || {
            "the operator hard-rule fast path is only for the operator or a registered supervisor, and this caller is not registered".to_string()
        };
        let id = self
            .get_registered_agent_id_read_only()
            .map_err(|_| unregistered())?;
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
                let suffix = self.encode_chore_suffix(&rule)?;
                return Ok(Self::success(if suffix.is_empty() {
                    format!("{id} is already Proven; nothing changed")
                } else {
                    format!("{id} is already Proven{suffix}")
                }));
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
        if let Some(refusal) = ForeignProjectGuard::for_project_root(project_root)
            .check_rule(&rule.content, &rule.tags)
        {
            return Err(invalid(refusal.to_string()));
        }

        if enforceable_mechanism(&rule).is_none() && distinct_rule_sources(&rule) < 2 {
            return Err(invalid(format!(
                "{id} is a judgement rule; promotion requires at least two distinct source IDs"
            )));
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
        let chore_suffix = self.encode_chore_suffix(&rule)?;

        let mut msg = format!("Promoted {id} to Proven (synced to Claude Code)");
        if raised_to_floor {
            msg.push_str(&format!(
                "; helpful_count raised to the sync floor of {floor}"
            ));
        }
        msg.push_str(&chore_suffix);
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
        let demoted =
            rule.status == RuleStatus::Proven && rule.harmful_count >= demotion_threshold(&config);
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
        let before = rule_store.get(&req.id).ok();

        rule_store.delete(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to delete: {e}")),
            data: None,
        })?;
        let cleanup = match (before, rule_store.get(&req.id)) {
            (Some(before), Ok(after)) => {
                self.rule_cleanup_suffix(&after, &crate::rule_refs::update_changes(&before, &after))
            }
            _ => String::new(),
        };

        Ok(Self::success(format!(
            "Retired rule: {} (history retained){cleanup}",
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
        let before = rule.clone();

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

        if let Some(sources) = req.source_ids {
            rule.source_ids = sources
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect();
            changes.push("source_ids");
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

        let attribution_changed = req.stakeholder.is_some()
            || req.stakeholder_quote.is_some()
            || req.stakeholder_scope.is_some()
            || req.review_after.is_some();
        let clear_stakeholder = req.stakeholder.as_deref() == Some("");
        if clear_stakeholder {
            if req.stakeholder_quote.is_some() || req.stakeholder_scope.is_some() {
                return Err(McpError { code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from("To clear attribution, omit stakeholder_quote and stakeholder_scope"), data: None });
            }
            rule.stakeholder = None;
            let old_len = rule.tags.len();
            rule.tags.retain(|tag| !tag.starts_with("stakeholder:"));
            if rule.tags.len() != old_len { changes.push("tags"); }
        }
        apply_stakeholder_attribution(
            &mut rule,
            if clear_stakeholder { None } else { req.stakeholder },
            req.stakeholder_quote,
            req.stakeholder_scope,
            req.review_after,
        )?;
        if attribution_changed {
            changes.push("stakeholder provenance");
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

        let update = if clear_stakeholder {
            rule_store.update_clearing_stakeholder(&rule, req.changed_by.as_deref(), req.change_note.as_deref())
        } else {
            rule_store.update_with_metadata(&rule, req.changed_by.as_deref(), req.change_note.as_deref())
        };
        update
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to update: {e}")),
                data: None,
            })?;

        // Re-sync if proven, or if the rule is or was an operator hard rule.
        if rule.status == RuleStatus::Proven || was_hard_rule || rule.is_operator_hard_rule() {
            let _ = self.sync_rules();
        }

        let suffix = self.encode_chore_suffix(&rule)?;
        let cleanup = self.rule_cleanup_suffix(&rule, &crate::rule_refs::update_changes(&before, &rule));
        Ok(Self::success(format!(
            "Updated rule {}: {}{}{cleanup}",
            req.id,
            changes.join(", "),
            suffix
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
        let suffix = self.encode_chore_suffix(&restored)?;
        Ok(Self::success(format!(
            "Restored rule {}{} (status: {}){suffix}",
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

    /// cas-f0a1: a verifier never drafts a rule from one verdict. Its lesson
    /// joins a rule candidate, which no agent context surfaces; once seen in
    /// `CANDIDATE_DECISION_TASKS` different tasks, a decision chore asks the
    /// supervisor and operator whether it becomes a rule.
    pub async fn cas_rule_observe(
        &self,
        candidate_id: Option<String>,
        content: String,
        category: String,
        verdict_link: String,
    ) -> Result<CallToolResult, McpError> {
        let invalid = |message: String| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(message),
            data: None,
        };
        let (task_id, verdict_id) = verdict_link.split_once(':').ok_or_else(|| {
            invalid(format!(
                "source_ids must link the verdict as <task-id>:<verification-id>, got {verdict_link:?}"
            ))
        })?;
        let candidate = cas_store::observe_rule_candidate(
            &self.cas_root,
            candidate_id.as_deref(),
            &content,
            &category,
            task_id,
            verdict_id,
        )
        .map_err(|error| {
            let mut mapped = invalid(format!("Failed to record the observation: {error}"));
            if matches!(&error, cas_store::StoreError::Database(source)
                if cas_store::shared_db::is_busy_error(source))
            {
                mapped.code = ErrorCode::INTERNAL_ERROR;
            }
            mapped
        })?;
        let tasks = candidate.distinct_tasks();
        let mut output = format!(
            "Observed rule candidate {} ({tasks}/{CANDIDATE_DECISION_TASKS} tasks). It is not a rule and is never surfaced to agents.",
            candidate.id
        );
        if tasks >= CANDIDATE_DECISION_TASKS {
            let chore = self.ensure_candidate_decision_chore(&candidate)?;
            output.push_str(&format!(
                " Raised to the supervisor and operator as decision chore {chore}."
            ));
        }
        Ok(Self::success(output))
    }

    /// Every rule candidate with its verdict links, for the supervisor's
    /// promote decision.
    pub async fn cas_rule_candidates(&self) -> Result<CallToolResult, McpError> {
        let candidates = cas_store::list_rule_candidates(&self.cas_root).map_err(|error| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list rule candidates: {error}")),
            data: None,
        })?;
        if candidates.is_empty() {
            return Ok(Self::success("No rule candidates."));
        }
        let mut output = format!("Rule candidates ({}):\n", candidates.len());
        for candidate in &candidates {
            let links: Vec<_> = candidate
                .occurrences
                .iter()
                .map(|occurrence| format!("{}:{}", occurrence.task_id, occurrence.verdict_id))
                .collect();
            output.push_str(&format!(
                "\n- {} [{}] {} task(s): {}\n  verdicts: {}\n",
                candidate.id,
                if candidate.category.is_empty() { "uncategorised" } else { &candidate.category },
                candidate.distinct_tasks(),
                candidate.content,
                links.join(", ")
            ));
        }
        Ok(Self::success(output))
    }

    /// One durable decision chore per candidate, found again by its external
    /// reference even after it closes, so later observations never re-raise it.
    fn ensure_candidate_decision_chore(
        &self,
        candidate: &cas_store::RuleCandidate,
    ) -> Result<String, McpError> {
        let internal = |message: String| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(message),
            data: None,
        };
        let task_store = self.open_task_store()?;
        let external_ref = format!("rule-candidate:{}", candidate.id);
        if let Some(existing) = task_store
            .list(None)
            .map_err(|error| internal(format!("Failed to list rule chores: {error}")))?
            .into_iter()
            .find(|task| task.external_ref.as_deref() == Some(external_ref.as_str()))
        {
            return Ok(existing.id);
        }
        let id = task_store
            .generate_id()
            .map_err(|error| internal(format!("Failed to generate a chore ID: {error}")))?;
        let mut chore = Task::new(id.clone(), format!("decide on rule candidate {}", candidate.id));
        chore.task_type = TaskType::Chore;
        chore.origin_project = task_store.project_id().map(str::to_owned);
        chore.external_ref = Some(external_ref);
        chore.description = format!(
            "Verifiers observed rule candidate {} in {} different tasks. The supervisor and operator decide whether it becomes a project rule; read it and its verdict links with `rule action=candidates`. To adopt it, create the rule with those links as source_ids and promote it. Otherwise close this chore.",
            candidate.id,
            candidate.distinct_tasks()
        );
        chore.acceptance_criteria =
            "The operator or supervisor recorded a promote or decline decision for the candidate.".into();
        task_store
            .create_atomic(&chore, &[], None, Some("rule-candidate"))
            .map_err(|error| internal(format!("Failed to create the decision chore: {error}")))?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::init_cas_dir;

    async fn promote_with_sources(tags: &[&str], sources: &[&str], repeat: bool) -> Vec<Task> {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().expect("temporary project");
        let cas_root = init_cas_dir(temp.path()).expect("initialize isolated project");
        let core = CasCore::with_daemon(cas_root, None, None);
        register_rule_author(&core, crate::types::AgentRole::Standard, false);
        let rule_store = core.open_rule_store().expect("rule store");
        rule_store.init().expect("initialize rule store");
        let task_store = core.open_task_store().expect("task store");
        task_store.init().expect("initialize task store");
        let mut rule = Rule::new(
            "cas-encode-example".to_string(),
            "Every change to the parser must pass its grammar check".to_string(),
        );
        rule.tags = tags.iter().map(|tag| (*tag).to_string()).collect();
        rule.source_ids = sources.iter().map(|id| (*id).to_string()).collect();
        rule_store.add(&rule).expect("add draft rule");

        core.cas_rule_promote(rule.id.clone(), Some("reviewed evidence".into()), None)
            .await
            .expect("promote rule");
        if repeat {
            core.cas_rule_promote(rule.id.clone(), Some("repeat review".into()), None)
                .await
                .expect("repeat promotion");
        }
        assert!(
            temp.path()
                .join(".claude/rules/cas/cas-encode-example.md")
                .exists(),
            "promoted rule must sync inside the isolated project"
        );
        task_store.list(None).expect("list tasks")
    }

    #[tokio::test]
    async fn tagged_rule_with_two_sources_files_one_encode_chore() {
        let tasks = promote_with_sources(
            &["from_learning", "enforceable:lint"],
            &["mem-one", "mem-two"],
            false,
        )
        .await;
        assert_eq!(tasks.len(), 1);
        let chore = &tasks[0];
        assert_eq!(chore.task_type, TaskType::Chore);
        assert_eq!(chore.title, "encode cas-encode-example as lint");
        assert_eq!(
            chore.external_ref.as_deref(),
            Some("rule-encode:cas-encode-example")
        );
        assert!(chore.description.contains("cas-encode-example"));
        assert!(chore.description.contains("mem-one, mem-two"));
    }

    #[tokio::test]
    async fn untagged_rule_files_no_encode_chore() {
        let tasks = promote_with_sources(&["from_learning"], &["mem-one", "mem-two"], false).await;
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn mechanical_rule_with_one_source_files_encode_chore() {
        let tasks = promote_with_sources(&["enforceable:gate"], &["mem-one"], false).await;
        assert_eq!(tasks.len(), 1);
    }

    #[tokio::test]
    async fn repeat_promotion_does_not_duplicate_encode_chore() {
        let tasks =
            promote_with_sources(&["enforceable:type"], &["mem-one", "mem-two"], true).await;
        assert_eq!(tasks.len(), 1);
    }
    fn isolated_core() -> (tempfile::TempDir, CasCore) {
        let temp = tempfile::tempdir().expect("temporary project");
        let cas_root = init_cas_dir(temp.path()).expect("isolated project");
        let core = CasCore::with_daemon(cas_root, None, None);
        core.open_rule_store().unwrap().init().unwrap();
        core.open_task_store().unwrap().init().unwrap();
        (temp, core)
    }

    #[tokio::test]
    async fn rule_list_labels_store_repo_and_held_sources_cas_83c3() {
        use crate::store::RuleStore as _;
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        let raw = crate::store::SqliteRuleStore::open(&root).unwrap();
        raw.init().unwrap();
        let mut rule = Rule::new("rule-source".into(), "Stored instruction".into());
        rule.status = RuleStatus::Proven;
        rule.helpful_count = 1;
        let mut legacy = rule.clone();
        legacy.id = "rule-held".into();
        // A legacy source must exist before projection receipts start.
        raw.add(&legacy).unwrap();
        let core = CasCore::with_daemon(root, None, None);
        core.open_rule_store().unwrap().add(&rule).unwrap();
        std::fs::write(temp.path().join(".claude/rules/team.md"), "Repository instruction").unwrap();
        let result = core.cas_rules_list().await.unwrap();
        let text = result.content.iter().filter_map(|c| c.as_text().map(|t| t.text.as_str())).collect::<Vec<_>>().join("\n");
        for expected in ["source: store", "source: repo", "team.md", "rule-held", "held: file absent"] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert_eq!(text.matches("rule-source").count(), 1, "generated projection is not a second rule");
    }

    fn update_rule(id: &str, tags: Option<&str>, sources: Option<&str>) -> RuleUpdateRequest {
        RuleUpdateRequest {
            stakeholder: None,
            stakeholder_quote: None,
            stakeholder_scope: None,
            review_after: None,
            id: id.to_owned(),
            content: None,
            paths: None,
            tags: tags.map(str::to_owned),
            source_ids: sources.map(str::to_owned),
            auto_approve_tools: None,
            auto_approve_paths: None,
            changed_by: None,
            change_note: Some("review incident evidence".into()),
        }
    }

    fn register_rule_author(core: &CasCore, role: crate::types::AgentRole, factory: bool) {
        let mut agent =
            crate::types::Agent::new_with_role("rule-author".into(), "rule-author".into(), role);
        agent.factory_session = factory.then(|| "factory-session".into());
        let store = core.open_agent_store().unwrap();
        store.init().unwrap();
        store.register(&agent).unwrap();
        core.set_agent_id_for_testing(agent.id);
    }

    /// cas-70a6: every encode-producing write uses the current caller's
    /// authority, including edits to a previously operator-authorised rule.
    #[tokio::test]
    async fn encode_chores_require_operator_authority_for_every_write() {
        use crate::types::AgentRole::{Director, Standard, Supervisor, Worker};
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        for (identity, role, factory, factory_env, allowed) in [
            ("worker", Some(Worker), true, false, false),
            ("supervisor", Some(Supervisor), true, false, false),
            ("director", Some(Director), false, false, false),
            ("factory agent", Some(Standard), true, false, false),
            ("factory environment", Some(Standard), false, true, false),
            ("unregistered", None, false, false, false),
            ("operator", Some(Standard), false, false, true),
        ] {
            if factory_env {
                env.set("CAS_FACTORY_MODE", "1");
            } else {
                env.remove("CAS_FACTORY_MODE");
            }
            for action in [
                "create",
                "update",
                "promote",
                "already proven",
                "helpful",
                "restore",
            ] {
                let (_temp, core) = isolated_core();
                if let Some(role) = role {
                    register_rule_author(&core, role, factory);
                }
                let rules = core.open_rule_store().unwrap();
                let mut rule = Rule::new("rule-mechanical".into(), "Check parser grammar".into());
                rule.tags = vec!["enforceable:test".into()];
                rule.source_ids = vec!["incident-one".into()];
                // Stored authority never delegates permission to the next writer.
                rule.authorize_operator_hard_rule("operator:previous-author");
                if action == "already proven" {
                    rule.status = RuleStatus::Proven;
                }
                if action != "create" {
                    rules.add(&rule).unwrap();
                }
                let result = match action {
                    "create" => {
                        core.cas_rule_create(Parameters(RuleCreateRequest {
                            stakeholder: None,
                            stakeholder_quote: None,
                            stakeholder_scope: None,
                            review_after: None,
                            scope: "project".into(),
                            content: rule.content.clone(),
                            paths: None,
                            tags: Some("enforceable:test".into()),
                            source_ids: Some("incident-one".into()),
                            auto_approve_tools: None,
                            auto_approve_paths: None,
                        }))
                        .await
                    }
                    "update" => {
                        core.cas_rule_update(Parameters(update_rule(
                            &rule.id,
                            Some("enforceable:test"),
                            None,
                        )))
                        .await
                    }
                    "promote" | "already proven" => {
                        core.cas_rule_promote(
                            rule.id.clone(),
                            Some("reviewed mechanism".into()),
                            None,
                        )
                        .await
                    }
                    "helpful" => {
                        core.cas_rule_helpful(Parameters(IdRequest {
                            id: rule.id.clone(),
                        }))
                        .await
                    }
                    "restore" => {
                        core.cas_rule_restore(Parameters(VersionRequest {
                            id: rule.id.clone(),
                            version: Some(1),
                            version_id: None,
                            changed_by: None,
                            change_note: Some("restore mechanism".into()),
                        }))
                        .await
                    }
                    _ => unreachable!(),
                }
                .unwrap();
                let text = result
                    .content
                    .iter()
                    .filter_map(|content| content.as_text())
                    .map(|text| text.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let tasks = core.open_task_store().unwrap().list(None).unwrap();
                assert_eq!(
                    tasks.len(),
                    usize::from(allowed),
                    "{identity}/{action}: {text}"
                );
                if allowed {
                    assert!(text.contains("encode chore"), "{identity}/{action}: {text}");
                } else {
                    assert!(
                        text.contains("no encode chore created"),
                        "{identity}/{action}: {text}"
                    );
                    assert!(
                        text.contains("rule action=update"),
                        "operator command missing: {text}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn tagging_legacy_draft_files_once_even_after_chore_closes() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let (_temp, core) = isolated_core();
        register_rule_author(&core, crate::types::AgentRole::Standard, false);
        let mut rule = Rule::new("legacy-rule".into(), "Check parser grammar".into());
        core.open_rule_store().unwrap().add(&rule).unwrap();
        core.cas_rule_update(Parameters(update_rule(
            &rule.id,
            Some("enforceable:test"),
            None,
        )))
        .await
        .unwrap();
        let store = core.open_task_store().unwrap();
        let mut tasks = store.list(None).unwrap();
        assert_eq!(
            tasks.len(),
            1,
            "the legacy rule record is the first occurrence"
        );
        let mut chore = tasks.remove(0);
        chore.status = TaskStatus::Closed;
        store.update(&chore).unwrap();
        core.cas_rule_update(Parameters(update_rule(
            &rule.id,
            Some("enforceable:test"),
            Some("incident-one"),
        )))
        .await
        .unwrap();
        core.cas_rule_promote(
            rule.id.clone(),
            Some("specific mechanical constraint".into()),
            None,
        )
        .await
        .unwrap();
        assert_eq!(store.list(None).unwrap().len(), 1);
        rule = core.open_rule_store().unwrap().get(&rule.id).unwrap();
        assert_eq!(rule.source_ids, vec!["incident-one"]);
        assert_eq!(rule.status, RuleStatus::Proven);
        core.cas_rule_delete(Parameters(IdRequest {
            id: rule.id.clone(),
        }))
        .await
        .unwrap();
        assert_eq!(
            core.open_rule_store()
                .unwrap()
                .get(&rule.id)
                .unwrap()
                .status,
            RuleStatus::Retired
        );
        core.cas_rule_restore(Parameters(VersionRequest {
            id: rule.id.clone(),
            version: None,
            version_id: None,
            changed_by: None,
            change_note: Some("restore verified constraint".into()),
        }))
        .await
        .unwrap();
        assert_eq!(
            core.open_rule_store()
                .unwrap()
                .get(&rule.id)
                .unwrap()
                .status,
            RuleStatus::Proven
        );
        assert_eq!(
            store.list(None).unwrap().len(),
            1,
            "restore reuses even a closed chore"
        );
    }

    #[tokio::test]
    async fn judgement_rule_waits_for_two_distinct_sources() {
        let (_temp, core) = isolated_core();
        let mut rule = Rule::new(
            "judgement-rule".into(),
            "Prefer a compact review explanation".into(),
        );
        rule.source_ids = vec!["incident-one".into(), " incident-one ".into(), " ".into()];
        let rules = core.open_rule_store().unwrap();
        rules.add(&rule).unwrap();
        let error = core
            .cas_rule_promote(rule.id.clone(), Some("one incident".into()), None)
            .await
            .expect_err("duplicate and blank IDs are not independent evidence");
        assert!(error.message.contains("two distinct source IDs"));
        assert_eq!(rules.get(&rule.id).unwrap().status, RuleStatus::Draft);
        assert!(
            core.open_task_store()
                .unwrap()
                .list(None)
                .unwrap()
                .is_empty()
        );
        core.cas_rule_update(Parameters(update_rule(
            &rule.id,
            None,
            Some("incident-one,incident-two"),
        )))
        .await
        .unwrap();
        core.cas_rule_promote(rule.id.clone(), Some("two observed incidents".into()), None)
            .await
            .unwrap();
        assert_eq!(rules.get(&rule.id).unwrap().status, RuleStatus::Proven);
        assert!(
            core.open_task_store()
                .unwrap()
                .list(None)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn retired_harmful_and_unknown_mechanisms_do_not_file() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let (_temp, core) = isolated_core();
        register_rule_author(&core, crate::types::AgentRole::Standard, false);
        let mut rule = Rule::new("example".into(), "Check parser grammar".into());
        rule.tags = vec!["enforceable:prose".into()];
        assert!(core.ensure_encode_chore(&rule).unwrap().is_none());
        rule.tags = vec!["enforceable:test".into()];
        rule.status = RuleStatus::Retired;
        assert!(core.ensure_encode_chore(&rule).unwrap().is_none());
        rule.status = RuleStatus::Draft;
        rule.harmful_count = 1;
        assert!(core.ensure_encode_chore(&rule).unwrap().is_none());
        rule.harmful_count = 0;
        rule.status = RuleStatus::Stale;
        assert!(core.ensure_encode_chore(&rule).unwrap().is_some());
    }

    /// A Git checkout whose code and test cite `id` under two recorded scopes.
    fn cited_rule(core: &CasCore, root: &std::path::Path, id: &str) {
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git").current_dir(root).args(args).status();
            assert!(status.unwrap().success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        for (path, scope, item) in [
            ("src/sms.rs", "Only SMS sends", "fn send_sms() {}"),
            ("tests/sms.rs", "All sending", "fn rejects_send() {}"),
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, format!("// cas-rule: {id} scope: {scope:?}\n{item}\n")).unwrap();
        }
        git(&["add", "src", "tests"]);
        let mut rule = Rule::new(id.into(), "Limit SMS sending".into());
        rule.stakeholder = Some(cas_types::StakeholderAttribution {
            name: "Ben".into(),
            quote: "Please limit sending".into(),
            scope: "All sending".into(),
        });
        core.open_rule_store().unwrap().add(&rule).unwrap();
    }

    /// The one cleanup task, its listed sites, and proof nothing was deleted.
    fn cleanup_task(root: &std::path::Path, core: &CasCore, id: &str) -> String {
        let tasks = core.open_task_store().unwrap().list(None).unwrap();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        let link = tasks[0].external_ref.as_deref().unwrap_or_default();
        assert!(link.starts_with(&format!("rule-cleanup:{id}:")), "{link}");
        for path in ["src/sms.rs", "tests/sms.rs"] {
            let text = std::fs::read_to_string(root.join(path)).unwrap();
            assert!(text.contains(&format!("cas-rule: {id} ")), "{path} was edited");
        }
        tasks[0].description.clone()
    }

    /// cas-56266: retiring, superseding or un-attributing a rule lists every
    /// cited site in one linked cleanup task.
    #[tokio::test]
    async fn whole_rule_changes_file_one_cleanup_task_listing_every_site_cas_56266() {
        for change in ["retire", "supersede", "clear"] {
            let (temp, core) = isolated_core();
            cited_rule(&core, temp.path(), "rule-901");
            let request = |tags: Option<&str>, stakeholder: Option<&str>| RuleUpdateRequest {
                stakeholder: stakeholder.map(str::to_owned),
                ..update_rule("rule-901", tags, None)
            };
            match change {
                "retire" => core.cas_rule_delete(Parameters(IdRequest { id: "rule-901".into() })).await,
                "supersede" => core.cas_rule_update(Parameters(request(Some("superseded,superseded-by:rule-902"), None))).await,
                _ => core.cas_rule_update(Parameters(request(None, Some("")))).await,
            }
            .unwrap();
            let listed = cleanup_task(temp.path(), &core, "rule-901");
            for site in [r#"src/sms.rs:1 scope: "Only SMS sends""#, r#"tests/sms.rs:1 scope: "All sending""#] {
                assert!(listed.contains(site), "{change}: {listed}");
            }
        }
    }

    /// cas-56266: narrowing the stakeholder scope lists only the markers whose
    /// recorded scope no longer matches it.
    #[tokio::test]
    async fn narrowing_a_rule_lists_sites_outside_its_new_scope_cas_56266() {
        let (temp, core) = isolated_core();
        cited_rule(&core, temp.path(), "rule-903");
        let narrow = RuleUpdateRequest {
            stakeholder_scope: Some("Only SMS sends".into()),
            ..update_rule("rule-903", None, None)
        };
        core.cas_rule_update(Parameters(narrow)).await.unwrap();
        let listed = cleanup_task(temp.path(), &core, "rule-903");
        assert!(listed.contains(r#"tests/sms.rs:1 scope: "All sending""#), "{listed}");
        assert!(!listed.contains("src/sms.rs"), "{listed}");
    }

    /// cas-56266: daemon maintenance files one cleanup task when a stakeholder
    /// rule's review_after passes, and repeated sweeps reuse it.
    #[test]
    fn expired_review_after_files_its_cleanup_task_once_cas_56266() {
        let (temp, core) = isolated_core();
        cited_rule(&core, temp.path(), "rule-904");
        let store = core.open_rule_store().unwrap();
        let mut rule = store.get("rule-904").unwrap();
        rule.review_after = Some(chrono::Utc::now() - chrono::Duration::days(1));
        store.update(&rule).unwrap();
        let config = crate::daemon::DaemonConfig {
            cas_root: core.cas_root.clone(),
            process_observations: false,
            consolidate_memories: false,
            auto_prune: false,
            apply_decay: false,
            update_entity_summaries: false,
            index_code: false,
            index_bm25: false,
            agent_purge_age_hours: 0,
            ..crate::daemon::DaemonConfig::default()
        };
        for _ in 0..2 {
            crate::daemon::run_maintenance(&config).unwrap();
        }
        let listed = cleanup_task(temp.path(), &core, "rule-904");
        assert!(listed.contains("src/sms.rs:1") && listed.contains("tests/sms.rs:1"), "{listed}");
    }

    #[test]
    fn pending_encode_chores_show_active_project_backlog_only() {
        let mut chore = Task::new("chore-one".into(), "encode rule-one as test".into());
        chore.task_type = TaskType::Chore;
        chore.external_ref = Some(encode_chore_ref("rule-one"));
        let mut closed = chore.clone();
        closed.id = "chore-closed".into();
        closed.status = TaskStatus::Closed;
        let mut unrelated = chore.clone();
        unrelated.id = "unrelated".into();
        unrelated.external_ref = None;
        let mut cancelled = closed.clone();
        cancelled.status = TaskStatus::Cancelled;
        let report = render_pending_encode_chores(&[chore.clone(), closed, unrelated, cancelled]);
        assert!(report.contains("project backlog: 1"));
        assert!(report.contains("chore-one [open] encode rule-one as test"));
        assert!(!report.contains("chore-closed"));
        assert!(!report.contains("unrelated"));
        for status in [
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            TaskStatus::AwaitingMerge,
        ] {
            chore.status = status;
            assert!(render_pending_encode_chores(&[chore.clone()]).contains("chore-one"));
        }
        assert!(render_pending_encode_chores(&[]).is_empty());
    }

    /// cas-f0a1: a second similar observation joins the existing candidate
    /// instead of drafting a rule; two different tasks raise one decision
    /// chore; nothing reaches the rule store, so no agent context surfaces it.
    #[tokio::test]
    async fn a_repeat_observation_counts_against_its_candidate_and_never_becomes_a_rule() {
        let (_temp, core) = isolated_core();
        let lesson = "Every test-first task must add a new test file";
        for (link, content) in [("cas-7da6:ver-1", lesson), ("cas-dbea:ver-2", "A test-first task must add a new test file")] {
            core.cas_rule_observe(None, content.into(), "testing".into(), link.into())
                .await
                .unwrap();
        }

        let candidates = cas_store::list_rule_candidates(&core.cas_root).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].distinct_tasks(), 2);
        assert!(core.open_rule_store().unwrap().list().unwrap().is_empty());
        let decision_ref = format!("rule-candidate:{}", candidates[0].id);
        let chores: Vec<_> = core
            .open_task_store()
            .unwrap()
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|task| task.external_ref.as_deref() == Some(decision_ref.as_str()))
            .collect();
        assert_eq!(chores.len(), 1);
        assert!(!chores[0].description.contains(lesson));

        // A real pooled-store wait timeout is retryable, not a bad observation.
        let connection = cas_store::shared_db::shared_connection(&core.cas_root.join("cas.db"))
            .unwrap();
        let held = connection.lock().unwrap();
        let budget = cas_store::wait_budget::bound_waits_until(std::time::Instant::now());
        let error = core
            .cas_rule_observe(None, lesson.into(), "testing".into(), "cas-0ab2:ver-busy".into())
            .await
            .unwrap_err();
        drop(budget);
        drop(held);
        assert_eq!(error.code, ErrorCode::INTERNAL_ERROR, "{error}");
        assert!(error.message.contains("retry"), "{error}");
        let invalid = core
            .cas_rule_observe(None, lesson.into(), "testing".into(), "missing-verdict".into())
            .await
            .unwrap_err();
        assert_eq!(invalid.code, ErrorCode::INVALID_PARAMS);
    }
}
