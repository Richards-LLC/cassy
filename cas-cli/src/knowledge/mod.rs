//! Project knowledge distillation (EPIC cas-7d31, T2 = cas-c9be).
//!
//! T1 ([`cas_store::KnowledgeStore`]) owns durability. This module owns the
//! *pass*: which sources are worth distilling, how they are chunked, the
//! two-stage prompts and their role-isolation armor, the cost-tiered merge into
//! canonical paths, and the provenance/wikilink repair that follows a deletion.
//!
//! Entry points:
//! - [`collect_sources`] — the source set for a project (docs, key configs and
//!   code-derived module summaries).
//! - [`run_distillation`] — one pass. Zero LLM calls when nothing moved.

pub mod chunk;
pub mod llm;
pub mod merge;
pub mod pipeline;
pub mod prompt;
pub mod sources;

use std::path::Path;

pub use chunk::{Chunk, ChunkOptions, chunk_markdown};
pub use llm::{ClaudeCliRunner, LlmError, LlmRunner, ScriptedLlm};
pub use merge::{MergeTier, StripOutcome};
pub use pipeline::{DistillConfig, DistillReport, run_distillation, run_distillation_with_timeout};
pub(crate) use pipeline::run_distillation_until;
pub use sources::{
    LoadedSource, SkippedSource, SourceKind, SourceScan, SymbolLite, collect_file_sources,
    scan_file_sources,
};

/// Environment gate for automatic distillation inside the daemon.
///
/// Distillation spends real tokens, so the daemon never starts one unless the
/// operator opts in. `cas knowledge build` is always available regardless.
pub const AUTO_DISTILL_ENV: &str = "CAS_KNOWLEDGE_AUTO_DISTILL";

/// Should the daemon distill after a code-index cycle detects changes?
pub fn auto_distill_enabled() -> bool {
    parse_auto_distill(&std::env::var(AUTO_DISTILL_ENV).unwrap_or_default())
}

/// The opt-in rule itself, split out so it can be tested without mutating the
/// process environment. Anything that is not an explicit yes is a no — the
/// default must never be "spend tokens in the background".
pub fn parse_auto_distill(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The full source set for a project: files on disk plus synthesized module
/// summaries seeded from the indexed code symbols.
///
/// `symbols` is passed in rather than fetched so the collector stays pure and
/// the caller decides how many symbols it is willing to load.
pub fn collect_sources(project_root: &Path, symbols: &[SymbolLite]) -> Vec<LoadedSource> {
    scan_sources(project_root, symbols).sources
}

/// [`collect_sources`] plus the files it could not decode, named.
///
/// Callers that print or log a pass report should use this one: a source that
/// silently drops out produces a wiki with a hole in it and no explanation
/// (cas-c736). Synthesized `code://` module sources can never be skipped —
/// they are built from already-indexed symbols, not re-read from disk — so the
/// skip list is exactly the file walk's.
pub fn scan_sources(project_root: &Path, symbols: &[SymbolLite]) -> SourceScan {
    let mut scan = scan_file_sources(project_root);
    scan.sources
        .extend(sources::build_module_sources(symbols, project_root));
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_distill_is_off_unless_explicitly_enabled() {
        for value in ["1", "true", "TRUE", " on ", "yes", "Yes"] {
            assert!(parse_auto_distill(value), "{value:?} must opt in");
        }
        for value in ["", "   ", "0", "false", "no", "off", "maybe", "onx", "1x", "enable"] {
            assert!(
                !parse_auto_distill(value),
                "{value:?} must NOT start a background pass that spends tokens"
            );
        }
    }

    #[test]
    fn collect_sources_merges_files_and_modules() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("README.md"), "# Hi").unwrap();
        let symbols = vec![SymbolLite {
            file_path: dir.path().join("src/lib.rs").to_string_lossy().to_string(),
            name: "run".to_string(),
            kind: "function".to_string(),
            signature: None,
            doc: None,
        }];
        let sources = collect_sources(dir.path(), &symbols);
        let paths: Vec<&str> = sources.iter().map(|s| s.path.as_str()).collect();
        assert!(paths.contains(&"README.md"));
        assert!(paths.iter().any(|p| p.starts_with("code://src")));
    }
}
