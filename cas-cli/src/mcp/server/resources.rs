use std::borrow::Cow;

use rmcp::ErrorData as McpError;
use rmcp::model::{AnnotateAble, ErrorCode, RawResource, Resource};

use crate::mcp::server::CasCore;
use crate::types::{RuleStatus, SkillStatus};

impl CasCore {
    /// Build list of available resources
    pub(crate) fn build_resources(&self) -> Vec<Resource> {
        let mut resources = Vec::new();

        if let Ok(store) = self.open_store() {
            if let Ok(entries) = store.list() {
                for entry in entries.iter().take(100) {
                    resources.push(
                        RawResource {
                            uri: format!("cas://entry/{}", entry.id),
                            name: entry.id.clone(),
                            title: entry.title.clone(),
                            description: Some(entry.preview(80)),
                            mime_type: Some("text/plain".to_string()),
                            size: Some(entry.content.len() as u32),
                            icons: None,
                            meta: None,
                        }
                        .no_annotation(),
                    );
                }
            }
        }

        if let Ok(store) = self.open_task_store() {
            if let Ok(tasks) = store.list(None) {
                for task in tasks.iter().filter(|t| !t.is_terminal()).take(50) {
                    resources.push(
                        RawResource {
                            uri: format!("cas://task/{}", task.id),
                            name: task.id.clone(),
                            title: Some(task.title.clone()),
                            description: Some(format!(
                                "P{} {:?} - {}",
                                task.priority.0, task.status, task.task_type
                            )),
                            mime_type: Some("text/plain".to_string()),
                            size: None,
                            icons: None,
                            meta: None,
                        }
                        .no_annotation(),
                    );
                }
            }
        }

        if let Ok(store) = self.open_rule_store() {
            if let Ok(rules) = store.list() {
                for rule in rules.iter().filter(|r| r.status == RuleStatus::Proven) {
                    resources.push(
                        RawResource {
                            uri: format!("cas://rule/{}", rule.id),
                            name: rule.id.clone(),
                            title: None,
                            description: Some(rule.preview(80)),
                            mime_type: Some("text/plain".to_string()),
                            size: Some(rule.content.len() as u32),
                            icons: None,
                            meta: None,
                        }
                        .no_annotation(),
                    );
                }
            }
        }

        if let Ok(store) = self.open_skill_store() {
            if let Ok(skills) = store.list(None) {
                for skill in skills.iter().filter(|s| s.status == SkillStatus::Enabled) {
                    resources.push(
                        RawResource {
                            uri: format!("cas://skill/{}", skill.id),
                            name: skill.id.clone(),
                            title: Some(skill.name.clone()),
                            description: Some(skill.description.chars().take(80).collect()),
                            mime_type: Some("text/plain".to_string()),
                            size: None,
                            icons: None,
                            meta: None,
                        }
                        .no_annotation(),
                    );
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
                    entry.created.format("%Y-%m-%d %H:%M"),
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
