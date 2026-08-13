use crate::search::*;
use cas_types::{Entry, Task};

fn create_test_entry(id: &str, content: &str) -> Entry {
    Entry {
        id: id.to_string(),
        content: content.to_string(),
        ..Default::default()
    }
}

#[test]
fn test_index_and_search() {
    let index = SearchIndex::in_memory().unwrap();

    let entries = vec![
        create_test_entry("001", "Rust is a systems programming language"),
        create_test_entry("002", "Python is good for data science"),
        create_test_entry("003", "JavaScript runs in browsers"),
    ];

    for entry in &entries {
        index.index_entry(entry).unwrap();
    }

    let opts = SearchOptions {
        query: "programming".to_string(),
        limit: 10,
        ..Default::default()
    };

    let results = index.search(&opts, &entries).unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].id, "001");
}

#[test]
fn unknown_field_like_tokens_are_literal_terms_in_unified_search() {
    let index = SearchIndex::in_memory().unwrap();
    let entry = create_test_entry(
        "001",
        "test colon handling 12:49 https://example.invalid/path foo::bar",
    );
    index.index_entry(&entry).unwrap();

    for query in [
        "test colon handling 12:49",
        "https://example.invalid/path",
        "foo::bar",
    ] {
        let results = index
            .search_unified(&SearchOptions {
                query: query.to_string(),
                limit: 10,
                ..Default::default()
            })
            .unwrap_or_else(|error| panic!("{query:?} must parse as literal text: {error}"));
        assert!(
            results.iter().any(|result| result.id == "001"),
            "expected literal query {query:?} to match the indexed entry"
        );
    }
}

#[test]
fn unified_search_keeps_registered_fields_strict() {
    let index = SearchIndex::in_memory().unwrap();
    let entry = create_test_entry("001", "a searchable entry");
    index.index_entry(&entry).unwrap();

    let results = index
        .search_unified(&SearchOptions {
            query: "doc_type:entry".to_string(),
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "001");

    assert!(
        index
            .search_unified(&SearchOptions {
                query: "doc_type:".to_string(),
                limit: 10,
                ..Default::default()
            })
            .is_err(),
        "registered fields must retain Tantivy's strict syntax validation"
    );
}

#[test]
fn test_feedback_boost() {
    let index = SearchIndex::in_memory().unwrap();

    // Entry with low BM25 score but high feedback
    let mut entry1 = create_test_entry("001", "Rust programming");
    entry1.helpful_count = 10;

    // Entry with higher BM25 score (more matching terms) but no feedback
    let entry2 = create_test_entry("002", "Rust programming language tutorial guide");

    let entries = vec![entry1, entry2];

    for entry in &entries {
        index.index_entry(entry).unwrap();
    }

    // Without boost: entry2 should rank higher due to more content
    let opts = SearchOptions {
        query: "programming".to_string(),
        limit: 10,
        boost_feedback: false,
        ..Default::default()
    };
    let results_without = index.search(&opts, &entries).unwrap();

    // With boost: entry1's feedback should help it compete or rank higher
    let opts = SearchOptions {
        query: "programming".to_string(),
        limit: 10,
        boost_feedback: true,
        ..Default::default()
    };
    let results_with = index.search(&opts, &entries).unwrap();

    // Find positions
    let pos_without = results_without.iter().position(|r| r.id == "001").unwrap();
    let pos_with = results_with.iter().position(|r| r.id == "001").unwrap();

    // With feedback boost, entry1 should rank better (lower position = better)
    // or at least maintain position
    assert!(
        pos_with <= pos_without,
        "Feedback boost should improve ranking: pos_with={pos_with}, pos_without={pos_without}"
    );
}

#[test]
fn test_extract_id_patterns() {
    // Single ID
    let (ids, remaining) = extract_id_patterns("cas-8cb5");
    assert_eq!(ids, vec!["cas-8cb5"]);
    assert_eq!(remaining, "");

    // Multiple IDs
    let (ids, remaining) = extract_id_patterns("cas-8cb5 cas-4a23 cas-c6a3");
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&"cas-8cb5".to_string()));
    assert!(ids.contains(&"cas-4a23".to_string()));
    assert!(ids.contains(&"cas-c6a3".to_string()));
    assert_eq!(remaining, "");

    // Mixed query with IDs and text
    let (ids, remaining) = extract_id_patterns("find cas-1234 and cas-5678 about rust");
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"cas-1234".to_string()));
    assert!(ids.contains(&"cas-5678".to_string()));
    assert_eq!(remaining, "find and about rust");

    // Rule IDs
    let (ids, _remaining) = extract_id_patterns("rule-041 rule-003");
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"rule-041".to_string()));
    assert!(ids.contains(&"rule-003".to_string()));

    // No IDs
    let (ids, remaining) = extract_id_patterns("search for rust programming");
    assert!(ids.is_empty());
    assert_eq!(remaining, "search for rust programming");

    // Case insensitive
    let (ids, _) = extract_id_patterns("CAS-ABCD cas-1234");
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"cas-abcd".to_string()));
    assert!(ids.contains(&"cas-1234".to_string()));

    // Cloud-reserved proposal task IDs use exactly 16 lowercase hex digits.
    let (ids, remaining) = extract_id_patterns("open cas-0123456789abcdef now");
    assert_eq!(ids, vec!["cas-0123456789abcdef"]);
    assert_eq!(remaining, "open now");

    // The cloud form is intentionally stricter than the legacy short form.
    let (ids, remaining) =
        extract_id_patterns("cas-0123456789abcdeg cas-0123456789ABCDEF cas-0123456789abcdef0");
    assert!(ids.is_empty());
    assert_eq!(
        remaining,
        "cas-0123456789abcdeg cas-0123456789ABCDEF cas-0123456789abcdef0"
    );
}

#[test]
fn test_unified_search_with_id_patterns() {
    let index = SearchIndex::in_memory().unwrap();

    // Create test tasks
    let task1 = Task {
        id: "cas-1234".to_string(),
        title: "Test task one".to_string(),
        description: "First task description".to_string(),
        ..Default::default()
    };
    let task2 = Task {
        id: "cas-5678".to_string(),
        title: "Test task two".to_string(),
        description: "Second task description".to_string(),
        ..Default::default()
    };

    index.index_task(&task1).unwrap();
    index.index_task(&task2).unwrap();

    // Search by single ID
    let opts = SearchOptions {
        query: "cas-1234".to_string(),
        limit: 10,
        ..Default::default()
    };
    let results = index.search_unified(&opts).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "cas-1234");
    assert_eq!(results[0].score, 1.0); // Exact ID match

    // Search by multiple IDs
    let opts = SearchOptions {
        query: "cas-1234 cas-5678".to_string(),
        limit: 10,
        ..Default::default()
    };
    let results = index.search_unified(&opts).unwrap();
    assert_eq!(results.len(), 2);

    // Search by ID with doc_type filter
    let opts = SearchOptions {
        query: "cas-1234".to_string(),
        limit: 10,
        doc_types: vec![DocType::Task],
        ..Default::default()
    };
    let results = index.search_unified(&opts).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].doc_type, DocType::Task);
}
