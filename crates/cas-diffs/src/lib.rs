//! Diff parsing, rendering, and syntax highlighting for CAS.
//!
//! This crate provides core data types and a parser for representing
//! unified diffs, ported from the `@pierre/diffs` TypeScript library.
//! Types model parsed patch files at the file, hunk, and line level.

pub mod highlight;
pub mod iter;
pub mod parser;

pub mod diff_files;
pub mod inline_diff;
pub mod widget;

pub use diff_files::diff_files;
pub use inline_diff::{InlineSpan, compute_inline_diff};

use serde::{Deserialize, Serialize};

/// Describes the type of change for a file in a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeType {
    /// File content was modified, name unchanged.
    Change,
    /// File was renamed/moved without content changes (100% similarity).
    RenamePure,
    /// File was renamed/moved and content was also modified.
    RenameChanged,
    /// A new file was added.
    New,
    /// An existing file was removed.
    Deleted,
}

/// Line types parsed from a patch file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HunkLineType {
    /// Unchanged context line (prefixed with space).
    Context,
    /// Expanded context line (loaded on demand).
    Expanded,
    /// Added line (prefixed with `+`).
    Addition,
    /// Removed line (prefixed with `-`).
    Deletion,
    /// Metadata line (e.g. `\ No newline at end of file`).
    Metadata,
}

/// Intra-line diff algorithm to use for highlighting changes within a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineDiffType {
    /// Word-level diff that joins regions separated by a single character.
    WordAlt,
    /// Standard word-level diff.
    Word,
    /// Character-level diff.
    Char,
    /// No intra-line diffing.
    None,
}

/// A block of unchanged context lines within a hunk.
///
/// Consecutive lines prefixed with a space are grouped into a single
/// `ContextContent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextContent {
    /// Number of unchanged lines in this context block.
    pub lines: usize,
    /// Zero-based index into `FileDiffMetadata::addition_lines` where this
    /// context block starts.
    pub addition_line_index: usize,
    /// Zero-based index into `FileDiffMetadata::deletion_lines` where this
    /// context block starts.
    pub deletion_line_index: usize,
}

/// A block of changes (additions and/or deletions) within a hunk.
///
/// Consecutive `+` and `-` lines are grouped into a single `ChangeContent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeContent {
    /// Number of deleted lines (prefixed with `-`) in this change block.
    pub deletions: usize,
    /// Zero-based index into `FileDiffMetadata::deletion_lines` where the
    /// deleted lines start.
    pub deletion_line_index: usize,
    /// Number of added lines (prefixed with `+`) in this change block.
    pub additions: usize,
    /// Zero-based index into `FileDiffMetadata::addition_lines` where the
    /// added lines start.
    pub addition_line_index: usize,
}

/// A content segment within a hunk — either context or changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HunkContent {
    /// Unchanged context lines.
    Context(ContextContent),
    /// Changed lines (additions and/or deletions).
    Change(ChangeContent),
}

/// A single hunk from a diff, corresponding to one `@@ ... @@` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// Number of unchanged lines between the previous hunk (or file start)
    /// and this hunk.
    pub collapsed_before: usize,

    /// Starting line number in the new file version (from `+X` in hunk header).
    pub addition_start: usize,
    /// Total line count in the new file version for this hunk (from `+X,count`).
    /// Includes both context lines and lines prefixed with `+`.
    pub addition_count: usize,
    /// Number of lines prefixed with `+` in this hunk.
    pub addition_lines: usize,
    /// Zero-based index into `FileDiffMetadata::addition_lines` where this
    /// hunk's content starts.
    pub addition_line_index: usize,

    /// Starting line number in the old file version (from `-X` in hunk header).
    pub deletion_start: usize,
    /// Total line count in the old file version for this hunk (from `-X,count`).
    /// Includes both context lines and lines prefixed with `-`.
    pub deletion_count: usize,
    /// Number of lines prefixed with `-` in this hunk.
    pub deletion_lines: usize,
    /// Zero-based index into `FileDiffMetadata::deletion_lines` where this
    /// hunk's content starts.
    pub deletion_line_index: usize,

    /// Content segments within this hunk (context and change blocks).
    pub hunk_content: Vec<HunkContent>,
    /// Function/method name after the `@@` markers, if present.
    pub hunk_context: Option<String>,
    /// Raw hunk header string (e.g. `@@ -1,5 +1,7 @@`).
    pub hunk_specs: Option<String>,

    /// Starting line index for this hunk in split (side-by-side) view.
    pub split_line_start: usize,
    /// Total rendered line count for this hunk in split view.
    pub split_line_count: usize,

    /// Starting line index for this hunk in unified view.
    pub unified_line_start: usize,
    /// Total rendered line count for this hunk in unified view.
    pub unified_line_count: usize,

    /// True if the old file version has no trailing newline at EOF.
    pub no_eof_cr_deletions: bool,
    /// True if the new file version has no trailing newline at EOF.
    pub no_eof_cr_additions: bool,
}

