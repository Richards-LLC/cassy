use crate::hybrid_search::DocType;
use crate::hybrid_search::cache::*;

#[test]
fn test_lru_cache_basic() {
    let mut cache: LruCache<String, i32> = LruCache::new(3, Duration::from_secs(60));

    cache.insert("a".to_string(), 1);
    cache.insert("b".to_string(), 2);
    cache.insert("c".to_string(), 3);

    assert_eq!(cache.get(&"a".to_string()), Some(1));
    assert_eq!(cache.get(&"b".to_string()), Some(2));
    assert_eq!(cache.get(&"c".to_string()), Some(3));
}

#[test]
fn test_lru_cache_eviction() {
    let mut cache: LruCache<String, i32> = LruCache::new(2, Duration::from_secs(60));

    cache.insert("a".to_string(), 1);
    cache.insert("b".to_string(), 2);
    // This should evict "a" (oldest)
    cache.insert("c".to_string(), 3);

    assert_eq!(cache.get(&"a".to_string()), None);
    assert_eq!(cache.get(&"b".to_string()), Some(2));
    assert_eq!(cache.get(&"c".to_string()), Some(3));
}

#[test]
fn test_lru_cache_access_updates_order() {
    let mut cache: LruCache<String, i32> = LruCache::new(2, Duration::from_secs(60));

    cache.insert("a".to_string(), 1);
    cache.insert("b".to_string(), 2);

    // Access "a" to make it recently used
    cache.get(&"a".to_string());

    // Insert "c" - should evict "b" (now oldest)
    cache.insert("c".to_string(), 3);

    assert_eq!(cache.get(&"a".to_string()), Some(1));
    assert_eq!(cache.get(&"b".to_string()), None);
    assert_eq!(cache.get(&"c".to_string()), Some(3));
}

#[test]
fn test_lru_cache_ttl_expiration() {
    let mut cache: LruCache<String, i32> = LruCache::new(10, Duration::from_millis(10));

    cache.insert("a".to_string(), 1);

    // Should be present immediately
    assert_eq!(cache.get(&"a".to_string()), Some(1));

    // Wait for expiration
    std::thread::sleep(Duration::from_millis(20));

    // Should be gone after TTL
    assert_eq!(cache.get(&"a".to_string()), None);
}

#[test]
fn test_lru_cache_invalidate_by_dependency() {
    let mut cache: LruCache<String, Vec<String>> = LruCache::new(10, Duration::from_secs(60));

    // Insert entries with dependencies
    cache.insert_with_deps(
        "query1".to_string(),
        vec!["result1".to_string()],
        vec!["entry-a".to_string(), "entry-b".to_string()],
    );
    cache.insert_with_deps(
        "query2".to_string(),
        vec!["result2".to_string()],
        vec!["entry-c".to_string()],
    );
    cache.insert_with_deps(
        "query3".to_string(),
        vec!["result3".to_string()],
        vec!["entry-a".to_string()],
    );

    // Invalidate entry-a - should remove query1 and query3
    cache.invalidate_by_dependency("entry-a");

    assert_eq!(cache.get(&"query1".to_string()), None);
    assert!(cache.get(&"query2".to_string()).is_some());
    assert_eq!(cache.get(&"query3".to_string()), None);
}

#[test]
fn test_hybrid_results_cache() {
    let cache = SearchCache::new();

    let results = vec![HybridSearchResult {
        id: "entry-1".to_string(),
        score: 0.9,
        bm25_score: 0.8,
        semantic_score: 0.95,
        temporal_score: 0.7,
        graph_score: 0.6,
        code_score: 0.0,
        rerank_score: None,
        doc_type: DocType::Entry,
        knowledge_score: 0.0,
        history_score: 0.0,
    }];

    let options_hash = 12345u64;

    // Cache miss
    assert!(cache.get_hybrid_results(options_hash).is_none());

    // Cache insert
    cache.put_hybrid_results(options_hash, results.clone());

    // Cache hit
    let result = cache.get_hybrid_results(options_hash);
    assert!(result.is_some());
    assert_eq!(result.unwrap().len(), 1);
}

#[test]
fn test_invalidation_clears_affected_entries() {
    let cache = SearchCache::new();

    // Create results with different entry dependencies
    let results1 = vec![
        HybridSearchResult {
            id: "entry-a".to_string(),
            score: 0.9,
            bm25_score: 0.8,
            semantic_score: 0.9,
            temporal_score: 0.7,
            graph_score: 0.0,
            code_score: 0.0,
            rerank_score: None,
            doc_type: DocType::Entry,
            knowledge_score: 0.0,
            history_score: 0.0,
        },
        HybridSearchResult {
            id: "entry-b".to_string(),
            score: 0.8,
            bm25_score: 0.7,
            semantic_score: 0.8,
            temporal_score: 0.6,
            graph_score: 0.0,
            code_score: 0.0,
            rerank_score: None,
            doc_type: DocType::Entry,
            knowledge_score: 0.0,
            history_score: 0.0,
        },
    ];

    let results2 = vec![HybridSearchResult {
        id: "entry-c".to_string(),
        score: 0.7,
        bm25_score: 0.6,
        semantic_score: 0.7,
        temporal_score: 0.5,
        graph_score: 0.0,
        code_score: 0.0,
        rerank_score: None,
        doc_type: DocType::Entry,
        knowledge_score: 0.0,
        history_score: 0.0,
    }];

    // Cache both result sets
    cache.put_hybrid_results(111, results1);
    cache.put_hybrid_results(222, results2);

    // Verify both are cached
    assert!(cache.get_hybrid_results(111).is_some());
    assert!(cache.get_hybrid_results(222).is_some());

    // Invalidate entry-a - should remove results1 but not results2
    cache.invalidate_entry("entry-a");

    assert!(cache.get_hybrid_results(111).is_none()); // Cleared
    assert!(cache.get_hybrid_results(222).is_some()); // Still cached

    // Invalidate entry-c
    cache.invalidate_entry("entry-c");

    assert!(cache.get_hybrid_results(222).is_none()); // Now cleared
}

#[test]
fn test_cache_miss_returns_none() {
    let cache = SearchCache::new();

    assert!(cache.get_hybrid_results(99999).is_none());
}
