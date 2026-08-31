pub fn truncate_list(items: &[&str], max: usize) -> String {
    if items.len() <= max {
        items.join(", ")
    } else {
        let shown: Vec<_> = items.iter().take(max).copied().collect();
        format!("{}, ... (+{} more)", shown.join(", "), items.len() - max)
    }
}

/// Truncate a string to a maximum length
pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len.saturating_sub(3).min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

/// Check if a similar rule already exists using BM25 search
///
/// Returns true if a rule with high similarity score exists
pub fn find_similar_rule(cas_root: &std::path::Path, content: &str) -> bool {
    use crate::hybrid_search::{DocType, SearchIndex, SearchOptions};

    let index_dir = crate::hybrid_search::tantivy_index_dir(cas_root);
    let search = match SearchIndex::open(&index_dir) {
        Ok(s) => s,
        Err(_) => return false,
    };

    // Search for rules with similar content
    let opts = SearchOptions {
        query: content.chars().take(200).collect(), // Use first 200 chars as query
        limit: 3,
        doc_types: vec![DocType::Rule],
        ..Default::default()
    };

    match search.search_unified(&opts) {
        Ok(results) => {
            // If any result has a high BM25 score, consider it a duplicate
            // BM25 scores vary but > 5.0 typically indicates strong match
            results.iter().any(|r| r.bm25_score > 5.0)
        }
        Err(_) => false,
    }
}

/// Check if a similar memory entry already exists using BM25 search.
///
/// Returns `true` if an entry with a high similarity score is found, meaning
/// the caller should skip writing a new near-duplicate entry.  Mirrors the
/// `find_similar_rule` guard used by the `extract_learnings_sync` path.
pub fn find_similar_entry(cas_root: &std::path::Path, content: &str) -> bool {
    use crate::hybrid_search::{DocType, SearchIndex, SearchOptions};

    let index_dir = crate::hybrid_search::tantivy_index_dir(cas_root);
    let search = match SearchIndex::open(&index_dir) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let opts = SearchOptions {
        query: content.chars().take(200).collect(),
        limit: 3,
        doc_types: vec![DocType::Entry],
        ..Default::default()
    };

    match search.search_unified(&opts) {
        Ok(results) => results.iter().any(|r| r.bm25_score > 5.0),
        Err(_) => false,
    }
}

/// Check if a file path represents an architectural/important file
pub fn is_architectural_file(path: &str) -> bool {
    // Configuration files
    let config_patterns = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "tsconfig.json",
        "webpack.config",
        "vite.config",
        ".eslintrc",
        ".prettierrc",
        "Makefile",
        "CMakeLists.txt",
    ];

    // Architectural directories/patterns
    let arch_patterns = [
        "/src/main.",
        "/src/lib.",
        "/src/mod.", // Entry points
        "/src/types/",
        "/src/models/",
        "/src/schema", // Type definitions
        "/src/store/",
        "/src/db/",
        "/src/database/", // Data layer
        "/src/api/",
        "/src/routes/",
        "/src/handlers/", // API layer
        "/src/config/",
        "/src/settings/", // Configuration
        "migrations/",
        "schema.sql",
        "schema.prisma", // Database schema
    ];

    // Check config files
    for pattern in config_patterns.iter() {
        if path.ends_with(pattern) || path.contains(pattern) {
            return true;
        }
    }

    // Check architectural patterns
    for pattern in arch_patterns.iter() {
        if path.contains(pattern) {
            return true;
        }
    }

    false
}
