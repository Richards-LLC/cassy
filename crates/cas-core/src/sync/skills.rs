//! Claude Code skill syncing
//!
//! Syncs enabled CAS skills to .claude/skills/ as Agent Skills for Claude Code.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::CoreError;
use cas_types::{Skill, SkillStatus};

/// Syncs CAS skills to Claude Code Agent Skills
pub struct SkillSyncer {
    target_dir: PathBuf,
}

/// Report of sync operation
#[derive(Debug, Default)]
pub struct SkillSyncReport {
    /// Number of skills synced
    pub synced: usize,
    /// Number of stale directories removed
    pub removed: usize,
    /// Skills that were synced
    pub synced_names: Vec<String>,
    /// Directories that were removed
    pub removed_names: Vec<String>,
}

impl SkillSyncer {
    /// Create a new syncer
    pub fn new(target_dir: PathBuf) -> Self {
        Self { target_dir }
    }

    /// Create a syncer with default settings
    pub fn with_defaults(project_root: &Path) -> Self {
        Self {
            target_dir: project_root.join(".claude/skills"),
        }
    }

    /// Check if a skill should be synced to Claude Code
    pub fn is_enabled(&self, skill: &Skill) -> bool {
        skill.status == SkillStatus::Enabled
    }

    /// Generate the SKILL.md content for a skill
    fn generate_skill_md(&self, skill: &Skill) -> String {
        let mut content = String::new();

        // YAML frontmatter
        content.push_str("---\n");
        content.push_str(&format!("name: cas-{}\n", sanitize_name(&skill.name)));
        // Use summary for frontmatter description (short trigger text)
        // Fall back to first line of description if summary is empty
        let frontmatter_desc = if !skill.summary.is_empty() {
            skill.summary.clone()
        } else {
            // Take first sentence or first 200 chars as fallback
            skill
                .description
                .lines()
                .next()
                .unwrap_or(&skill.description)
                .chars()
                .take(200)
                .collect::<String>()
        };
        content.push_str(&format!(
            "description: {}\n",
            escape_yaml(&frontmatter_desc)
        ));
        // Add user-invocable: false if skill should not appear in slash menu
        // (Claude Code now shows skills in slash menu by default)
        if !skill.invokable {
            content.push_str("user-invocable: false\n");
        } else if !skill.argument_hint.is_empty() {
            // Add argument-hint if skill is invokable and has a hint
            content.push_str(&format!(
                "argument-hint: {}\n",
                escape_yaml(&skill.argument_hint)
            ));
        }

        // Add context mode if set (e.g., "fork" for forked sub-agent context)
        if let Some(ref context_mode) = skill.context_mode {
            content.push_str(&format!("context: {}\n", escape_yaml(context_mode)));
        }

        // Add agent type if set
        if let Some(ref agent_type) = skill.agent_type {
            content.push_str(&format!("agent: {}\n", escape_yaml(agent_type)));
        }

        // Add allowed-tools as YAML list if set
        if !skill.allowed_tools.is_empty() {
            content.push_str("allowed-tools:\n");
            for tool in &skill.allowed_tools {
                content.push_str(&format!("  - {}\n", escape_yaml(tool)));
            }
        }

        // Add disallowed-tools as YAML list if set (Claude Code 2.1.152+, cas-5be8)
        if !skill.disallowed_tools.is_empty() {
            content.push_str("disallowed-tools:\n");
            for tool in &skill.disallowed_tools {
                content.push_str(&format!("  - {}\n", escape_yaml(tool)));
            }
        }

        // Add disable-model-invocation if set (Claude Code 2.1.3+)
        if skill.disable_model_invocation {
            content.push_str("disable-model-invocation: true\n");
        }

        // Add hooks if set (Claude Code 2.1.0+)
        if let Some(ref hooks) = skill.hooks {
            if !hooks.is_empty() {
                content.push_str("hooks:\n");
                // Generate PreToolUse hooks
                if let Some(ref pre_hooks) = hooks.pre_tool_use {
                    content.push_str("  PreToolUse:\n");
                    for hook_config in pre_hooks {
                        if let Some(ref matcher) = hook_config.matcher {
                            content.push_str(&format!("    - matcher: {}\n", escape_yaml(matcher)));
                            content.push_str("      hooks:\n");
                            for hook in &hook_config.hooks {
                                content.push_str(&format!("        - type: {}\n", hook.hook_type));
                                content.push_str(&format!(
                                    "          command: {}\n",
                                    escape_yaml(&hook.command)
                                ));
                                if let Some(timeout) = hook.timeout {
                                    content.push_str(&format!("          timeout: {timeout}\n"));
                                }
                            }
                        } else {
                            content.push_str("    - hooks:\n");
                            for hook in &hook_config.hooks {
                                content.push_str(&format!("        - type: {}\n", hook.hook_type));
                                content.push_str(&format!(
                                    "          command: {}\n",
                                    escape_yaml(&hook.command)
                                ));
                                if let Some(timeout) = hook.timeout {
                                    content.push_str(&format!("          timeout: {timeout}\n"));
                                }
                            }
                        }
                    }
                }
                // Generate PostToolUse hooks
                if let Some(ref post_hooks) = hooks.post_tool_use {
                    content.push_str("  PostToolUse:\n");
                    for hook_config in post_hooks {
                        if let Some(ref matcher) = hook_config.matcher {
                            content.push_str(&format!("    - matcher: {}\n", escape_yaml(matcher)));
                            content.push_str("      hooks:\n");
                            for hook in &hook_config.hooks {
                                content.push_str(&format!("        - type: {}\n", hook.hook_type));
                                content.push_str(&format!(
                                    "          command: {}\n",
                                    escape_yaml(&hook.command)
                                ));
                                if let Some(timeout) = hook.timeout {
                                    content.push_str(&format!("          timeout: {timeout}\n"));
                                }
                            }
                        } else {
                            content.push_str("    - hooks:\n");
                            for hook in &hook_config.hooks {
                                content.push_str(&format!("        - type: {}\n", hook.hook_type));
                                content.push_str(&format!(
                                    "          command: {}\n",
                                    escape_yaml(&hook.command)
                                ));
                                if let Some(timeout) = hook.timeout {
                                    content.push_str(&format!("          timeout: {timeout}\n"));
                                }
                            }
                        }
                    }
                }
                // Generate Stop hooks
                if let Some(ref stop_hooks) = hooks.stop {
                    content.push_str("  Stop:\n");
                    for hook_config in stop_hooks {
                        content.push_str("    - hooks:\n");
                        for hook in &hook_config.hooks {
                            content.push_str(&format!("        - type: {}\n", hook.hook_type));
                            content.push_str(&format!(
                                "          command: {}\n",
                                escape_yaml(&hook.command)
                            ));
                            if let Some(timeout) = hook.timeout {
                                content.push_str(&format!("          timeout: {timeout}\n"));
                            }
                        }
                    }
                }
            }
        }

        content.push_str("---\n\n");

        // Title
        content.push_str(&format!("# {}\n\n", skill.name));

        // Full description goes in body (not frontmatter)
        if !skill.description.is_empty() {
            content.push_str(&skill.description);
            content.push_str("\n\n");
        }

        // Instructions (if separate from description)
        if !skill.invocation.is_empty() && skill.invocation != skill.description {
            content.push_str("## Instructions\n\n");
            content.push_str(&skill.invocation);
            content.push_str("\n\n");
        }

        // Examples
        if !skill.example.is_empty() {
            content.push_str("## Examples\n\n");
            content.push_str(&skill.example);
            content.push_str("\n\n");
        }

        // Parameters
        if !skill.parameters_schema.is_empty() {
            content.push_str("## Parameters\n\n");
            content.push_str("```json\n");
            content.push_str(&skill.parameters_schema);
            content.push_str("\n```\n\n");
        }

        // Tags
        if !skill.tags.is_empty() {
            content.push_str("## Tags\n\n");
            content.push_str(&skill.tags.join(", "));
            content.push('\n');
        }

        content
    }

