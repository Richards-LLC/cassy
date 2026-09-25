use std::path::{Path, PathBuf};

pub(crate) const CAS_SECTION_BEGIN: &str =
    "<!-- CAS:BEGIN - This section is managed by CAS. Do not edit manually. -->";
pub(crate) const CAS_SECTION_END: &str = "<!-- CAS:END -->";

/// Cassy directive content (MCP tools)
const CAS_DIRECTIVE_CONTENT: &str = r#"# IMPORTANT: USE Cassy FOR TASK AND MEMORY MANAGEMENT

**DO NOT USE BUILT-IN TOOLS (TodoWrite, EnterPlanMode) FOR TASK TRACKING.**

Use CAS MCP tools instead:
First use each session — load MCP schemas: ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search"). ToolSearch only loads the schema — it does not call the tool. Once it succeeds, call `mcp__cas__task` etc. directly; never re-run ToolSearch for a tool already resolved.
- `mcp__cas__task` with action: create - Create tasks (NOT TodoWrite)
- `mcp__cas__task` with action: start/close - Manage task status
- `mcp__cas__task` with action: ready - See ready tasks
- `mcp__cas__memory` with action: remember - Store memories and learnings
- `mcp__cas__search` with action: search - Search all context

Cassy provides persistent context across sessions. Built-in tools are ephemeral.

Bug routing: `cas config get issues.repo` / `issues.components.{cassy,violet,cloud}` name the project, Cassy, Violet and Cloud trackers; file operational bugs in the matching repo before moving on.
Release notes: when a merge reaches `staging` or `main`, use the `release-notes` skill and follow docs/release-notes/RUBRIC.md."#;

/// Build the full Cassy section with markers
pub(crate) fn build_cas_section() -> String {
    format!("{CAS_SECTION_BEGIN}\n{CAS_DIRECTIVE_CONTENT}\n{CAS_SECTION_END}")
}

/// Returns true if any ancestor directory of `project_root` (from its parent
/// up to and including `$HOME`) already contains a CLAUDE.md with the Cassy
/// managed block.
///
/// If `project_root` IS `$HOME`, returns false immediately — the root is
/// always the canonical injection point, never a "descendant" of itself.
///
/// Paths are canonicalized before comparison to avoid symlink loops.
fn ancestor_has_cas_block(project_root: &Path) -> bool {
    // Resolve $HOME once; if unset or unresolvable, walk to filesystem root.
    let home: Option<PathBuf> = std::env::var_os("HOME").map(PathBuf::from).map(|h| {
        h.canonicalize().unwrap_or(h)
    });

    // Canonicalize project_root to resolve any symlinks in the path.
    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    // If project_root IS $HOME, it is the root anchor — always inject here.
    if let Some(ref home) = home {
        if canonical_root == *home {
            return false;
        }
    }

    let mut current = canonical_root.parent();
    while let Some(dir) = current {
        let claude_md = dir.join("CLAUDE.md");
        if claude_md.exists() {
            if let Ok(content) = std::fs::read_to_string(&claude_md) {
                if content.contains(CAS_SECTION_BEGIN) {
                    return true;
                }
            }
        }

        // Stop after checking $HOME — do not traverse above it.
        if let Some(ref home) = home {
            if dir == home.as_path() {
                break;
            }
        }

        current = dir.parent();
    }

    false
}

/// Update or create CLAUDE.md with Cassy directive section
/// Returns Ok(true) if file was modified, Ok(false) if no changes needed
pub fn update_claude_md(project_root: &Path) -> anyhow::Result<bool> {
    // Skip injection when an ancestor already carries the managed block.
    // The shallowest ancestor (typically ~/CLAUDE.md) is the canonical copy;
    // injecting into every descendent project multiplies context noise without value.
    // Existing duplicate blocks at this level are left untouched (not deleted).
    if ancestor_has_cas_block(project_root) {
        return Ok(false);
    }

    let claude_md_path = project_root.join("CLAUDE.md");
    let new_section = build_cas_section();

    if claude_md_path.exists() {
        let content = std::fs::read_to_string(&claude_md_path)?;

        // Check for marked section
        if let (Some(begin_pos), Some(end_pos)) = (
            content.find(CAS_SECTION_BEGIN),
            content.find(CAS_SECTION_END),
        ) {
            // Replace existing marked section
            let before = &content[..begin_pos];
            let after = &content[end_pos + CAS_SECTION_END.len()..];
            let new_content = format!(
                "{}{}{}",
                before.trim_end(),
                if before.is_empty() { "" } else { "\n" },
                new_section
            );
            let new_content = format!("{new_content}{after}");

            if new_content == content {
                return Ok(false);
            }
            std::fs::write(&claude_md_path, new_content)?;
            return Ok(true);
        }

        // Check for old-style directive (migration path)
        if content.contains("IMPORTANT: USE Cassy FOR TASK AND MEMORY MANAGEMENT") {
            let new_content = if content.starts_with("# IMPORTANT: USE Cassy") {
                if let Some(pos) = content.find("---\n\n") {
                    format!("{}\n\n{}", new_section, &content[pos + 5..])
                } else if let Some(pos) = content.find("---\n") {
                    format!("{}\n\n{}", new_section, &content[pos + 4..])
                } else {
                    format!("{new_section}\n\n{content}")
                }
            } else {
                format!("{new_section}\n\n{content}")
            };
            std::fs::write(&claude_md_path, new_content)?;
            return Ok(true);
        }

        // Prepend new section to existing content
        let new_content = format!("{new_section}\n\n{content}");
        std::fs::write(&claude_md_path, new_content)?;
        Ok(true)
    } else {
        std::fs::write(&claude_md_path, format!("{new_section}\n"))?;
        Ok(true)
    }
}

