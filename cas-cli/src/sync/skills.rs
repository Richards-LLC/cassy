//! Claude Code skill syncing
//!
//! Syncs enabled Cassy skills to .claude/skills/ as Agent Skills for Claude Code.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::builtins::is_managed_by_cas;
use crate::error::CasError;
use crate::types::{Skill, SkillStatus};

/// Syncs Cassy skills to Claude Code Agent Skills
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
        if !skill.source_ids.is_empty() {
            content.push_str("source_ids:\n");
            for source_id in &skill.source_ids {
                content.push_str(&format!("  - {}\n", escape_yaml(source_id)));
            }
        }
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

        if !skill.preconditions.is_empty() {
            content.push_str("## Preconditions\n\n");
            for precondition in &skill.preconditions {
                content.push_str(&format!("- {precondition}\n"));
            }
            content.push('\n');
        }

        if !skill.postconditions.is_empty() {
            content.push_str("## Postconditions\n\n");
            for postcondition in &skill.postconditions {
                content.push_str(&format!("- {postcondition}\n"));
            }
            content.push('\n');
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
    /// Returns true if the skill was synced, false if it wasn't enabled or conflicts with builtin
    pub fn sync_skill(&self, skill: &Skill) -> Result<bool, CasError> {
        let skill_dir_name = skill_dir_name(&skill.name);
        let skill_dir = self.target_dir.join(&skill_dir_name);

        if !self.is_enabled(skill) {
            // If skill exists but is no longer enabled, remove it
            // But only if it's not a builtin (managed by Cassy)
            if skill_dir.exists() {
                let skill_file = skill_dir.join("SKILL.md");
                if skill_file.exists() {
                    if let Ok(content) = fs::read_to_string(&skill_file) {
                        if is_managed_by_cas(&content) {
                            // This is a builtin skill - don't remove it
                            return Ok(false);
                        }
                    }
                }
                fs::remove_dir_all(&skill_dir)?;
            }
            return Ok(false);
        }

        // Check if this would overwrite a builtin skill
        let filepath = skill_dir.join("SKILL.md");
        if filepath.exists() {
            if let Ok(existing) = fs::read_to_string(&filepath) {
                if is_managed_by_cas(&existing) {
                    // This is a builtin skill - don't overwrite it with database skill
                    return Ok(false);
                }
            }
        }

        fs::create_dir_all(&skill_dir)?;

        let content = self.generate_skill_md(skill);
        fs::write(&filepath, content)?;
        Ok(true)
    }

    /// Sync all enabled skills and remove stale directories
    pub fn sync_all(&self, skills: &[Skill]) -> Result<SkillSyncReport, CasError> {
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

        // Remove stale directories (only cas-* prefixed ones that aren't builtins)
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
                        // Check if this is a builtin skill (has managed_by: cas marker)
                        let skill_file = path.join("SKILL.md");
                        if skill_file.exists() {
                            if let Ok(content) = fs::read_to_string(&skill_file) {
                                if is_managed_by_cas(&content) {
                                    // This is a builtin skill - don't remove it
                                    continue;
                                }
                            }
                        }

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
    pub fn remove_skill(&self, skill_name: &str) -> Result<(), CasError> {
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
    pub fn list_synced(&self) -> Result<Vec<String>, CasError> {
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
    cas_core::sync::yaml_scalar(s)
}

/// Read all skills from the .claude/skills/ directory
/// This reads SKILL.md files and parses their YAML frontmatter
pub fn read_skills_from_files(project_root: &Path) -> Result<Vec<Skill>, CasError> {
    let skills_dir = project_root.join(".claude/skills");
    let mut skills = Vec::new();

    if !skills_dir.exists() {
        return Ok(skills);
    }

    for entry in fs::read_dir(&skills_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let skill_file = path.join("SKILL.md");
            if skill_file.exists() {
                if let Ok(skill) = parse_skill_file(&skill_file) {
                    skills.push(skill);
                }
            }
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// Read a single skill from file by name (directory name)
pub fn read_skill_from_file(project_root: &Path, name: &str) -> Result<Option<Skill>, CasError> {
    // Try exact name first
    let skill_file = project_root
        .join(".claude/skills")
        .join(name)
        .join("SKILL.md");
    if skill_file.exists() {
        return Ok(Some(parse_skill_file(&skill_file)?));
    }

    // Try with cas- prefix
    let prefixed_name = if name.starts_with("cas-") {
        name.to_string()
    } else {
        format!("cas-{name}")
    };
    let skill_file = project_root
        .join(".claude/skills")
        .join(&prefixed_name)
        .join("SKILL.md");
    if skill_file.exists() {
        return Ok(Some(parse_skill_file(&skill_file)?));
    }

    Ok(None)
}

/// Parse a SKILL.md file and extract skill information from frontmatter
fn parse_skill_file(path: &Path) -> Result<Skill, CasError> {
    let content = fs::read_to_string(path)?;

    // Extract frontmatter (between --- markers)
    let frontmatter = extract_frontmatter(&content);
    let body = extract_body(&content);

    // Parse frontmatter values
    let name = extract_yaml_value(&frontmatter, "name").unwrap_or_else(|| {
        // Use directory name as fallback
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    let description = extract_yaml_value(&frontmatter, "description").unwrap_or_default();

    let summary = description.clone(); // Use description as summary for file-based skills

    let invokable = !extract_yaml_value(&frontmatter, "user-invocable")
        .map(|v| v == "false")
        .unwrap_or(false);

    let argument_hint = extract_yaml_value(&frontmatter, "argument-hint").unwrap_or_default();

    let context_mode = extract_yaml_value(&frontmatter, "context");
    let agent_type = extract_yaml_value(&frontmatter, "agent");

    let allowed_tools = extract_yaml_list(&frontmatter, "allowed-tools");
    let disallowed_tools = extract_yaml_list(&frontmatter, "disallowed-tools");
    let source_ids = extract_yaml_list(&frontmatter, "source_ids");

    let disable_model_invocation = extract_yaml_value(&frontmatter, "disable-model-invocation")
        .map(|v| v == "true")
        .unwrap_or(false);

    let managed_by_cas = extract_yaml_value(&frontmatter, "managed_by")
        .map(|v| v == "cas")
        .unwrap_or(false);

    // Generate ID from name
    let id = if let Some(stripped) = name.strip_prefix("cas-") {
        format!("file-{stripped}")
    } else {
        format!("file-{name}")
    };

    let mut skill = Skill::new(id, name);
    skill.description = body;
    skill.summary = summary;
    skill.invokable = invokable;
    skill.argument_hint = argument_hint;
    skill.context_mode = context_mode;
    skill.agent_type = agent_type;
    skill.allowed_tools = allowed_tools;
    skill.disallowed_tools = disallowed_tools;
    skill.source_ids = source_ids;
    skill.disable_model_invocation = disable_model_invocation;
    skill.status = SkillStatus::Enabled;
    skill.skill_type = if managed_by_cas {
        crate::types::SkillType::Internal
    } else {
        crate::types::SkillType::Mcp
    };

    Ok(skill)
}

/// Extract YAML frontmatter from content (between --- markers)
fn extract_frontmatter(content: &str) -> String {
    if !content.starts_with("---") {
        return String::new();
    }

    if let Some(end_pos) = content[3..].find("\n---") {
        content[3..3 + end_pos].trim().to_string()
    } else {
        String::new()
    }
}

/// Extract body content after frontmatter
fn extract_body(content: &str) -> String {
    if !content.starts_with("---") {
        return content.to_string();
    }

    if let Some(end_pos) = content[3..].find("\n---") {
        let body_start = 3 + end_pos + 4; // Skip past "\n---"
        if body_start < content.len() {
            content[body_start..].trim().to_string()
        } else {
            String::new()
        }
    } else {
        content.to_string()
    }
}

/// Extract a single value from YAML frontmatter
/// Reads one scalar out of generated frontmatter.
///
/// cas-d731. The writer now emits real YAML, which means a value can carry
/// real YAML quoting and escapes. The scan below cannot decode those — it
/// trims quote characters and returns the rest verbatim — so a parser is asked
/// first and the scan survives only for files the parser rejects.
///
/// That fallback is not defensive habit: every `SKILL.md` written before this
/// change by the old escaper is exactly such a file. `Use C:\project: inspect`
/// was written as `description: "Use C:\project: inspect"`, which no YAML
/// parser accepts, and those files are on disk in every checkout that has run
/// a sync. Dropping the scan would make CAS stop reading its own history.
///
/// The fallback is narrow by construction: it runs only when the frontmatter
/// fails to parse as YAML. A document that parses but lacks the key, or holds
/// a value that is not a scalar, returns `None` — the parser's answer stands.
fn extract_yaml_value(frontmatter: &str, key: &str) -> Option<String> {
    match serde_yaml::from_str::<serde_yaml::Value>(frontmatter) {
        Ok(parsed) => scalar_field(&parsed, key),
        // Only a document YAML cannot read at all falls through to the legacy
        // scan, because only the pre-fix writer produced those.
        Err(_) => legacy_line_scan(frontmatter, key),
    }
}

/// Exact-key scalar lookup that preserves the typed-field behaviour callers
/// already depend on: `user-invocable: false` and `disable-model-invocation:
/// true` are compared as the strings `"false"` and `"true"` upstream, so a
/// YAML boolean has to read back as that text rather than as `None`.
fn scalar_field(parsed: &serde_yaml::Value, key: &str) -> Option<String> {
    let value = parsed.get(key)?;
    let text = match value {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Number(n) => n.to_string(),
        // A sequence, mapping or null is not a scalar. Reporting it as absent
        // is the honest answer and matches what the old scan did with a list.
        _ => return None,
    };
    if text.is_empty() { None } else { Some(text) }
}

/// The pre-cas-d731 reader, kept verbatim for frontmatter that is not valid
/// YAML. Prefix-matches the key exactly as it always did, so files it could
/// read before are read identically now.
fn legacy_line_scan(frontmatter: &str, key: &str) -> Option<String> {
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(key) {
            if let Some(colon_pos) = trimmed.find(':') {
                let value = trimmed[colon_pos + 1..].trim();
                // Remove quotes if present
                let value = value.trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Extract a YAML list from frontmatter
fn extract_yaml_list(frontmatter: &str, key: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut in_list = false;

    for line in frontmatter.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with(key) && trimmed.contains(':') {
            let value = trimmed
                .split_once(':')
                .map(|(_, value)| value.trim())
                .unwrap_or_default();
            if value.starts_with('[') && value.ends_with(']') {
                return value[1..value.len() - 1]
                    .split(',')
                    .map(|item| item.trim().trim_matches('"').trim_matches('\''))
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            in_list = true;
            continue;
        }

        if in_list {
            if let Some(stripped) = trimmed.strip_prefix("- ") {
                let value = stripped.trim().trim_matches('"').trim_matches('\'');
                result.push(value.to_string());
            } else if !trimmed.starts_with('-') && !trimmed.is_empty() {
                // End of list
                break;
            }
        }
    }

    result
}

/// Generate the built-in Cassy skill content (MCP-only)
pub fn generate_cas_skill() -> String {
    r#"---
name: cas
description: Coding Agent System - unified memory, tasks, rules, and skills. Use when you need to remember something, track work, search past context, or manage tasks. (project)
---

# Cassy - Coding Agent System

**IMPORTANT: Use CAS MCP tools instead of built-in tools for task and memory management.**

Cassy provides persistent memory and task management across sessions. Built-in tools like TodoWrite are ephemeral and don't persist.

## WHEN TO USE Cassy (ALWAYS)

- **Task tracking**: Use `mcp__cas__task action=create` instead of TodoWrite
- **Planning tasks**: Use `mcp__cas__task action=create` with dependencies
- **Storing learnings**: Use `mcp__cas__memory action=remember` to store context
- **Searching context**: Use `mcp__cas__search action=search` to find past work

## Task Tools (USE INSTEAD OF TodoWrite)

- `mcp__cas__task action=create` - Create a new task (REPLACES TodoWrite)
- `mcp__cas__task action=ready` - Show tasks ready to work on
- `mcp__cas__task action=start` - Start working on a task
- `mcp__cas__task action=close` - Mark task as done
- `mcp__cas__task action=show` - Show task details

### Task Dependencies

- `mcp__cas__task action=dep_add` - Add blocking dependency
- `mcp__cas__task action=dep_list` - List dependencies

## Memory Tools

- `mcp__cas__memory action=remember` - Store a memory entry
- `mcp__cas__search action=search` - Search memories
- `mcp__cas__memory action=get` - Get entry details
- `mcp__cas__memory action=helpful` / `action=harmful` - Provide feedback

## Rules & Skills

- `mcp__cas__rule action=list` - Show active rules
- `mcp__cas__rule action=helpful` - Promote rule to proven
- `mcp__cas__skill action=list` - Show enabled skills

## Context

- `mcp__cas__search action=context` - Get full session context
"#
    .to_string()
}

/// Create the built-in Cassy skill directory
pub fn create_cas_skill(project_root: &Path) -> Result<(), CasError> {
    let skill_dir = project_root.join(".claude/skills/cas");
    fs::create_dir_all(&skill_dir)?;

    let filepath = skill_dir.join("SKILL.md");
    fs::write(filepath, generate_cas_skill())?;

    // Also create the planning skill
    create_planning_skill(project_root)?;

    Ok(())
}

/// Generate the implementation planning skill content (MCP-only)
pub fn generate_planning_skill() -> String {
    r#"---
name: cas-planning
description: Use Cassy for implementation planning. Helps structure plans, track dependencies, and create tasks from approved plans. (project)
---

# Implementation Planning with Cassy

Use this skill when planning implementation of features, fixes, or refactors.

## When to Use

- When asked to "plan", "design", or "architect" something
- Before starting complex multi-step implementations
- When breaking down epics into tasks

## Planning Workflow

### 1. Gather Context

Use these MCP tools:
- `mcp__cas__search action=search` - Search for related context
- `mcp__cas__task action=list` - List existing tasks
- `mcp__cas__task action=dep_list` - View dependencies

### 2. After Plan Approval

Create tasks using MCP tools:
- `mcp__cas__task action=create task_type=epic` - Create epic
- `mcp__cas__task action=create` - Create subtasks
- `mcp__cas__task action=dep_add` - Link dependencies

### 3. Record Decisions

- `mcp__cas__memory action=remember` - Store design decisions with tags

## Tips

- Check existing context before planning
- Use task dependencies to show execution order
- Use `task_type: "epic"` for large features
- Use `mcp__cas__search action=context` for full session context
"#
    .to_string()
}

/// Create the planning skill directory
pub fn create_planning_skill(project_root: &Path) -> Result<(), CasError> {
    let skill_dir = project_root.join(".claude/skills/cas-planning");
    fs::create_dir_all(&skill_dir)?;

    let filepath = skill_dir.join("SKILL.md");
    fs::write(filepath, generate_planning_skill())?;

    Ok(())
}

#[cfg(test)]
#[path = "skills_tests/tests.rs"]
mod tests;
