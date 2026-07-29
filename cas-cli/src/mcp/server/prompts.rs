use std::collections::HashMap;

use rmcp::ErrorData as McpError;
use rmcp::model::{
    ErrorCode, GetPromptResult, Prompt, PromptArgument, PromptMessage, PromptMessageRole,
};

use crate::mcp::server::CasCore;
use crate::types::{RuleStatus, TaskStatus};

impl CasCore {
    /// Build list of available prompts
    pub(crate) fn build_prompts() -> Vec<Prompt> {
        vec![
            Prompt::new(
                "session_start",
                Some("Get context for starting a new session"),
                Some(vec![PromptArgument {
                    name: "limit".to_string(),
                    title: Some("Limit".to_string()),
                    description: Some("Maximum items per section".to_string()),
                    required: Some(false),
                }]),
            ),
            Prompt::new(
                "task_planning",
                Some("Plan a new task with dependencies"),
                Some(vec![
                    PromptArgument {
                        name: "title".to_string(),
                        title: Some("Title".to_string()),
                        description: Some("Task title".to_string()),
                        required: Some(true),
                    },
                    PromptArgument {
                        name: "context".to_string(),
                        title: Some("Context".to_string()),
                        description: Some("Additional context for planning".to_string()),
                        required: Some(false),
                    },
                ]),
            ),
            Prompt::new(
                "memory_review",
                Some("Review and consolidate recent memories"),
                Some(vec![PromptArgument {
                    name: "days".to_string(),
                    title: Some("Days".to_string()),
                    description: Some("Number of days to review".to_string()),
                    required: Some(false),
                }]),
            ),
            Prompt::new(
                "rule_extraction",
                Some("Extract rules from observations"),
                Some(vec![PromptArgument {
                    name: "tag".to_string(),
                    title: Some("Tag".to_string()),
                    description: Some("Filter by tag".to_string()),
                    required: Some(false),
                }]),
            ),
            Prompt::new(
                "task_summary",
                Some("Get summary of current task state"),
                None,
            ),
            Prompt::new(
                "search_context",
                Some("Search for relevant context"),
                Some(vec![PromptArgument {
                    name: "query".to_string(),
                    title: Some("Query".to_string()),
                    description: Some("Search query".to_string()),
                    required: Some(true),
                }]),
            ),
            Prompt::new("daily_standup", Some("Generate daily standup notes"), None),
            Prompt::new(
                "code_review_context",
                Some("Get context for code review"),
                Some(vec![PromptArgument {
                    name: "file_patterns".to_string(),
                    title: Some("File Patterns".to_string()),
                    description: Some("Glob patterns for files".to_string()),
                    required: Some(false),
                }]),
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
                let limit = args.get("limit").and_then(|s| s.parse().ok()).unwrap_or(5);
                let context = crate::hooks::build_context(
                    &crate::hooks::HookInput {
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
                        source: None,
                        reason: None,
                        subagent_type: None,
                        subagent_prompt: None,
                        agent_role: std::env::var("CAS_AGENT_ROLE").ok(),
                        message: None,
                    },
                    limit,
                    &self.cas_root,
                )
                .unwrap_or_else(|_| "No context available".to_string());

                Ok(GetPromptResult {
                    description: Some("Session start context from CAS".to_string()),
                    messages: vec![PromptMessage::new_text(
                        PromptMessageRole::User,
                        format!("Here is the current context from CAS:\n\n{context}"),
                    )],
                })
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

                Ok(GetPromptResult {
                    description: Some("Task planning prompt".to_string()),
                    messages: vec![PromptMessage::new_text(PromptMessageRole::User, prompt)],
                })
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

                Ok(GetPromptResult {
                    description: Some("Memory review prompt".to_string()),
                    messages: vec![PromptMessage::new_text(PromptMessageRole::User, prompt)],
                })
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

                Ok(GetPromptResult {
                    description: Some("Rule extraction prompt".to_string()),
                    messages: vec![PromptMessage::new_text(PromptMessageRole::User, prompt)],
                })
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

                Ok(GetPromptResult {
                    description: Some("Task summary".to_string()),
                    messages: vec![PromptMessage::new_text(PromptMessageRole::User, prompt)],
                })
            }
            "search_context" => {
                let query = args.get("query").cloned().unwrap_or_else(|| "".to_string());
                if query.is_empty() {
                    return Err(Self::error(ErrorCode::INVALID_PARAMS, "Query is required"));
                }

                let prompt = format!(
                    "Search CAS for: {query}\n\nUse the cas_search tool to find relevant memories, tasks, rules, and skills."
                );

                Ok(GetPromptResult {
                    description: Some("Search context prompt".to_string()),
                    messages: vec![PromptMessage::new_text(PromptMessageRole::User, prompt)],
                })
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

                Ok(GetPromptResult {
                    description: Some("Daily standup prompt".to_string()),
                    messages: vec![PromptMessage::new_text(PromptMessageRole::User, prompt)],
                })
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

                Ok(GetPromptResult {
                    description: Some("Code review context".to_string()),
                    messages: vec![PromptMessage::new_text(PromptMessageRole::User, prompt)],
                })
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
                        entry.created.format("%H:%M"),
                        entry.preview(50)
                    ));
                }
                prompt.push_str("\nSummarize:\n1. What was accomplished\n2. What's in progress\n3. Next steps for the incoming session");

                Ok(GetPromptResult {
                    description: Some("Session end summary prompt".to_string()),
                    messages: vec![PromptMessage::new_text(PromptMessageRole::User, prompt)],
                })
            }
            _ => Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Unknown prompt: {name}"),
            )),
        }
    }
}