// ============================================================================
// Cassy skill generation
// ============================================================================

pub(crate) const CAS_SKILL: &str = r#"---
name: cas
description: Coding Agent System - unified memory, tasks, rules, and skills. Use when you need to remember something, track work, search past context, or manage tasks. (project)
managed_by: cas
---

# Cassy - Coding Agent System

Use Cassy MCP tools, not built-in TodoWrite or plan mode, for work that must outlive the session:

- Track work with `mcp__cas__task` (see the `cas-task-tracking` skill).
- Store facts and learnings with `mcp__cas__memory` (see `cas-memory-management`).
- Find past tasks, memories, code and context with `mcp__cas__search` (see `cas-search`).

Each tool's MCP schema lists its actions and parameters; follow it rather than a remembered parameter list.
"#;

/// Check if a file is managed by Cassy (has `managed_by: cas` in frontmatter)
pub(crate) fn is_skill_managed_by_cas(content: &str) -> bool {
    if let Some(stripped) = content.strip_prefix("---") {
        if let Some(end) = stripped.find("---") {
            let frontmatter = &content[3..3 + end];
            return frontmatter.contains("managed_by: cas")
                || frontmatter.contains("managed_by: \"cas\"");
        }
    }
    false
}

/// Check if a file is the old Cassy skill (for migration)
pub(crate) fn is_old_cas_skill(content: &str) -> bool {
    if let Some(stripped) = content.strip_prefix("---") {
        if let Some(end) = stripped.find("---") {
            let frontmatter = &content[3..3 + end];
            return frontmatter.contains("name: cas") && !frontmatter.contains("managed_by:");
        }
    }
    false
}

/// Generate Cassy skill file
pub fn generate_cas_skill(project_root: &Path) -> anyhow::Result<bool> {
    let skill_dir = project_root.join(".claude/skills/cas");
    let skill_path = skill_dir.join("SKILL.md");
    let skill_content = CAS_SKILL;

    std::fs::create_dir_all(&skill_dir)?;

    if skill_path.exists() {
        let existing = std::fs::read_to_string(&skill_path)?;

        if existing == skill_content {
            return Ok(false);
        }

        if !is_skill_managed_by_cas(&existing) && !is_old_cas_skill(&existing) {
            return Ok(false);
        }
    }

    std::fs::write(&skill_path, skill_content)?;
    Ok(true)
}

