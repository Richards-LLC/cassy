//! Generate Codex's `AGENTS.md` from a project's `CLAUDE.md` files.
//!
//! `CLAUDE.md` remains the source of truth. This module contains no CLI
//! dependencies so its transformation and filesystem behavior can be tested
//! independently from command parsing.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::CoreError;

/// Prefix placed at the start of every generated file.
pub const GENERATED_HEADER: &str =
    "<!-- Auto-generated from CLAUDE.md by `cas sync agents-md`. Do not edit directly. -->\n";

/// Whether synchronization should update files or only report stale output.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AgentsMdSyncMode {
    Check,
    Write,
}

/// One generated file considered by an agents-md synchronization run.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AgentsMdFileReport {
    pub source: PathBuf,
    pub output: PathBuf,
    pub changed: bool,
}

/// Aggregate result of an agents-md synchronization run.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct AgentsMdSyncReport {
    pub files: Vec<AgentsMdFileReport>,
}

impl AgentsMdSyncReport {
    /// Files whose current AGENTS.md did not match the generated output.
    pub fn stale_files(&self) -> impl Iterator<Item = &AgentsMdFileReport> {
        self.files.iter().filter(|file| file.changed)
    }

    pub fn stale_count(&self) -> usize {
        self.stale_files().count()
    }
}

/// Transform CLAUDE.md source into its generated AGENTS.md representation.
pub fn transform_agents_md(source: &str) -> String {
    let source = source.replace("mcp__cas__", "mcp__cs__");
    let body = transform_markers(&source);
    format!("{GENERATED_HEADER}{body}")
}

/// Synchronize every CLAUDE.md below `project_root` with its adjacent AGENTS.md.
///
/// Build products and VCS metadata are ignored so a project checkout is the
/// source boundary, rather than every nested dependency checkout it contains.
pub fn sync_agents_md(
    project_root: &Path,
    mode: AgentsMdSyncMode,
) -> Result<AgentsMdSyncReport, CoreError> {
    let mut sources = Vec::new();
    discover_claude_md(project_root, &mut sources)?;
    sources.sort();

    let mut files = Vec::with_capacity(sources.len());
    for source in sources {
        let generated = transform_agents_md(&fs::read_to_string(&source)?);
        let output = source.with_file_name("AGENTS.md");
        let current = fs::read_to_string(&output).ok();
        let changed = current.as_deref() != Some(generated.as_str());

        if changed && mode == AgentsMdSyncMode::Write {
            fs::write(&output, generated)?;
        }

        files.push(AgentsMdFileReport {
            source,
            output,
            changed,
        });
    }

    Ok(AgentsMdSyncReport { files })
}

fn discover_claude_md(dir: &Path, sources: &mut Vec<PathBuf>) -> Result<(), CoreError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if !ignored_directory(entry.file_name().as_ref()) {
                discover_claude_md(&path, sources)?;
            }
        } else if file_type.is_file() && entry.file_name() == "CLAUDE.md" {
            sources.push(path);
        }
    }
    Ok(())
}

fn ignored_directory(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".git" | ".cas" | "target" | "node_modules")
    )
}

fn transform_markers(source: &str) -> String {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Mode {
        Normal,
        ClaudeOnly,
        CodexOnly,
        CodexComment,
    }

    let mut output = String::with_capacity(source.len());
    let mut mode = Mode::Normal;

    for line in source.split_inclusive('\n') {
        let trimmed = line.trim();
        match mode {
            Mode::Normal if trimmed == "<!-- claude-only:start -->" => mode = Mode::ClaudeOnly,
            Mode::Normal if trimmed == "<!-- codex-only:start -->" => mode = Mode::CodexOnly,
            Mode::Normal if trimmed == "<!-- codex-only:start" => mode = Mode::CodexComment,
            Mode::Normal => output.push_str(line),
            Mode::ClaudeOnly if trimmed == "<!-- claude-only:end -->" => mode = Mode::Normal,
            Mode::ClaudeOnly => {}
            Mode::CodexOnly if trimmed == "<!-- codex-only:end -->" => mode = Mode::Normal,
            Mode::CodexOnly if trimmed == "<!--" || trimmed == "-->" => {}
            Mode::CodexOnly => output.push_str(line),
            Mode::CodexComment if trimmed == "codex-only:end -->" => mode = Mode::Normal,
            Mode::CodexComment => output.push_str(line),
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn identity_adds_only_header() {
        assert_eq!(
            transform_agents_md("# Project\nUse local tools.\n"),
            format!("{GENERATED_HEADER}# Project\nUse local tools.\n")
        );
    }

    #[test]
    fn swaps_cas_mcp_prefixes() {
        let generated = transform_agents_md("mcp__cas__task action=start\n");
        assert!(generated.contains("mcp__cs__task action=start"));
        assert!(!generated.contains("mcp__cas__"));
    }

    #[test]
    fn strips_claude_only_blocks() {
        let generated = transform_agents_md(
            "before\n<!-- claude-only:start -->\nclaude secret\n<!-- claude-only:end -->\nafter\n",
        );
        assert!(generated.contains("before\n"));
        assert!(generated.contains("after\n"));
        assert!(!generated.contains("claude secret"));
    }

    #[test]
    fn includes_codex_only_blocks_without_markers() {
        let generated = transform_agents_md(
            "before\n<!-- codex-only:start -->\ncodex guidance\n<!-- codex-only:end -->\nafter\n",
        );
        assert!(generated.contains("codex guidance\n"));
        assert!(!generated.contains("codex-only"));
    }

    #[test]
    fn uncomments_fully_commented_codex_only_blocks() {
        let generated =
            transform_agents_md("<!-- codex-only:start\ncodex guidance\ncodex-only:end -->\n");
        assert!(generated.contains("codex guidance\n"));
        assert!(!generated.contains("<!--"));
    }

    #[test]
    fn write_is_idempotent_and_check_reports_staleness() {
        let project = tempdir().unwrap();
        let source = project.path().join("CLAUDE.md");
        fs::write(&source, "mcp__cas__task\n").unwrap();

        let first = sync_agents_md(project.path(), AgentsMdSyncMode::Write).unwrap();
        assert_eq!(first.stale_count(), 1);
        let second = sync_agents_md(project.path(), AgentsMdSyncMode::Write).unwrap();
        assert_eq!(second.stale_count(), 0);

        fs::write(&source, "mcp__cas__task updated\n").unwrap();
        let check = sync_agents_md(project.path(), AgentsMdSyncMode::Check).unwrap();
        assert_eq!(check.stale_count(), 1);
        assert!(check.files[0].output.exists());
    }
}
