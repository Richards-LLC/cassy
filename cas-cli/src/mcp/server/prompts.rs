use std::collections::HashMap;

use rmcp::ErrorData as McpError;
use rmcp::model::{ErrorCode, GetPromptResult, Prompt, PromptArgument, PromptMessage, Role};

use crate::mcp::server::CasCore;
use crate::types::{RuleStatus, TaskStatus};

impl CasCore {
    /// Build list of available prompts
    pub(crate) fn build_prompts() -> Vec<Prompt> {
        vec![
            Prompt::new(
                "session_start",
                Some("Get context for starting a new session"),
                Some(vec![
                    PromptArgument::new("limit")
                        .with_title("Limit")
                        .with_description(
                            "Maximum items per section (defaults to hooks.context_limit, 5)",
                        )
                        .with_required(false),
                ]),
            ),
            Prompt::new(
                "task_planning",
                Some("Plan a new task with dependencies"),
                Some(vec![
                    PromptArgument::new("title")
                        .with_title("Title")
                        .with_description("Task title")
                        .with_required(true),
                    PromptArgument::new("context")
                        .with_title("Context")
                        .with_description("Additional context for planning")
                        .with_required(false),
                ]),
            ),
            Prompt::new(
                "memory_review",
                Some("Review and consolidate recent memories"),
                Some(vec![
                    PromptArgument::new("days")
                        .with_title("Days")
                        .with_description("Number of days to review")
                        .with_required(false),
                ]),
            ),
            Prompt::new(
                "rule_extraction",
                Some("Extract rules from observations"),
                Some(vec![
                    PromptArgument::new("tag")
                        .with_title("Tag")
                        .with_description("Filter by tag")
                        .with_required(false),
                ]),
            ),
            Prompt::new(
                "task_summary",
                Some("Get summary of current task state"),
                None,
            ),
            Prompt::new(
                "search_context",
                Some("Search for relevant context"),
                Some(vec![
                    PromptArgument::new("query")
                        .with_title("Query")
                        .with_description("Search query")
                        .with_required(true),
                ]),
            ),
            Prompt::new("daily_standup", Some("Generate daily standup notes"), None),
            Prompt::new(
                "code_review_context",
                Some("Get context for code review"),
                Some(vec![
                    PromptArgument::new("file_patterns")
                        .with_title("File Patterns")
                        .with_description("Glob patterns for files")
                        .with_required(false),
                ]),
            ),
            Prompt::new(
                "session_end",
                Some("Generate session summary for handoff"),
                None,
            ),
        ]
    }

