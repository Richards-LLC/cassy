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

/// What the nearest-to-root ancestors of a project carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AncestorBlock {
    /// No ancestor CLAUDE.md (up to and including `$HOME`) has the block.
    None,
    /// At least one ancestor has the block, but none carries the current text.
    Stale,
    /// At least one ancestor carries the current block verbatim.
    Current,
}

/// Classify the Cassy managed blocks carried by the ancestors of
/// `project_root` (from its parent up to and including `$HOME`).
///
/// If `project_root` IS `$HOME`, returns `None` immediately — the root is
/// always the canonical injection point, never a "descendant" of itself.
///
/// Paths are canonicalized before comparison to avoid symlink loops.
fn ancestor_cas_block(project_root: &Path) -> AncestorBlock {
    // Resolve $HOME once; if unset or unresolvable, walk to filesystem root.
    let home: Option<PathBuf> = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|h| h.canonicalize().unwrap_or(h));

    // Canonicalize project_root to resolve any symlinks in the path.
    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    // If project_root IS $HOME, it is the root anchor — always inject here.
    if let Some(ref home) = home {
        if canonical_root == *home {
            return AncestorBlock::None;
        }
    }

    let current_section = build_cas_section();
    let mut found = AncestorBlock::None;
    let mut current = canonical_root.parent();
    while let Some(dir) = current {
        let claude_md = dir.join("CLAUDE.md");
        if claude_md.exists() {
            if let Ok(content) = std::fs::read_to_string(&claude_md) {
                if content.contains(&current_section) {
                    return AncestorBlock::Current;
                }
                if content.contains(CAS_SECTION_BEGIN) {
                    found = AncestorBlock::Stale;
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

    found
}

/// The single decision `cas init`, `cas update` and the `cas update
/// --dry-run` preview all apply to a project's CLAUDE.md, so the preview can
/// never promise a change that apply will not make (L6 F6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaudeMdPlan {
    /// Nothing to do.
    Unchanged,
    /// Write a new CLAUDE.md with this content.
    Create { content: String },
    /// Rewrite the existing CLAUDE.md.
    Modify {
        old: String,
        new: String,
        description: &'static str,
    },
    /// Delete CLAUDE.md: it held only a managed block that an ancestor
    /// already carries.
    Delete { old: String },
}

/// Byte range of the managed block (markers included), if well-formed.
fn managed_block_range(content: &str) -> Option<(usize, usize)> {
    let begin = content.find(CAS_SECTION_BEGIN)?;
    let end = begin + content[begin..].find(CAS_SECTION_END)? + CAS_SECTION_END.len();
    Some((begin, end))
}

/// Replace the managed block in `content` with `new_section`.
fn replace_managed_block(content: &str, begin: usize, end: usize, new_section: &str) -> String {
    let before = &content[..begin];
    let after = &content[end..];
    format!(
        "{}{}{}{}",
        before.trim_end(),
        if before.is_empty() { "" } else { "\n" },
        new_section,
        after
    )
}

/// Remove the managed block from `content`, joining the surrounding text
/// with one blank line.
fn remove_managed_block(content: &str, begin: usize, end: usize) -> String {
    let before = content[..begin].trim_end();
    let after = content[end..].trim_start_matches(['\r', '\n']);
    match (before.is_empty(), after.is_empty()) {
        (true, _) => after.to_string(),
        (false, true) => format!("{before}\n"),
        (false, false) => format!("{before}\n\n{after}"),
    }
}

/// True when `path` is tracked in its git repository. When git cannot be run
/// the answer is unknown, so this says tracked: the caller then only
/// refreshes, never removes.
fn is_git_tracked(path: &Path) -> bool {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    match std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(_) => true,
    }
}

/// Decide what should happen to `project_root/CLAUDE.md`.
///
/// - No ancestor carries the block: create, refresh, migrate or prepend it
///   here (this directory is the canonical copy).
/// - An ancestor carries the current block: a block here is a duplicate that
///   costs every session its tokens, so remove it — deleting the file when
///   nothing else is left. No block is ever added. A git-tracked CLAUDE.md is
///   refreshed instead: its committed block serves every other clone, and
///   pruning it locally would leave a permanent diff.
/// - Ancestors carry only a stale block: refresh an existing block here so
///   the session at least sees the current text; never add a new one.
pub(crate) fn plan_claude_md(project_root: &Path) -> anyhow::Result<ClaudeMdPlan> {
    let claude_md_path = project_root.join("CLAUDE.md");
    let new_section = build_cas_section();
    let ancestor = ancestor_cas_block(project_root);

    let content = if claude_md_path.exists() {
        Some(std::fs::read_to_string(&claude_md_path)?)
    } else {
        None
    };

    let Some(content) = content else {
        return Ok(match ancestor {
            AncestorBlock::None => ClaudeMdPlan::Create {
                content: format!("{new_section}\n"),
            },
            AncestorBlock::Stale | AncestorBlock::Current => ClaudeMdPlan::Unchanged,
        });
    };

    if let Some((begin, end)) = managed_block_range(&content) {
        if ancestor == AncestorBlock::Current && !is_git_tracked(&claude_md_path) {
            let pruned = remove_managed_block(&content, begin, end);
            if pruned.trim().is_empty() {
                return Ok(ClaudeMdPlan::Delete { old: content });
            }
            return Ok(ClaudeMdPlan::Modify {
                old: content,
                new: pruned,
                description: "Remove duplicate Cassy section (an ancestor CLAUDE.md carries it)",
            });
        }
        let refreshed = replace_managed_block(&content, begin, end, &new_section);
        if refreshed == content {
            return Ok(ClaudeMdPlan::Unchanged);
        }
        return Ok(ClaudeMdPlan::Modify {
            old: content,
            new: refreshed,
            description: "Update Cassy section in CLAUDE.md",
        });
    }

    // No managed block here. Never add one below an ancestor that has it.
    if ancestor != AncestorBlock::None {
        return Ok(ClaudeMdPlan::Unchanged);
    }

    // Old-style directive (migration path)
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
        return Ok(ClaudeMdPlan::Modify {
            old: content,
            new: new_content,
            description: "Migrate Cassy section format in CLAUDE.md",
        });
    }

    // Prepend new section to existing content
    let new_content = format!("{new_section}\n\n{content}");
    Ok(ClaudeMdPlan::Modify {
        old: content,
        new: new_content,
        description: "Add Cassy section to CLAUDE.md",
    })
}