/// Metadata and content for a single file's diff.
///
/// A JSON-compatible representation of a diff for a single file, including
/// all hunks, line arrays, and rendering metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiffMetadata {
    /// The file's name and path.
    pub name: String,
    /// Previous file path, present only if file was renamed or moved.
    pub prev_name: Option<String>,

    /// The type of change for this file.
    pub change_type: ChangeType,

    /// Diff hunks containing line-level change information. Each hunk
    /// corresponds to a `@@ -X,X +X,X @@` group in a diff.
    pub hunks: Vec<Hunk>,
    /// Pre-computed line count for this diff rendered in split style.
    pub split_line_count: usize,
    /// Pre-computed line count for this diff rendered in unified style.
    pub unified_line_count: usize,

    /// Whether the diff was parsed from a patch file (`true`) or generated
    /// from full file contents (`false`).
    ///
    /// When `true`, `deletion_lines`/`addition_lines` contain only the lines
    /// present in the patch and hunk expansion is unavailable.
    pub is_partial: bool,

    /// Lines from the previous version of the file. If `is_partial` is
    /// `false`, this is the entire old file. Otherwise, only lines from
    /// context and deletion regions of the patch.
    pub deletion_lines: Vec<String>,
    /// Lines from the new version of the file. If `is_partial` is `false`,
    /// this is the entire new file. Otherwise, only lines from context and
    /// addition regions of the patch.
    pub addition_lines: Vec<String>,
}

/// A parsed patch file, typically corresponding to a single commit.
///
/// Returned when parsing raw patch/diff strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedPatch {
    /// Optional raw introductory text before the file diffs (e.g. commit
    /// message, author, date).
    pub patch_metadata: Option<String>,
    /// File changes contained in the patch.
    pub files: Vec<FileDiffMetadata>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_type_variants() {
        for (variant, spelling) in [
            (ChangeType::Change, "change"),
            (ChangeType::RenamePure, "rename-pure"),
            (ChangeType::RenameChanged, "rename-changed"),
            (ChangeType::New, "new"),
            (ChangeType::Deleted, "deleted"),
        ] {
            let fixture = serde_json::Value::String(spelling.into());
            assert_eq!(serde_json::to_value(variant).unwrap(), fixture);
            assert_eq!(
                serde_json::from_value::<ChangeType>(fixture).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn hunk_line_type_serde() {
        assert_eq!(
            serde_json::to_string(&HunkLineType::Context).unwrap(),
            "\"context\""
        );
        assert_eq!(
            serde_json::to_string(&HunkLineType::Addition).unwrap(),
            "\"addition\""
        );
    }

    #[test]
    fn line_diff_type_serde() {
        assert_eq!(
            serde_json::to_string(&LineDiffType::WordAlt).unwrap(),
            "\"word-alt\""
        );
        assert_eq!(
            serde_json::to_string(&LineDiffType::None).unwrap(),
            "\"none\""
        );
    }

    #[test]
    fn hunk_content_tagged_serde() {
        let ctx = HunkContent::Context(ContextContent {
            lines: 1,
            addition_line_index: 0,
            deletion_line_index: 0,
        });
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("\"type\":\"context\""));

        let change = HunkContent::Change(ChangeContent {
            deletions: 1,
            deletion_line_index: 0,
            additions: 1,
            addition_line_index: 0,
        });
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("\"type\":\"change\""));
    }


}