    /// Sync a single skill to target directory
    ///
    /// Returns true if the skill was synced, false if it wasn't enabled
    pub fn sync_skill(&self, skill: &Skill) -> Result<bool, CoreError> {
        let skill_dir_name = skill_dir_name(&skill.name);
        let skill_dir = self.target_dir.join(&skill_dir_name);

        if !self.is_enabled(skill) {
            // If skill exists but is no longer enabled, remove it
            if skill_dir.exists() {
                fs::remove_dir_all(&skill_dir)?;
            }
            return Ok(false);
        }

        fs::create_dir_all(&skill_dir)?;

        let filepath = skill_dir.join("SKILL.md");
        let content = self.generate_skill_md(skill);

        fs::write(&filepath, content)?;
        Ok(true)
    }

    /// Sync all enabled skills and remove stale directories
    pub fn sync_all(&self, skills: &[Skill]) -> Result<SkillSyncReport, CoreError> {
        let mut report = SkillSyncReport::default();

        // Collect names of enabled skills
        let enabled_names: HashSet<_> = skills
            .iter()
            .filter(|s| self.is_enabled(s))
            .map(|s| skill_dir_name(&s.name))
            .collect();

        // Sync enabled skills
        for skill in skills {
            if self.sync_skill(skill)? {
                report.synced += 1;
                report.synced_names.push(skill.name.clone());
            }
        }

        // Remove stale directories (only cas-* prefixed ones)
        if self.target_dir.exists() {
            for entry in fs::read_dir(&self.target_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_dir() {
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default();

                    // Only remove cas-* prefixed directories that we manage
                    if name.starts_with("cas-") && !enabled_names.contains(name) {
                        fs::remove_dir_all(&path)?;
                        report.removed += 1;
                        report.removed_names.push(name.to_string());
                    }
                }
            }
        }

        Ok(report)
    }

