use crate::mcp::tools::core::imports::*;

impl CasCore {
    // ========================================================================
    // Skill Tools (10)
    // ========================================================================

    /// List enabled skills
    pub async fn cas_skill_list(&self) -> Result<CallToolResult, McpError> {
        let skill_store = self.open_skill_store()?;

        let skills = skill_store.list_enabled().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list: {e}")),
            data: None,
        })?;

        if skills.is_empty() {
            return Ok(Self::success("No enabled skills"));
        }

        let mut output = format!("Enabled skills ({}):\n\n", skills.len());
        for skill in skills {
            let summary = if skill.summary.is_empty() {
                skill.description.chars().take(50).collect::<String>()
            } else {
                skill.summary.clone()
            };
            output.push_str(&format!(
                "- [{}] {:?} {} - {}\n",
                skill.id, skill.skill_type, skill.name, summary
            ));
        }

        Ok(Self::success(output))
    }

    /// Show skill details
    /// Checks database first, then falls back to .claude/skills/ files
    pub async fn cas_skill_show(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        use crate::sync::skills::read_skill_from_file;

        let skill_store = self.open_skill_store()?;

        // Try database first
        let (skill, served_from_project_file) = match skill_store.get(&req.id) {
            Ok(s) => (s, false),
            Err(_) => {
                // Try to find in .claude/skills/ by name
                let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
                match read_skill_from_file(project_root, &req.id) {
                    Ok(Some(s)) => (s, true),
                    Ok(None) => {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!("Skill not found: {}", req.id)),
                            data: None,
                        });
                    }
                    Err(e) => {
                        return Err(McpError {
                            code: ErrorCode::INTERNAL_ERROR,
                            message: Cow::from(format!("Error reading skill: {e}")),
                            data: None,
                        });
                    }
                }
            }
        };

        let source = if served_from_project_file {
            "file (.claude/skills/)"
        } else {
            "database"
        };
        let output = format!(
            "Skill: {} ({})\n{}\n\nSource: {}\nType: {:?}\nStatus: {:?}\nUsage count: {}\nTags: {}\nSource entries: {}\nPreconditions: {}\nPostconditions: {}\nValidation script: {}\nCreated: {}\n\nDescription:\n{}\n\nInvocation:\n{}",
            skill.name,
            skill.id,
            "=".repeat(skill.name.len() + skill.id.len() + 4),
            source,
            skill.skill_type,
            skill.status,
            skill.usage_count,
            if skill.tags.is_empty() {
                "none".to_string()
            } else {
                skill.tags.join(", ")
            },
            if skill.source_ids.is_empty() {
                "none".to_string()
            } else {
                skill.source_ids.join(", ")
            },
            if skill.preconditions.is_empty() {
                "none".to_string()
            } else {
                skill.preconditions.join(", ")
            },
            if skill.postconditions.is_empty() {
                "none".to_string()
            } else {
                skill.postconditions.join(", ")
            },
            if skill.validation_script.is_empty() {
                "not configured".to_string()
            } else {
                skill.validation_script.clone()
            },
            skill.created_at.format("%Y-%m-%d %H:%M"),
            skill.description,
            skill.invocation
        );

        Ok(Self::success(output))
    }

    /// Create a new skill
    pub async fn cas_skill_create(
        &self,
        Parameters(req): Parameters<SkillCreateRequest>,
    ) -> Result<CallToolResult, McpError> {
        let skill_store = self.open_skill_store()?;

        let id = skill_store.generate_id().map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to generate ID: {e}")),
            data: None,
        })?;

        let skill_type = match req.skill_type.to_lowercase().as_str() {
            "mcp" => SkillType::Mcp,
            "plugin" => SkillType::Plugin,
            "internal" => SkillType::Internal,
            _ => SkillType::Command,
        };

        let tags: Vec<String> = req
            .tags
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let preconditions: Vec<String> = req
            .preconditions
            .map(|p| {
                p.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let postconditions: Vec<String> = req
            .postconditions
            .map(|p| {
                p.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let allowed_tools: Vec<String> = req
            .allowed_tools
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let disallowed_tools: Vec<String> = req
            .disallowed_tools
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let status = if req.draft {
            SkillStatus::Draft
        } else {
            SkillStatus::Enabled
        };

        let skill = Skill {
            id: id.clone(),
            scope: Scope::default(),
            name: req.name.clone(),
            description: req.description,
            skill_type,
            invocation: req.invocation,
            parameters_schema: String::new(),
            example: req.example.unwrap_or_default(),
            preconditions,
            postconditions,
            validation_script: req.validation_script.unwrap_or_default(),
            status,
            tags,
            summary: req.summary.unwrap_or_default(),
            invokable: req.invokable,
            argument_hint: req.argument_hint.unwrap_or_default(),
            context_mode: req.context_mode,
            agent_type: req.agent_type,
            allowed_tools,
            disallowed_tools,
            hooks: None,
            disable_model_invocation: req.disable_model_invocation,
            usage_count: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_used: None,
            team_id: None,
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
            share: None,
        };

        let validation = crate::skill_validation::validate_skill_with_policy(
            &skill,
            self.load_config().skill_validation().require_sandbox,
        )
        .map_err(|error| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Skill validation rejected create: {error}")),
            data: None,
        })?;

        skill_store.add(&skill).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to create skill: {e}")),
            data: None,
        })?;

        // Sync to Claude Code
        let _ = self.sync_skills();

        let warning = validation
            .warning
            .map(|warning| format!("\n{warning}"))
            .unwrap_or_default();
        Ok(Self::success(format!("Created skill: {id}{warning}")))
    }

    /// Enable a skill
    pub async fn cas_skill_enable(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let skill_store = self.open_skill_store()?;

        let mut skill = skill_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Skill not found: {e}")),
            data: None,
        })?;

        skill.status = SkillStatus::Enabled;
        skill.updated_at = chrono::Utc::now();

        skill_store.update(&skill).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        // Sync to Claude Code
        let _ = self.sync_skills();

        Ok(Self::success(format!(
            "Enabled skill: {} - synced to Claude Code",
            req.id
        )))
    }

    /// Disable a skill
    pub async fn cas_skill_disable(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let skill_store = self.open_skill_store()?;

        let mut skill = skill_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Skill not found: {e}")),
            data: None,
        })?;

        skill.status = SkillStatus::Disabled;
        skill.updated_at = chrono::Utc::now();

        skill_store.update(&skill).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        // Sync to Claude Code (removes disabled skills)
        let _ = self.sync_skills();

        Ok(Self::success(format!("Disabled skill: {}", req.id)))
    }

    /// Record skill usage
    pub async fn cas_skill_use(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let start_time = std::time::Instant::now();

        // cas-4fef: ownership gate. `cas-code-review` is supervisor-owned by
        // default (cas-865b); a factory worker running the persona pipeline
        // pre-close both burns ~14 minutes and produces an envelope the close
        // path no longer wants. The prohibition used to live only in the skill
        // description's tail and was violated twice in one session, so it is
        // enforced here — before the usage is recorded — rather than asked for.
        if crate::code_review_dispatch::is_cas_code_review_skill(&req.id) {
            // cas-bcfb: shared resolver — the PreToolUse gate on the
            // harness-native `Skill`/`Workflow` paths answers "who owns review
            // here?" with this same function, so the two cannot drift.
            let supervisor_owned =
                crate::code_review_dispatch::supervisor_owned_at(Some(self.cas_root.as_path()));
            if let crate::code_review_dispatch::ReviewDispatchDecision::Refused { message } =
                crate::code_review_dispatch::review_dispatch_decision(
                    crate::code_review_dispatch::is_factory_worker_from_env(),
                    supervisor_owned,
                )
            {
                return Err(McpError {
                    code: ErrorCode::INVALID_REQUEST,
                    message: Cow::from(message),
                    data: None,
                });
            }
        }

        let skill_store = self.open_skill_store()?;

        let mut skill = skill_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Skill not found: {e}")),
            data: None,
        })?;

        skill.usage_count += 1;
        skill.updated_at = chrono::Utc::now();

        skill_store.update(&skill).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update: {e}")),
            data: None,
        })?;

        // Trace skill invocation
        if let Some(tracer) = crate::tracing::DevTracer::get() {
            let trace = crate::tracing::SkillInvocationTrace {
                skill_id: skill.id.clone(),
                skill_name: skill.name.clone(),
                context: format!("usage_count: {}", skill.usage_count),
                result_summary: Some("success".to_string()),
            };
            let _ = tracer.record_skill_invocation(
                &trace,
                start_time.elapsed().as_millis() as u64,
                true,
                None,
            );
        }

        Ok(Self::success(format!(
            "Recorded usage for skill {} (count: {})",
            req.id, skill.usage_count
        )))
    }

    /// Sync skills to Claude Code
    pub async fn cas_skill_sync(&self) -> Result<CallToolResult, McpError> {
        let synced = self.sync_skills()?;
        Ok(Self::success(format!(
            "Synced {synced} skills to Claude Code"
        )))
    }

    // ========================================================================
    // Additional Skill Tools
    // ========================================================================

    /// Update a skill
    pub async fn cas_skill_update(
        &self,
        Parameters(req): Parameters<SkillUpdateRequest>,
    ) -> Result<CallToolResult, McpError> {
        use crate::sync::skills::{SkillSyncer, read_skill_from_file};

        let skill_store = self.open_skill_store()?;
        let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);

        // Try database first, then fall back to file-based skills (same as cas_skill_show)
        let (mut skill, is_file_skill) = match skill_store.get(&req.id) {
            Ok(s) => (s, false),
            Err(_) => {
                // Try to find in .claude/skills/ by name
                match read_skill_from_file(project_root, &req.id) {
                    Ok(Some(s)) => (s, true),
                    Ok(None) => {
                        return Err(McpError {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!("Skill not found: {}", req.id)),
                            data: None,
                        });
                    }
                    Err(e) => {
                        return Err(McpError {
                            code: ErrorCode::INTERNAL_ERROR,
                            message: Cow::from(format!("Error reading skill: {e}")),
                            data: None,
                        });
                    }
                }
            }
        };

        let mut changes = Vec::new();

        if let Some(name) = req.name {
            skill.name = name;
            changes.push("name");
        }

        if let Some(description) = req.description {
            skill.description = description;
            changes.push("description");
        }

        if let Some(invocation) = req.invocation {
            skill.invocation = invocation;
            changes.push("invocation");
        }

        if let Some(tags) = req.tags {
            skill.tags = tags
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            changes.push("tags");
        }

        if let Some(preconditions) = req.preconditions {
            skill.preconditions = preconditions
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            changes.push("preconditions");
        }

        if let Some(postconditions) = req.postconditions {
            skill.postconditions = postconditions
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            changes.push("postconditions");
        }

        if let Some(validation_script) = req.validation_script {
            skill.validation_script = validation_script;
            changes.push("validation_script");
        }

        if let Some(summary) = req.summary {
            skill.summary = summary;
            changes.push("summary");
        }

        if let Some(disable) = req.disable_model_invocation {
            skill.disable_model_invocation = disable;
            changes.push("disable_model_invocation");
        }

        if changes.is_empty() {
            return Ok(Self::success("No changes specified"));
        }

        skill.updated_at = chrono::Utc::now();

        let validation = crate::skill_validation::validate_skill_with_policy(
            &skill,
            self.load_config().skill_validation().require_sandbox,
        )
        .map_err(|error| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Skill validation rejected update: {error}")),
            data: None,
        })?;

        if is_file_skill {
            // File-based skill: write back to file
            let syncer = SkillSyncer::with_defaults(project_root);

            let synced = syncer.sync_skill(&skill).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to write skill file: {e}")),
                data: None,
            })?;

            if skill.status == SkillStatus::Enabled && !synced {
                return Err(McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Failed to sync enabled skill '{}' (conflict with builtin)",
                        skill.name
                    )),
                    data: None,
                });
            }
        } else {
            // Database skill: update in store
            skill_store
                .update_with_metadata(
                    &skill,
                    req.changed_by.as_deref(),
                    req.change_note.as_deref(),
                )
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to update: {e}")),
                    data: None,
                })?;

            // Re-sync if enabled
            if skill.status == SkillStatus::Enabled {
                let _ = self.sync_skills();
            }
        }

        let warning = validation
            .warning
            .map(|warning| format!("\n{warning}"))
            .unwrap_or_default();
        Ok(Self::success(format!(
            "Updated skill {}: {}{}",
            req.id,
            changes.join(", "),
            warning
        )))
    }

    /// List prior skill states, newest first.
    pub async fn cas_skill_history(
        &self,
        Parameters(req): Parameters<VersionRequest>,
    ) -> Result<CallToolResult, McpError> {
        let skill_store = self.open_skill_store()?;
        let versions = skill_store.list_versions(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list skill history: {e}")),
            data: None,
        })?;
        if versions.is_empty() {
            return Ok(Self::success(format!("No history for skill {}", req.id)));
        }

        let mut output = format!(
            "Skill history for {} ({} versions):\n\n",
            req.id,
            versions.len()
        );
        for version in versions {
            let preview: String = version.description.chars().take(120).collect();
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

    /// Restore a prior skill state, or un-retire to the newest prior state.
    pub async fn cas_skill_restore(
        &self,
        Parameters(req): Parameters<VersionRequest>,
    ) -> Result<CallToolResult, McpError> {
        let skill_store = self.open_skill_store()?;
        let version = req.version.or(req.version_id);
        skill_store
            .restore_version(
                &req.id,
                version,
                req.changed_by.as_deref(),
                req.change_note.as_deref(),
            )
            .map_err(|e| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!("Failed to restore skill: {e}")),
                data: None,
            })?;
        let restored = skill_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to read restored skill: {e}")),
            data: None,
        })?;
        let _ = self.sync_skills();
        Ok(Self::success(format!(
            "Restored skill {}{} (status: {})",
            req.id,
            version
                .map(|v| format!(" to version {v}"))
                .unwrap_or_default(),
            restored.status
        )))
    }

    /// Delete a skill
    pub async fn cas_skill_delete(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let skill_store = self.open_skill_store()?;

        skill_store.delete(&req.id).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to delete: {e}")),
            data: None,
        })?;

        // Re-sync to remove from Claude Code
        let _ = self.sync_skills();

        Ok(Self::success(format!(
            "Retired skill: {} (history retained)",
            req.id
        )))
    }

    /// List all skills (including disabled)
    /// Merges skills from database and .claude/skills/ directory
    pub async fn cas_skill_list_all(
        &self,
        Parameters(req): Parameters<LimitRequest>,
    ) -> Result<CallToolResult, McpError> {
        use crate::sync::skills::read_skills_from_files;
        use std::collections::HashSet;

        let skill_store = self.open_skill_store()?;

        // Get database skills
        let db_skills = skill_store.list(None).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to list: {e}")),
            data: None,
        })?;

        // Get file-based skills from .claude/skills/
        let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
        let file_skills = read_skills_from_files(project_root).unwrap_or_default();

        // Track database skill names to avoid duplicates
        let db_names: HashSet<String> = db_skills.iter().map(|s| s.name.to_lowercase()).collect();

        // Merge: database skills + file skills not in database
        let mut all_skills = db_skills;
        for file_skill in file_skills {
            let name_lower = file_skill.name.to_lowercase();
            // Skip if already in database (database takes precedence)
            if !db_names.contains(&name_lower) {
                all_skills.push(file_skill);
            }
        }

        if all_skills.is_empty() {
            return Ok(Self::success("No skills found"));
        }

        // Sort by name
        all_skills.sort_by(|a, b| a.name.cmp(&b.name));

        let limit = req.limit.unwrap_or(50);
        let mut output = format!(
            "All skills ({} total, showing {}):\n\n",
            all_skills.len(),
            all_skills.len().min(limit)
        );
        for skill in all_skills.iter().take(limit) {
            let source = if skill.id.starts_with("file-") {
                "file"
            } else {
                "db"
            };
            output.push_str(&format!(
                "- [{}] {:?} {:?} {} ({})\n",
                skill.id, skill.status, skill.skill_type, skill.name, source
            ));
        }

        if all_skills.len() > limit {
            output.push_str(&format!("\n... and {} more", all_skills.len() - limit));
        }

        Ok(Self::success(output))
    }
}
