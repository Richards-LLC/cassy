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
            ("memory leaks", vec!["memory", "rust"]),
            ("error handling API", vec!["api-errors", "api-rate"]),
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
            assert_eq!(
                results.first().expect("a matching memory").id,
                expected,
                "query {text:?}: {results:?}"
            );
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