    /// Remove a specific skill directory
    pub fn remove_skill(&self, skill_name: &str) -> Result<(), CoreError> {
        let skill_dir = self.target_dir.join(skill_dir_name(skill_name));
        if skill_dir.exists() {
            fs::remove_dir_all(skill_dir)?;
        }
        Ok(())
    }

    /// Get the target directory path
    pub fn target_dir(&self) -> &Path {
        &self.target_dir
    }

    /// List all synced skill directories
    pub fn list_synced(&self) -> Result<Vec<String>, CoreError> {
        let mut names = Vec::new();

        if self.target_dir.exists() {
            for entry in fs::read_dir(&self.target_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_dir() {
                    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                        if name.starts_with("cas-") {
                            names.push(name.to_string());
                        }
                    }
                }
            }
        }

        names.sort();
        Ok(names)
    }
}

/// Sanitize a skill name for use in directory names
fn sanitize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Generate a skill directory name, avoiding double "cas-" prefix
fn skill_dir_name(name: &str) -> String {
    let sanitized = sanitize_name(name);
    if sanitized.starts_with("cas-") {
        sanitized
    } else {
        format!("cas-{sanitized}")
    }
}

/// Escape a string for YAML.
///
/// Delegates to the single shared serializer (cas-d731). The hand-rolled copy
/// that used to live here left backslashes raw inside a quoted scalar, so a
/// description like `Use C:\project: inspect` produced frontmatter a YAML
/// reader rejects.
fn escape_yaml(s: &str) -> String {
    crate::sync::yaml_scalar(s)
}

