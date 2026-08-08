//! Semantic source-code vector drain.
//!
//! Symbols are queued by the structural indexer and embedded into the
//! dedicated `index/code-vectors` LMDB environment. Nothing in this module
//! writes the knowledge cache or the unified memory/task index.

use std::collections::HashMap;
use std::path::Path;

use cas_code::CodeSymbol;
use cas_store::{CodeStore, SqliteCodeStore, SqliteCodeVectorStore};

use crate::cloud::embeddings::{
    EmbedReport, EmbedUnit, KnowledgeEmbedder, KnowledgeVectorCache, RateLimiter, code_symbol_key,
    drain_units,
};
use crate::error::CasError;

/// Stable semantic representation of a symbol. Structural identity is placed
/// first so exact names and paths remain present in the vector text, followed
/// by documentation/signature and bounded source context for paraphrases.
pub fn code_embedding_text(symbol: &CodeSymbol) -> String {
    let mut parts = vec![
        format!("{} {}", symbol.kind, symbol.qualified_name),
        format!("file: {}", symbol.file_path),
    ];
    if let Some(documentation) = symbol
        .documentation
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        parts.push(documentation.to_string());
    }
    if let Some(signature) = symbol
        .signature
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        parts.push(signature.to_string());
    }
    let source = symbol.source.trim();
    if !source.is_empty() {
        let mut end = source.len().min(2_000);
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        parts.push(source[..end].to_string());
    }
    parts.join("\n\n")
}

pub fn embed_pending_code(
    cas_root: &Path,
    embedder: &KnowledgeEmbedder,
    limiter: &RateLimiter,
    limit: usize,
) -> Result<EmbedReport, CasError> {
    let state = SqliteCodeVectorStore::open(cas_root)
        .map_err(|e| CasError::Other(format!("Failed to open code vector queue: {e}")))?;
    let cache = KnowledgeVectorCache::open_code(cas_root, embedder.meta())?;
    let mut report = EmbedReport {
        reindexed: cache.reindexed(),
        ..Default::default()
    };
    if cache.reindexed() {
        state
            .mark_all_pending()
            .map_err(|e| CasError::Other(format!("Failed to re-arm code vectors: {e}")))?;
    }

    let work = state
        .list_pending(limit)
        .map_err(|e| CasError::Other(format!("Failed to list pending code vectors: {e}")))?;
    if work.is_empty() {
        let stats = state
            .stats()
            .map_err(|e| CasError::Other(format!("Failed to count code vectors: {e}")))?;
        report.pending_after = stats.pending + stats.failed;
        return Ok(report);
    }

    let code = SqliteCodeStore::open(cas_root)
        .map_err(|e| CasError::Other(format!("Failed to open code store: {e}")))?;
    let ids: Vec<&str> = work.iter().map(|item| item.symbol_id.as_str()).collect();
    let symbols = code
        .get_symbols_batch(&ids)
        .map_err(|e| CasError::Other(format!("Failed to load code symbols: {e}")))?;
    let symbol_map: HashMap<&str, &CodeSymbol> = symbols
        .iter()
        .map(|symbol| (symbol.id.as_str(), symbol))
        .collect();
    let hashes: HashMap<&str, &str> = work
        .iter()
        .map(|item| (item.symbol_id.as_str(), item.content_hash.as_str()))
        .collect();

    let mut missing = Vec::new();
    let mut units = Vec::new();
    for item in &work {
        let Some(symbol) = symbol_map.get(item.symbol_id.as_str()) else {
            missing.push(item.symbol_id.clone());
            continue;
        };
        // A newer parse may have landed between listing and fetching. Leave
        // that row pending; marking the stale hash can never complete it.
        if symbol.content_hash != item.content_hash {
            continue;
        }
        units.push(EmbedUnit::new(
            code_symbol_key(&item.symbol_id),
            item.symbol_id.clone(),
            code_embedding_text(symbol),
        ));
    }
    state
        .retire(&missing)
        .map_err(|e| CasError::Other(format!("Failed to retire missing code symbols: {e}")))?;

    {
        let mut mark = |id: &str| {
            let hash = hashes
                .get(id)
                .ok_or_else(|| format!("missing queued content hash for {id}"))?;
            state
                .mark_vectorized(id, hash)
                .map_err(|e| e.to_string())
                .and_then(|updated| {
                    updated
                        .then_some(())
                        .ok_or_else(|| format!("symbol {id} changed while embedding"))
                })
        };
        drain_units(embedder, &cache, &units, limiter, &mut mark, &mut report);
    }

    // Failed is a durable, visible state, but remains retryable because
    // `list_pending` intentionally includes it on the next tick.
    if report.had_trouble() || report.capability_absent {
        let message =
            report.request_errors.first().cloned().unwrap_or_else(|| {
                "embedding provider returned an unusable code vector".to_string()
            });
        for unit in &units {
            if cache.get(&unit.key).ok().flatten().is_none() {
                if let Some(hash) = hashes.get(unit.id.as_str()) {
                    let _ = state.mark_failed(&unit.id, hash, &message);
                }
            }
        }
    }

    let stats = state
        .stats()
        .map_err(|e| CasError::Other(format!("Failed to count code vectors: {e}")))?;
    report.pending_after = stats.pending + stats.failed;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_code::{Language, SymbolKind};
    use chrono::Utc;

    #[test]
    fn embedding_text_carries_identity_docs_signature_and_source() {
        let symbol = CodeSymbol {
            id: "sym-1".into(),
            qualified_name: "cache::evict_stale".into(),
            name: "evict_stale".into(),
            kind: SymbolKind::Function,
            language: Language::Rust,
            file_path: "src/cache.rs".into(),
            file_id: "file-1".into(),
            line_start: 1,
            line_end: 3,
            source: "fn evict_stale() { remove_expired(); }".into(),
            documentation: Some("Drops entries after their TTL expires.".into()),
            signature: Some("fn evict_stale()".into()),
            parent_id: None,
            repository: "repo".into(),
            commit_hash: None,
            created: Utc::now(),
            updated: Utc::now(),
            content_hash: "h".into(),
            scope: "project".into(),
        };
        let text = code_embedding_text(&symbol);
        assert!(text.contains("cache::evict_stale"));
        assert!(text.contains("src/cache.rs"));
        assert!(text.contains("TTL expires"));
        assert!(text.contains("remove_expired"));
    }
}
