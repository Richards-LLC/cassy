use std::borrow::Cow;

use rmcp::ErrorData as McpError;
use rmcp::model::{ErrorCode, Resource};

use crate::mcp::server::CasCore;
use crate::types::{RuleStatus, SkillStatus};

/// A plain-text `cas://` resource listing entry.
fn text_resource(
    uri: String,
    name: String,
    title: Option<String>,
    description: String,
    size: Option<usize>,
) -> Resource {
    let mut resource = Resource::new(uri, name)
        .with_description(description)
        .with_mime_type("text/plain");
    resource.title = title;
    resource.size = size.map(|size| size as u64);
    resource
}

impl CasCore {
    /// Build list of available resources
    pub(crate) fn build_resources(&self) -> Vec<Resource> {
        let mut resources = Vec::new();

        if let Ok(store) = self.open_store() {
            if let Ok(entries) = store.list() {
                for entry in entries.iter().take(100) {
                    resources.push(text_resource(
                        format!("cas://entry/{}", entry.id),
                        entry.id.clone(),
                        entry.title.clone(),
                        entry.preview(80),
                        Some(entry.content.len()),
                    ));
                }
            }
        }

        if let Ok(store) = self.open_task_store() {
            if let Ok(tasks) = store.list(None) {
                for task in tasks.iter().filter(|t| !t.is_terminal()).take(50) {
                    resources.push(text_resource(
                        format!("cas://task/{}", task.id),
                        task.id.clone(),
                        Some(task.title.clone()),
                        format!(
                            "P{} {:?} - {}",
                            task.priority.0, task.status, task.task_type
                        ),
                        None,
                    ));
                }
            }
        }

        if let Ok(store) = self.open_rule_store() {
            if let Ok(rules) = store.list() {
                for rule in rules.iter().filter(|r| r.status == RuleStatus::Proven) {
                    resources.push(text_resource(
                        format!("cas://rule/{}", rule.id),
                        rule.id.clone(),
                        None,
                        rule.preview(80),
                        Some(rule.content.len()),
                    ));
                }
            }
        }

        if let Ok(store) = self.open_skill_store() {
            if let Ok(skills) = store.list(None) {
                for skill in skills.iter().filter(|s| s.status == SkillStatus::Enabled) {
                    resources.push(text_resource(
                        format!("cas://skill/{}", skill.id),
                        skill.id.clone(),
                        Some(skill.name.clone()),
                        skill.description.chars().take(80).collect(),
                        None,
                    ));
                }
            }
        }

        resources
    }

    /// Read a specific resource
    pub(crate) fn read_resource_content(&self, uri: &str) -> Result<String, McpError> {
        let parts: Vec<&str> = uri
            .strip_prefix("cas://")
            .unwrap_or(uri)
            .split('/')
            .collect();
        if parts.len() < 2 {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Invalid resource URI",
            ));
        }

        let resource_type = parts[0];
        let id = parts[1];

        match resource_type {
            "entry" => {
                let store = self.open_store()?;
                let entry = store.get(id).map_err(|e| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Entry not found: {e}")),
                    data: None,
                })?;
                Ok(format!(
                    "ID: {}\nType: {:?}\nTags: {}\nCreated: {}\nImportance: {:.2}\n\n{}",
                    entry.id,
                    entry.entry_type,
                    if entry.tags.is_empty() {
                        "none".to_string()
                    } else {
                        entry.tags.join(", ")
                    },
                    entry.format_created("%Y-%m-%d %H:%M"),
                    entry.importance,
                    entry.content
                ))
            }
            "task" => {
                let store = self.open_task_store()?;
                let task = store.get(id).map_err(|e| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Task not found: {e}")),
                    data: None,
                })?;
                let mut content = format!(
                    "ID: {}\nTitle: {}\nStatus: {:?}\nPriority: P{}\nType: {}\n",
                    task.id, task.title, task.status, task.priority.0, task.task_type
                );
                if !task.description.is_empty() {
                    content.push_str(&format!("\nDescription:\n{}\n", task.description));
                }
                if !task.notes.is_empty() {
                    content.push_str(&format!("\nNotes:\n{}\n", task.notes));
                }
                Ok(content)
            }
            "rule" => {
                let store = self.open_rule_store()?;
                let rule = store.get(id).map_err(|e| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Rule not found: {e}")),
                    data: None,
                })?;
                Ok(format!(
                    "ID: {}\nStatus: {:?}\nPaths: {}\nFeedback: +{} -{}\n\n{}",
                    rule.id,
                    rule.status,
                    if rule.paths.is_empty() {
                        "all"
                    } else {
                        &rule.paths
                    },
                    rule.helpful_count,
                    rule.harmful_count,
                    rule.content
                ))
            }
            "skill" => {
                let store = self.open_skill_store()?;
                let skill = store.get(id).map_err(|e| McpError {
                    code: ErrorCode::INVALID_PARAMS,
                    message: Cow::from(format!("Skill not found: {e}")),
                    data: None,
                })?;
                Ok(format!(
                    "ID: {}\nName: {}\nType: {:?}\nStatus: {:?}\nUsage: {}\n\nDescription:\n{}\n\nInvocation:\n{}",
                    skill.id,
                    skill.name,
                    skill.skill_type,
                    skill.status,
                    skill.usage_count,
                    skill.description,
                    skill.invocation
                ))
            }
            _ => Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "Unknown resource type",
            )),
        }
    }
}
