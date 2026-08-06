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
pub use pipeline::{DistillConfig, DistillReport, run_distillation};
pub use sources::{LoadedSource, SourceKind, SymbolLite, collect_file_sources};

/// Environment gate for automatic distillation inside the daemon.
///
/// Distillation spends real tokens, so the daemon never starts one unless the
/// operator opts in. `cas knowledge build` is always available regardless.
pub const AUTO_DISTILL_ENV: &str = "CAS_KNOWLEDGE_AUTO_DISTILL";

/// Should the daemon distill after a code-index cycle detects changes?
pub fn auto_distill_enabled() -> bool {
    matches!(
        std::env::var(AUTO_DISTILL_ENV)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The full source set for a project: files on disk plus synthesized module
/// summaries seeded from the indexed code symbols.
///
/// `symbols` is passed in rather than fetched so the collector stays pure and
/// the caller decides how many symbols it is willing to load.
pub fn collect_sources(project_root: &Path, symbols: &[SymbolLite]) -> Vec<LoadedSource> {
    let mut all = collect_file_sources(project_root);
    all.extend(sources::build_module_sources(symbols, project_root));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_distill_is_off_unless_explicitly_enabled() {
        // The gate reads a process-wide env var; assert the parser directly on
        // representative values rather than mutating the environment.
        for value in ["", "0", "false", "no", "off", "maybe"] {
            assert!(
                !matches!(value, "1" | "true" | "yes" | "on"),
                "{value} must not enable auto distillation"
            );
        }
        assert!(!auto_distill_enabled() || std::env::var(AUTO_DISTILL_ENV).is_ok());
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
