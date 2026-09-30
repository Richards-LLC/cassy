use super::*;

#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_exact_keyword_match_scores_high() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    // Add memories with distinct keywords
    add_memory(
        &dir,
        "Rust borrow checker ensures memory safety without garbage collection",
        Some("Rust Borrow Checker"),
        Some("rust,programming"),
    );
    add_memory(
        &dir,
        "Python is great for data science and machine learning",
        Some("Python Data Science"),
        Some("python,ml"),
    );
    add_memory(
        &dir,
        "JavaScript async/await simplifies asynchronous programming",
        Some("JS Async"),
        Some("javascript,async"),
    );

    // Wait for index
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Search for exact keyword
    let results = search_bm25(&dir, "rust borrow checker");

    // The exact match should be #1 with high score
    assert!(
        !results.is_empty(),
        "Should find results for 'rust borrow checker'"
    );

    let (_first_id, first_score) = &results[0];
    assert!(
        first_score >= &0.5,
        "Exact keyword match should score >= 0.5, got {first_score}"
    );
}

/// Test: Conceptual/semantic queries should find related content
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_conceptual_query_finds_related() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    // Add memories about a topic using different terms
    add_memory(
        &dir,
        "The ownership system in Rust prevents data races at compile time",
        Some("Rust Ownership"),
        Some("rust"),
    );
    add_memory(
        &dir,
        "Memory management in C requires manual malloc and free calls",
        Some("C Memory"),
        Some("c,memory"),
    );
    add_memory(
        &dir,
        "Garbage collection automatically reclaims unused memory in Java",
        Some("Java GC"),
        Some("java,gc"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Conceptual query - should find Rust ownership even without exact match
    let results = search_bm25(&dir, "how to avoid memory leaks");

    // Should find memory-related content
    assert!(!results.is_empty(), "Should find memory-related content");
}

/// Test: Multi-word queries should work well
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_multi_word_query() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    add_memory(
        &dir,
        "Implemented error handling with custom Result types for API endpoints",
        Some("Error Handling"),
        Some("rust,api"),
    );
    add_memory(
        &dir,
        "Added logging middleware to capture request and response data",
        Some("Logging Middleware"),
        Some("rust,middleware"),
    );
    add_memory(
        &dir,
        "The API now supports pagination for large result sets",
        Some("API Pagination"),
        Some("api"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    let results = search_bm25(&dir, "error handling API");

    assert!(
        !results.is_empty(),
        "Should find results for multi-word query"
    );

    // First result should mention both error handling and API
    let (_, score) = &results[0];
    assert!(
        score >= &0.3,
        "Multi-word match should score reasonably, got {score}"
    );
}

/// Test: Score ordering is correct (more relevant = higher score)
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_score_ordering() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    // Entry that perfectly matches
    let perfect_id = add_memory(
        &dir,
        "Database connection pooling configuration for PostgreSQL",
        Some("DB Pool Config"),
        Some("database,postgresql"),
    );

    // Entry that partially matches
    let partial_id = add_memory(
        &dir,
        "Configured the application to use environment variables",
        Some("Env Config"),
        Some("config"),
    );

    // Entry that barely matches
    let _weak_id = add_memory(
        &dir,
        "Updated the README with installation instructions",
        Some("README Update"),
        Some("docs"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    let results = search_bm25(&dir, "database configuration");

    if results.len() >= 2 {
        // Find scores for our entries
        let perfect_score = results
            .iter()
            .find(|(id, _)| id == &perfect_id)
            .map(|(_, s)| *s);
        let partial_score = results
            .iter()
            .find(|(id, _)| id == &partial_id)
            .map(|(_, s)| *s);

        if let (Some(p), Some(pa)) = (perfect_score, partial_score) {
            assert!(
                p >= pa,
                "Perfect match ({p}) should score >= partial match ({pa})"
            );
        }
    }
}

/// Test: Empty or very short queries handle gracefully
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_short_queries() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    add_memory(
        &dir,
        "The API server runs on port 8080 by default",
        Some("API Port"),
        Some("api"),
    );
    add_memory(
        &dir,
        "API rate limiting is set to 100 requests per minute",
        Some("API Rate Limit"),
        Some("api"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Single word query
    let results = search_bm25(&dir, "API");
    assert!(
        results.len() >= 2,
        "Single word 'API' should find both entries"
    );

    // All scores should be positive
    for (_, score) in &results {
        assert!(score > &0.0, "Scores should be positive");
    }
}

/// Test: Scores are in reasonable range (calibrated)
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_score_calibration() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    add_memory(
        &dir,
        "Implemented user authentication with JWT tokens",
        Some("Auth JWT"),
        Some("auth,jwt"),
    );
    add_memory(
        &dir,
        "Added OAuth2 provider support for Google and GitHub login",
        Some("OAuth Providers"),
        Some("auth,oauth"),
    );
    add_memory(
        &dir,
        "User sessions are stored in Redis with 24-hour TTL",
        Some("Sessions"),
        Some("auth,redis"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    let results = search_bm25(&dir, "authentication");

    for (_, score) in &results {
        // Scores should be in 0-1 range after calibration
        assert!(
            (&0.0..=&1.0).contains(&score),
            "Score {score} should be in [0, 1]"
        );
    }

    if !results.is_empty() {
        // Top result should have decent score for a relevant query
        let (_, top_score) = &results[0];
        assert!(
            top_score >= &0.3,
            "Top result for relevant query should score >= 0.3, got {top_score}"
        );
    }
}

/// Test: Technical queries with special characters
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_technical_queries() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    add_memory(
        &dir,
        "Fixed error E0382: borrow of moved value in the parser module",
        Some("E0382 Fix"),
        Some("rust,error"),
    );
    add_memory(
        &dir,
        "The endpoint /api/v1/users returns user list in JSON",
        Some("API Endpoint"),
        Some("api"),
    );
    add_memory(
        &dir,
        "Using src/lib.rs as the library entry point",
        Some("Lib Entry"),
        Some("rust"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Search for error code
    let results = search_bm25(&dir, "E0382");
    assert!(!results.is_empty(), "Should find error code reference");

    // Search for path
    let results = search_bm25(&dir, "/api/v1/users");
    assert!(!results.is_empty(), "Should find API path");
}

/// Test: Queries with common stop words
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_stop_words() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    add_memory(
        &dir,
        "The quick brown fox jumps over the lazy dog",
        Some("Pangram"),
        Some("test"),
    );
    add_memory(
        &dir,
        "A fast red fox leaps across a sleepy hound",
        Some("Similar"),
        Some("test"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Query with stop words should still find relevant content
    let results = search_bm25(&dir, "the fox and the dog");
    assert!(
        !results.is_empty(),
        "Should find content despite stop words"
    );
}

/// Test: No false positives for unrelated content
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_no_false_positives() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    add_memory(
        &dir,
        "Implemented dark mode toggle in the settings page",
        Some("Dark Mode"),
        Some("ui"),
    );
    add_memory(
        &dir,
        "Fixed CSS flexbox layout issues on mobile",
        Some("CSS Fix"),
        Some("css"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Query for completely unrelated topic
    let results = search_bm25(&dir, "quantum computing algorithms");

    // Should either return no results or very low scores
    if !results.is_empty() {
        let (_, top_score) = &results[0];
        assert!(
            top_score < &0.3,
            "Unrelated query should score low, got {top_score}"
        );
    }
}

/// Test: Recent vs old memories with temporal queries
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_temporal_awareness() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    // Add memories (newer ones will have more recent timestamps)
    add_memory(
        &dir,
        "Old project setup from last year",
        Some("Old Setup"),
        Some("setup"),
    );
    std::thread::sleep(std::time::Duration::from_millis(50));
    add_memory(
        &dir,
        "Recent project configuration changes",
        Some("Recent Config"),
        Some("config"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // This is more of a smoke test - temporal scoring depends on actual time differences
    let results = search_bm25(&dir, "project setup configuration");
    assert!(!results.is_empty(), "Should find project-related content");
}

/// Test: Tags don't interfere with content search
#[test]
#[ignore = "add/search CLI commands removed - tests need MCP fixtures"]
fn test_tags_dont_dominate() {
    let dir = TempDir::new().unwrap();
    init_cas(&dir);

    // Entry with rust in tags but not content
    add_memory(
        &dir,
        "Implemented a web server using Actix framework",
        Some("Web Server"),
        Some("rust,web"),
    );

    // Entry with rust in content
    add_memory(
        &dir,
        "Rust provides zero-cost abstractions for systems programming",
        Some("Rust Info"),
        Some("programming"),
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    let results = search_bm25(&dir, "rust programming");

    // Entry with "rust" in content should score higher
    if results.len() >= 2 {
        // The Rust Info entry should rank well since it has "Rust" in content
        let content_match = results.iter().any(|(id, _)| id.contains("Rust"));
        assert!(
            content_match || !results.is_empty(),
            "Should find content with 'rust' keyword"
        );
    }
}

// Current-seam replacements for the unmatched lexical/ranking contracts.
mod current {
    use cas::hybrid_search::{SearchIndex, SearchOptions, SearchResult};
    use cas::types::Entry;

    fn memory(id: &str, content: &str, tags: &[&str]) -> Entry {
        Entry {
            id: id.to_string(),
            content: content.to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            ..Default::default()
        }
    }

    fn query(entries: &[Entry], text: &str) -> Vec<SearchResult> {
        let index = SearchIndex::in_memory().unwrap();
        index.index_entries_batch(entries).unwrap();
        index
            .search(
                &SearchOptions {
                    query: text.to_string(),
                    limit: 10,
                    ..Default::default()
                },
                entries,
            )
            .unwrap_or_else(|error| panic!("query {text:?}: {error}"))
    }

    #[test]
    fn lexical_queries_require_expected_ids_and_calibrated_scores() {
        let mut old = memory("old-project", "Old project setup", &[]);
        old.created = "2020-01-01T00:00:00Z".parse().unwrap();
        let mut recent = memory("recent-project", "Recent project configuration", &[]);
        recent.created = "2025-01-01T00:00:00Z".parse().unwrap();
        let entries = vec![
            memory(
                "rust",
                "Rust borrow checker ensures memory safety",
                &["programming"],
            ),
            memory(
                "memory",
                "Manual memory management prevents memory leaks",
                &[],
            ),
            memory(
                "api-errors",
                "API error handling uses custom Result types",
                &[],
            ),
            memory(
                "api-rate",
                "API rate limiting allows 100 requests per minute",
                &[],
            ),
            old,
            recent,
        ];
        for (text, expected_ids) in [
            ("rust borrow checker", vec!["rust"]),
            ("memory leaks", vec!["memory"]),
            ("error handling API", vec!["api-errors"]),
            ("API", vec!["api-errors", "api-rate"]),
            ("project", vec!["old-project", "recent-project"]),
        ] {
            let results = query(&entries, text);
            let mut ids: Vec<_> = results.iter().map(|result| result.id.as_str()).collect();
            ids.sort_unstable();
            let mut expected_ids = expected_ids;
            expected_ids.sort_unstable();
            assert_eq!(ids, expected_ids, "query {text:?}");
            assert!(
                results.iter().all(|result| result.score.is_finite()
                    && result.score > 0.0
                    && result.score <= 1.0),
                "query {text:?}: {results:?}"
            );
            assert!(
                results
                    .windows(2)
                    .all(|pair| pair[0].score >= pair[1].score),
                "query {text:?}: {results:?}"
            );
        }
    }

    #[test]
    fn higher_term_frequency_orders_the_more_relevant_memory_first() {
        let entries = vec![
            memory("strong", "database database database configuration", &[]),
            memory("partial", "database configuration reference guide", &[]),
        ];
        let results = query(&entries, "database");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "strong");
        assert_eq!(results[1].id, "partial");
        assert!(results[0].bm25_score > results[1].bm25_score);
        assert!(results[0].score > results[1].score);
    }

    #[test]
    fn technical_and_stopword_queries_retrieve_the_named_memory() {
        let entries = vec![
            memory("error", "Fixed error E0382: borrow of moved value", &[]),
            memory("endpoint", "The endpoint /api/v1/users returns JSON", &[]),
            memory("fox", "The quick brown fox jumps over the lazy dog", &[]),
        ];
        for (text, expected) in [
            ("E0382", "error"),
            ("/api/v1/users", "endpoint"),
            ("the fox and the dog", "fox"),
        ] {
            let results = query(&entries, text);
            assert_eq!(results.len(), 1, "query {text:?}: {results:?}");
            assert_eq!(results[0].id, expected);
        }
    }

    #[test]
    fn unrelated_and_empty_queries_return_no_memories() {
        let entries = vec![memory(
            "ui",
            "Dark mode toggle and CSS layout",
            &["ui", "css"],
        )];
        for text in ["quantum computing algorithms", "", "   "] {
            assert!(query(&entries, text).is_empty(), "query {text:?}");
        }
    }

    #[test]
    fn content_matches_remain_retrievable_beside_tag_only_matches() {
        let entries = vec![
            memory("tag-only", "Actix web server", &["rust"]),
            memory(
                "content",
                "Rust provides programming abstractions",
                &["programming"],
            ),
        ];
        let results = query(&entries, "rust programming");
        assert!(
            results.iter().any(|result| result.id == "content"),
            "{results:?}"
        );
        assert_eq!(
            results[0].id, "content",
            "content matching both terms must rank first"
        );
    }
}