/// Update, create, prune or delete CLAUDE.md per [`plan_claude_md`].
/// Returns Ok(true) if the file was modified, Ok(false) if no changes needed.
pub fn update_claude_md(project_root: &Path) -> anyhow::Result<bool> {
    let claude_md_path = project_root.join("CLAUDE.md");
    match plan_claude_md(project_root)? {
        ClaudeMdPlan::Unchanged => Ok(false),
        ClaudeMdPlan::Create { content } | ClaudeMdPlan::Modify { new: content, .. } => {
            std::fs::write(&claude_md_path, content)?;
            Ok(true)
        }
        ClaudeMdPlan::Delete { .. } => {
            std::fs::remove_file(&claude_md_path)?;
            Ok(true)
        }
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
            section.contains(
                r#"ToolSearch(query="select:mcp__cas__task,mcp__cas__memory,mcp__cas__search")"#
            ),
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
            assert!(
                result,
                "expected block to be written when no ancestor has it"
            );
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

    /// L6 F6: a descendant block duplicating an ancestor's current block is
    /// pruned, and the rest of the file is kept.
    #[test]
    fn test_duplicate_project_block_pruned_when_ancestor_has_current_block() {
        TestEnvGuard::run_with_temp_home(|home| {
            fs::write(home.join("CLAUDE.md"), build_cas_section()).unwrap();

            let project = home.join("project");
            fs::create_dir_all(&project).unwrap();
            let project_claude = project.join("CLAUDE.md");
            fs::write(
                &project_claude,
                format!("{}\n\n# Project\n\nKeep me.\n", build_cas_section()),
            )
            .unwrap();

            assert!(update_claude_md(&project).unwrap());
            let content = fs::read_to_string(&project_claude).unwrap();
            assert_eq!(content, "# Project\n\nKeep me.\n");

            // Idempotent: a second run has nothing left to do.
            assert!(!update_claude_md(&project).unwrap());
        });
    }

    /// A git-tracked descendant CLAUDE.md keeps (a refreshed copy of) its
    /// block: other clones rely on it, and pruning would leave a local diff.
    #[test]
    fn test_tracked_project_block_refreshed_not_pruned() {
        TestEnvGuard::run_with_temp_home(|home| {
            fs::write(home.join("CLAUDE.md"), build_cas_section()).unwrap();

            let project = home.join("project");
            fs::create_dir_all(&project).unwrap();
            let stale = format!("{CAS_SECTION_BEGIN}\n# stale directive\n{CAS_SECTION_END}\n");
            fs::write(project.join("CLAUDE.md"), stale).unwrap();
            let git = |args: &[&str]| {
                let status = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&project)
                    .args(args)
                    .status()
                    .expect("run git");
                assert!(status.success(), "git {args:?} failed");
            };
            git(&["init", "-q"]);
            git(&["add", "CLAUDE.md"]);

            assert!(update_claude_md(&project).unwrap());
            let content = fs::read_to_string(project.join("CLAUDE.md")).unwrap();
            assert_eq!(content, format!("{}\n", build_cas_section()));
        });
    }

    /// A descendant CLAUDE.md holding nothing but the duplicate block is
    /// deleted rather than left empty.
    #[test]
    fn test_block_only_project_file_deleted_when_ancestor_has_current_block() {
        TestEnvGuard::run_with_temp_home(|home| {
            fs::write(home.join("CLAUDE.md"), build_cas_section()).unwrap();

            let project = home.join("project");
            fs::create_dir_all(&project).unwrap();
            let stale = format!("{CAS_SECTION_BEGIN}\n# stale directive\n{CAS_SECTION_END}\n");
            fs::write(project.join("CLAUDE.md"), stale).unwrap();

            assert!(update_claude_md(&project).unwrap());
            assert!(!project.join("CLAUDE.md").exists());
        });
    }

    /// Text around the removed block is joined with one blank line.
    #[test]
    fn test_pruned_block_between_text_joins_cleanly() {
        TestEnvGuard::run_with_temp_home(|home| {
            fs::write(home.join("CLAUDE.md"), build_cas_section()).unwrap();

            let project = home.join("project");
            fs::create_dir_all(&project).unwrap();
            fs::write(
                project.join("CLAUDE.md"),
                format!("# Top\n\n{}\n\n# Bottom\n", build_cas_section()),
            )
            .unwrap();

            assert!(update_claude_md(&project).unwrap());
            let content = fs::read_to_string(project.join("CLAUDE.md")).unwrap();
            assert_eq!(content, "# Top\n\n# Bottom\n");
        });
    }

    /// When the ancestor's block is stale, a descendant block is refreshed
    /// (not pruned, which would leave only stale text), and a descendant
    /// without a block still gets none.
    #[test]
    fn test_stale_ancestor_refreshes_existing_descendant_block() {
        TestEnvGuard::run_with_temp_home(|home| {
            let stale = format!("{CAS_SECTION_BEGIN}\n# stale directive\n{CAS_SECTION_END}\n");
            fs::write(home.join("CLAUDE.md"), &stale).unwrap();

            let project = home.join("project");
            fs::create_dir_all(&project).unwrap();
            fs::write(project.join("CLAUDE.md"), format!("{stale}\n# Project\n")).unwrap();

            assert!(update_claude_md(&project).unwrap());
            let content = fs::read_to_string(project.join("CLAUDE.md")).unwrap();
            assert!(content.contains(&build_cas_section()), "{content}");
            assert!(content.ends_with("# Project\n"), "{content}");
            assert!(!content.contains("stale directive"), "{content}");

            let bare = home.join("bare");
            fs::create_dir_all(&bare).unwrap();
            fs::write(bare.join("CLAUDE.md"), "# Bare\n").unwrap();
            assert!(!update_claude_md(&bare).unwrap());
            assert_eq!(
                fs::read_to_string(bare.join("CLAUDE.md")).unwrap(),
                "# Bare\n"
            );
        });
    }

    /// Skills audit L6 F1: the init-written `cas` skill must not recommend
    /// `start` on `task action=create` — `TaskRequest` has no such field and
    /// rejects unknown fields — and must stay a short pointer.
    #[test]
    fn cas_skill_does_not_recommend_nonexistent_start_param() {
        assert!(!CAS_SKILL.contains("`start`"), "{CAS_SKILL}");
        assert!(!CAS_SKILL.contains("RECOMMENDED"), "{CAS_SKILL}");
        assert!(is_skill_managed_by_cas(CAS_SKILL));
        for pointer in ["cas-task-tracking", "cas-memory-management", "cas-search"] {
            assert!(
                CAS_SKILL.contains(pointer),
                "cas skill must point at {pointer}"
            );
        }
        assert!(
            CAS_SKILL.lines().count() <= 20,
            "cas skill is a pointer, not a fourth manual"
        );
    }

    /// GH #963: the managed block names the current registry key only.
    #[test]
    fn template_names_violet_not_deprecated_mecha_cassy_key() {
        let section = build_cas_section();
        assert!(
            section.contains("issues.components.{cassy,violet,cloud}"),
            "{section}"
        );
        assert!(!section.contains("mecha_cassy"), "{section}");
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