    /// Get prompt content
    pub(crate) fn get_prompt_content(
        &self,
        name: &str,
        args: &HashMap<String, String>,
    ) -> Result<GetPromptResult, McpError> {
        match name {
            "session_start" => {
                let limit = args
                    .get("limit")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| {
                        crate::config::Config::load(&self.cas_root)
                            .unwrap_or_default()
                            .context_limit()
                    });
                let context = crate::hooks::build_context(
                    &crate::hooks::HookInput {
                        model: None,
                        from_model: None,
                        to_model: None,
                        effort: None,
                        prompt_id: None,
                        session_id: "mcp".to_string(),
                        transcript_path: None,
                        cwd: self
                            .cas_root
                            .parent()
                            .unwrap_or(&self.cas_root)
                            .to_string_lossy()
                            .to_string(),
                        workspace_root: None,
                        permission_mode: None,
                        hook_event_name: "SessionStart".to_string(),
                        tool_name: None,
                        tool_input: None,
                        tool_response: None,
                        tool_use_id: None,
                        tool_input_truncated: None,
                        user_prompt: None,
                        machine_prompt_provenance: None,
                        source: None,
                        reason: None,
                        subagent_type: None,
                        agent_id: None,
                        agent_type: None,
                        subagent_prompt: None,
                        agent_role: std::env::var("CAS_AGENT_ROLE").ok(),
                        message: None,
                        message_is_final: None,
                        index: None,
                        stop_hook_active: None,
                    },
                    limit,
                    &self.cas_root,
                )
                .unwrap_or_else(|_| "No context available".to_string());

                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    Role::User,
                    format!("Here is the current context from Cassy:\n\n{context}"),
                )])
                .with_description("Session start context from Cassy"))
            }
            "task_planning" => {
                let title = args
                    .get("title")
                    .cloned()
                    .unwrap_or_else(|| "New Task".to_string());
                let context = args.get("context").cloned().unwrap_or_default();

                let mut prompt = format!("I need to plan a task: {title}\n\n");
                if !context.is_empty() {
                    prompt.push_str(&format!("Additional context: {context}\n\n"));
                }
                prompt.push_str("Please help me:\n1. Break down the task into subtasks\n2. Identify dependencies\n3. Estimate complexity\n4. Suggest priority level");

                Ok(
                    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt)])
                        .with_description("Task planning prompt"),
                )
            }
            "memory_review" => {
                let days = args.get("days").and_then(|s| s.parse().ok()).unwrap_or(7);
                let store = self.open_store()?;
                let entries = store.recent(50).unwrap_or_default();

                let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
                let recent: Vec<_> = entries.into_iter().filter(|e| e.created > cutoff).collect();

                let mut prompt = format!(
                    "Please review these {} memories from the last {} days:\n\n",
                    recent.len(),
                    days
                );
                for entry in recent {
                    prompt.push_str(&format!(
                        "- [{}] {:?}: {}\n",
                        entry.id,
                        entry.entry_type,
                        entry.preview(80)
                    ));
                }
                prompt.push_str("\nIdentify:\n1. Duplicate or redundant entries\n2. Entries that could be merged\n3. Patterns that should become rules");

                Ok(
                    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt)])
                        .with_description("Memory review prompt"),
                )
            }
            "rule_extraction" => {
                let tag = args.get("tag").cloned();
                let store = self.open_store()?;
                let entries = store.list().unwrap_or_default();

                let observations: Vec<_> = entries
                    .into_iter()
                    .filter(|e| e.entry_type == crate::types::EntryType::Observation)
                    .filter(|e| tag.as_ref().map(|t| e.tags.contains(t)).unwrap_or(true))
                    .take(20)
                    .collect();

                let mut prompt = format!(
                    "Extract rules from these {} observations:\n\n",
                    observations.len()
                );
                for obs in observations {
                    prompt.push_str(&format!("- {}\n", obs.content));
                }
                prompt.push_str("\nFor each potential rule:\n1. State the rule clearly\n2. Identify when it applies (file patterns)\n3. Rate confidence (0-1)");

                Ok(
                    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt)])
                        .with_description("Rule extraction prompt"),
                )
            }
            "task_summary" => {
                let task_store = self.open_task_store()?;
                let tasks = task_store.list(None).unwrap_or_default();

                let in_progress: Vec<_> = tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::InProgress)
                    .collect();
                let open: Vec<_> = tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Open)
                    .collect();
                let blocked = task_store.list_blocked().unwrap_or_default();

                let mut prompt = format!(
                    "Current task state:\n\n## In Progress ({})\n",
                    in_progress.len()
                );
                for task in in_progress {
                    prompt.push_str(&format!(
                        "- [{}] P{} {}\n",
                        task.id, task.priority.0, task.title
                    ));
                }
                prompt.push_str(&format!("\n## Blocked ({})\n", blocked.len()));
                for (task, blockers) in &blocked {
                    let blocker_ids: Vec<_> = blockers.iter().map(|t| t.id.as_str()).collect();
                    prompt.push_str(&format!(
                        "- [{}] {} (blocked by: {})\n",
                        task.id,
                        task.title,
                        blocker_ids.join(", ")
                    ));
                }
                prompt.push_str(&format!("\n## Ready ({})\n", open.len()));
                for task in open.iter().take(10) {
                    prompt.push_str(&format!(
                        "- [{}] P{} {}\n",
                        task.id, task.priority.0, task.title
                    ));
                }

                Ok(
                    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt)])
                        .with_description("Task summary"),
                )
            }
            "search_context" => {
                let query = args.get("query").cloned().unwrap_or_else(|| "".to_string());
                if query.is_empty() {
                    return Err(Self::error(ErrorCode::INVALID_PARAMS, "Query is required"));
                }

                let prompt = format!(
                    "Search Cassy for: {query}\n\nUse the cas_search tool to find relevant memories, tasks, rules, and skills."
                );

                Ok(
                    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt)])
                        .with_description("Search context prompt"),
                )
            }
            "daily_standup" => {
                let task_store = self.open_task_store()?;
                let store = self.open_store()?;

                let tasks = task_store.list(None).unwrap_or_default();
                let in_progress: Vec<_> = tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::InProgress)
                    .collect();

                let entries = store.recent(10).unwrap_or_default();
                let today = chrono::Utc::now().date_naive();
                let recent: Vec<_> = entries
                    .into_iter()
                    .filter(|e| e.created.date_naive() == today)
                    .collect();

                let mut prompt =
                    "Generate a daily standup update:\n\n## Currently Working On\n".to_string();
                for task in in_progress {
                    prompt.push_str(&format!("- {} ({})\n", task.title, task.id));
                    if !task.notes.is_empty() {
                        let last_note = task.notes.lines().last().unwrap_or("");
                        prompt.push_str(&format!("  Latest: {last_note}\n"));
                    }
                }
                prompt.push_str("\n## Today's Learnings\n");
                for entry in recent {
                    prompt.push_str(&format!("- {}\n", entry.preview(60)));
                }

                Ok(
                    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt)])
                        .with_description("Daily standup prompt"),
                )
            }
            "code_review_context" => {
                let patterns = args
                    .get("file_patterns")
                    .cloned()
                    .unwrap_or_else(|| "**/*.rs".to_string());
                let rule_store = self.open_rule_store()?;
                let rules = rule_store.list().unwrap_or_default();

                let matching: Vec<_> = rules
                    .iter()
                    .filter(|r| r.status == RuleStatus::Proven)
                    .filter(|r| {
                        r.paths.is_empty()
                            || r.paths.contains(&patterns)
                            || patterns.contains(&r.paths)
                    })
                    .collect();

                let mut prompt = format!(
                    "Code review context for files matching: {patterns}\n\n## Applicable Rules\n"
                );
                for rule in matching {
                    prompt.push_str(&format!("- [{}] {}\n", rule.id, rule.preview(80)));
                }
                prompt.push_str("\nApply these rules when reviewing the code.");

                Ok(
                    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt)])
                        .with_description("Code review context"),
                )
            }
            "session_end" => {
                let task_store = self.open_task_store()?;
                let store = self.open_store()?;

                let tasks = task_store.list(None).unwrap_or_default();
                let in_progress: Vec<_> = tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::InProgress)
                    .collect();

                let entries = store.recent(20).unwrap_or_default();

                let mut prompt =
                    "Generate a session handoff summary:\n\n## Tasks Worked On\n".to_string();
                for task in in_progress {
                    prompt.push_str(&format!("- {} ({})\n", task.title, task.id));
                }
                prompt.push_str("\n## Recent Activity\n");
                for entry in entries.iter().take(10) {
                    prompt.push_str(&format!(
                        "- {} {}\n",
                        entry.format_created("%H:%M"),
                        entry.preview(50)
                    ));
                }
                prompt.push_str("\nSummarize:\n1. What was accomplished\n2. What's in progress\n3. Next steps for the incoming session");

                Ok(
                    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt)])
                        .with_description("Session end summary prompt"),
                )
            }
            _ => Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Unknown prompt: {name}"),
            )),
        }
    }
}