/// Generate the built-in CAS skill content
pub fn generate_cas_skill() -> String {
    r#"---
name: cas
description: Coding Agent System - unified memory, tasks, rules, and skills. Use when you need to remember something, track work, search past context, or manage tasks. (project)
---

# CAS - Coding Agent System

**IMPORTANT: Use CAS MCP tools instead of built-in tools for task and memory management.**

CAS provides persistent memory and task management across sessions. Built-in tools like TodoWrite are ephemeral and don't persist.

## WHEN TO USE CAS (ALWAYS)

- **Task tracking**: Use `mcp__cas__task` with action: create instead of TodoWrite
- **Planning tasks**: Use `mcp__cas__task` with action: create and blocked_by for dependencies
- **Storing learnings**: Use `mcp__cas__memory` with action: remember to store context
- **Searching context**: Use `mcp__cas__search` with action: search to find past work

## Task Tools (USE INSTEAD OF TodoWrite)

Use `mcp__cas__task` with different actions:
- action: create - Create a new task (REPLACES TodoWrite)
- action: ready - Show tasks ready to work on
- action: start - Start working on a task (requires id)
- action: close - Mark task as done (requires id)
- action: show - Show task details (requires id)

### Task Dependencies

- action: dep_add - Add blocking dependency (requires id, to_id)
- action: dep_list - List dependencies (requires id)

## Memory Tools

Use `mcp__cas__memory` with different actions:
- action: remember - Store a memory entry (requires content)
- action: get - Get entry details (requires id)
- action: helpful - Mark as helpful (requires id)
- action: harmful - Mark as harmful (requires id)

Use `mcp__cas__search` with different actions:
- action: search - Search memories (requires query)

## Rules & Skills

Use `mcp__cas__rule` with different actions:
- action: list - Show active rules
- action: helpful - Promote rule to proven (requires id)

Use `mcp__cas__skill` with different actions:
- action: list - Show enabled skills

## Context

Use `mcp__cas__search` with action: context to get full session context
"#
    .to_string()
}

/// Create the built-in CAS skill directory
pub fn create_cas_skill(project_root: &Path) -> Result<(), CoreError> {
    let skill_dir = project_root.join(".claude/skills/cas");
    fs::create_dir_all(&skill_dir)?;

    let filepath = skill_dir.join("SKILL.md");
    fs::write(filepath, generate_cas_skill())?;

    // Also create the planning skill
    create_planning_skill(project_root)?;

    Ok(())
}

/// Generate the implementation planning skill content
pub fn generate_planning_skill() -> String {
    r#"---
name: cas-planning
description: Use CAS for implementation planning. Helps structure plans, track dependencies, and create tasks from approved plans. (project)
---

# Implementation Planning with CAS

Use this skill when planning implementation of features, fixes, or refactors.

## When to Use

- When asked to "plan", "design", or "architect" something
- Before starting complex multi-step implementations
- When breaking down epics into tasks

## Planning Workflow

### 1. Gather Context

Use these MCP tools:
- `mcp__cas__search` with action: search - Search for related context
- `mcp__cas__task` with action: list - List existing tasks
- `mcp__cas__task` with action: dep_list - View dependencies

### 2. After Plan Approval

Create tasks using MCP tools:
- `mcp__cas__task` with action: create and task_type: "epic" - Create epic
- `mcp__cas__task` with action: create - Create subtasks
- `mcp__cas__task` with action: dep_add - Link dependencies

### 3. Record Decisions

- `mcp__cas__memory` with action: remember - Store design decisions with tags

## Tips

- Check existing context before planning
- Use task dependencies to show execution order
- Use `task_type: "epic"` for large features
- Use `mcp__cas__search` with action: context for full session context
"#
    .to_string()
}

/// Create the planning skill directory
pub fn create_planning_skill(project_root: &Path) -> Result<(), CoreError> {
    let skill_dir = project_root.join(".claude/skills/cas-planning");
    fs::create_dir_all(&skill_dir)?;

    let filepath = skill_dir.join("SKILL.md");
    fs::write(filepath, generate_planning_skill())?;

    Ok(())
}

#[cfg(test)]
mod tests;