// ============================================================================
// Agent and command generation (using builtins)
// ============================================================================

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use std::fs;

    /// The managed block must document the ToolSearch bootstrap query so that
    /// Claude knows how to load MCP schemas before calling task/memory/search.
    #[test]
    fn template_documents_toolsearch_bootstrap() {
        let section = build_cas_section();
        assert!(
            section.contains(r#"ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search")"#),
            "Managed block must contain the exact ToolSearch bootstrap query; got:\n{section}"
        );
    }

    /// cas-e7c8: a haiku/low-tier worker never got past the ToolSearch
    /// discovery step — it re-ran ToolSearch 7+ times instead of calling
    /// `mcp__cas__task` directly. The CLAUDE.md-level bootstrap line is the
    /// very first thing every session reads, so it must say explicitly that
    /// ToolSearch only loads the schema and does not call the tool.
    #[test]
    fn template_clarifies_toolsearch_does_not_call_the_tool() {
        let section = build_cas_section();
        assert!(
            section.contains("ToolSearch only loads the schema — it does not call the tool"),
            "Managed block must clarify ToolSearch is a discovery step, not a call; got:\n{section}"
        );
    }

    /// GH #65: the Cassy-managed block must breadcrumb the release-notes rubric
    /// so every Cassy project inherits the "announce every staging/main merge"
    /// expectation instead of it living only in one project's hand-edited
    /// CLAUDE.md.
    #[test]
    fn template_breadcrumbs_release_notes_rubric() {
        let section = build_cas_section();
        assert!(
            section.contains("docs/release-notes/RUBRIC.md"),
            "Managed block must point at the release-notes rubric; got:\n{section}"
        );
        assert!(
            section.contains("staging") && section.contains("main"),
            "Managed block breadcrumb must name the staging/main merge trigger; got:\n{section}"
        );
    }

    /// The full managed block (markers + content) must stay ≤ 18 lines to
    /// keep the per-session context tax small (sister task cas-253e dedupes
    /// the block; this bounds its cost when it *does* appear).
    #[test]
    fn managed_block_line_count_within_budget() {
        let section = build_cas_section();
        let line_count = section.lines().count();
        assert!(
            line_count <= 18,
            "Managed CLAUDE.md block must be ≤ 18 lines (current: {line_count});\
             if you added content, trim elsewhere"
        );
    }

    /// No ancestor has the managed block → injection proceeds.
    #[test]
    fn test_no_ancestor_block_writes_block() {
        TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            fs::create_dir_all(&project).unwrap();

            let result = update_claude_md(&project).unwrap();
            assert!(result, "expected block to be written when no ancestor has it");
            let content = fs::read_to_string(project.join("CLAUDE.md")).unwrap();
            assert!(content.contains(CAS_SECTION_BEGIN));
        });
    }

    /// An ancestor directory already has the Cassy block → injection is skipped.
    /// FAILING before fix: current code writes the block regardless of ancestors.
    #[test]
    fn test_ancestor_has_block_skips_injection() {
        TestEnvGuard::run_with_temp_home(|home| {
            // Parent dir inside HOME gets the managed block.
            let parent = home.join("parent");
            fs::create_dir_all(&parent).unwrap();
            fs::write(parent.join("CLAUDE.md"), build_cas_section()).unwrap();

            // Project is a child of parent.
            let project = parent.join("project");
            fs::create_dir_all(&project).unwrap();

            let result = update_claude_md(&project).unwrap();
            assert!(
                !result,
                "expected injection to be skipped when ancestor has the block"
            );
            assert!(
                !project.join("CLAUDE.md").exists(),
                "CLAUDE.md should not be created when ancestor already has the block"
            );
        });
    }

    /// The user-global ($HOME-level) CLAUDE.md always receives the block (root of chain).
    #[test]
    fn test_home_level_always_injects() {
        TestEnvGuard::run_with_temp_home(|home| {
            let result = update_claude_md(home).unwrap();
            assert!(result, "expected block to be written at HOME level");
            let content = fs::read_to_string(home.join("CLAUDE.md")).unwrap();
            assert!(content.contains(CAS_SECTION_BEGIN));
        });
    }

    /// Existing project-level block is left untouched when ancestor also has it.
    /// The new logic must skip re-injection but NOT delete the existing block.
    #[test]
    fn test_existing_project_block_preserved_when_ancestor_has_block() {
        TestEnvGuard::run_with_temp_home(|home| {
            // HOME-level CLAUDE.md has the managed block.
            fs::write(home.join("CLAUDE.md"), build_cas_section()).unwrap();

            // Project also has the block (pre-existing duplicate).
            let project = home.join("project");
            fs::create_dir_all(&project).unwrap();
            let project_claude = project.join("CLAUDE.md");
            fs::write(&project_claude, build_cas_section()).unwrap();

            // update_claude_md must not delete the existing block.
            let _ = update_claude_md(&project);
            let content = fs::read_to_string(&project_claude).unwrap();
            assert!(
                content.contains(CAS_SECTION_BEGIN),
                "existing project-level block must not be deleted"
            );
        });
    }

    /// A symlinked project path doesn't cause an infinite loop during ancestor walk.
    #[test]
    #[cfg(unix)]
    fn test_symlink_ancestor_no_infinite_loop() {
        TestEnvGuard::run_with_temp_home(|home| {
            let real_dir = home.join("real_project");
            fs::create_dir_all(&real_dir).unwrap();

            let link_path = home.join("linked_project");
            std::os::unix::fs::symlink(&real_dir, &link_path).unwrap();

            // Should complete without hanging or panicking.
            let result = update_claude_md(&link_path);
            assert!(
                result.is_ok(),
                "symlinked project path must not cause an error"
            );
        });
    }
}
