use crate::code_search::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Mock CodeStore that tracks batch vs single symbol fetches
struct MockCodeStore {
    symbols: HashMap<String, CodeSymbol>,
    single_fetch_count: AtomicUsize,
    batch_fetch_count: AtomicUsize,
}

impl MockCodeStore {
    fn new() -> Self {
        Self {
            symbols: HashMap::new(),
            single_fetch_count: AtomicUsize::new(0),
            batch_fetch_count: AtomicUsize::new(0),
        }
    }

    fn insert_symbol(&mut self, symbol: CodeSymbol) {
        self.symbols.insert(symbol.id.clone(), symbol);
    }

    fn single_fetches(&self) -> usize {
        self.single_fetch_count.load(Ordering::SeqCst)
    }

    fn batch_fetches(&self) -> usize {
        self.batch_fetch_count.load(Ordering::SeqCst)
    }
}

impl CodeStore for MockCodeStore {
    fn init(&self) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn generate_file_id(&self) -> cas_store::error::Result<String> {
        Ok("f-1".into())
    }
    fn generate_file_id_for(&self, _: &str, _: &str) -> String {
        "f-1".into()
    }
    fn add_file(&self, _: &cas_code::CodeFile) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn get_file(&self, _: &str) -> cas_store::error::Result<cas_code::CodeFile> {
        Err(cas_store::error::StoreError::NotFound("".into()))
    }
    fn get_file_by_path(
        &self,
        _: &str,
        _: &str,
    ) -> cas_store::error::Result<Option<cas_code::CodeFile>> {
        Ok(None)
    }
    fn list_files(
        &self,
        _: &str,
        _: Option<Language>,
    ) -> cas_store::error::Result<Vec<cas_code::CodeFile>> {
        Ok(vec![])
    }
    fn delete_file(&self, _: &str) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn generate_symbol_id(&self) -> cas_store::error::Result<String> {
        Ok("s-1".into())
    }
    fn generate_symbol_id_for(&self, _: &str, _: &str, _: &str) -> String {
        "s-1".into()
    }
    fn add_symbol(&self, _: &CodeSymbol) -> cas_store::error::Result<()> {
        Ok(())
    }

    fn get_symbol(&self, id: &str) -> cas_store::error::Result<CodeSymbol> {
        self.single_fetch_count.fetch_add(1, Ordering::SeqCst);
        self.symbols
            .get(id)
            .cloned()
            .ok_or_else(|| cas_store::error::StoreError::NotFound(id.into()))
    }

    fn get_symbols_by_name(&self, name: &str) -> cas_store::error::Result<Vec<CodeSymbol>> {
        Ok(self
            .symbols
            .values()
            .filter(|s| s.qualified_name == name)
            .cloned()
            .collect())
    }
    fn get_symbols_in_file(&self, _: &str) -> cas_store::error::Result<Vec<CodeSymbol>> {
        Ok(vec![])
    }
    fn search_symbols(
        &self,
        _: &str,
        _: Option<SymbolKind>,
        _: Option<Language>,
        _: usize,
    ) -> cas_store::error::Result<Vec<CodeSymbol>> {
        Ok(self.symbols.values().cloned().collect())
    }
    fn search_symbols_paginated(
        &self,
        _: &str,
        _: Option<SymbolKind>,
        _: Option<Language>,
        limit: usize,
        offset: usize,
    ) -> cas_store::error::Result<Vec<CodeSymbol>> {
        Ok(self
            .symbols
            .values()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect())
    }
    fn delete_symbol(&self, _: &str) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn delete_symbols_in_file(&self, _: &str) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn generate_relationship_id(&self) -> cas_store::error::Result<String> {
        Ok("r-1".into())
    }
    fn add_relationship(&self, _: &cas_code::CodeRelationship) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn get_callers(&self, _: &str) -> cas_store::error::Result<Vec<CodeSymbol>> {
        Ok(vec![])
    }
    fn get_callees(&self, _: &str) -> cas_store::error::Result<Vec<CodeSymbol>> {
        Ok(vec![])
    }
    fn get_relationships_from(
        &self,
        _: &str,
    ) -> cas_store::error::Result<Vec<cas_code::CodeRelationship>> {
        Ok(vec![])
    }
    fn get_relationships_to(
        &self,
        _: &str,
    ) -> cas_store::error::Result<Vec<cas_code::CodeRelationship>> {
        Ok(vec![])
    }
    fn delete_relationships_for_symbol(&self, _: &str) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn link_to_memory(&self, _: &cas_code::CodeMemoryLink) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn get_linked_memories(&self, _: &str) -> cas_store::error::Result<Vec<String>> {
        Ok(vec![])
    }
    fn get_linked_code(&self, _: &str) -> cas_store::error::Result<Vec<String>> {
        Ok(vec![])
    }
    fn get_memory_links(&self, _: &str) -> cas_store::error::Result<Vec<cas_code::CodeMemoryLink>> {
        Ok(vec![])
    }
    fn delete_memory_link(
        &self,
        _: &str,
        _: &str,
        _: cas_code::CodeMemoryLinkType,
    ) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn delete_memory_links_for_code(&self, _: &str) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn add_symbols_batch(&self, _: &[CodeSymbol]) -> cas_store::error::Result<()> {
        Ok(())
    }
    fn add_relationships_batch(
        &self,
        _: &[cas_code::CodeRelationship],
    ) -> cas_store::error::Result<()> {
        Ok(())
    }

    fn get_symbols_batch(&self, ids: &[&str]) -> cas_store::error::Result<Vec<CodeSymbol>> {
        self.batch_fetch_count.fetch_add(1, Ordering::SeqCst);
        Ok(ids
            .iter()
            .filter_map(|id| self.symbols.get(*id).cloned())
            .collect())
    }

    fn count_files(&self) -> cas_store::error::Result<usize> {
        Ok(0)
    }
    fn count_symbols(&self) -> cas_store::error::Result<usize> {
        Ok(self.symbols.len())
    }
    fn count_files_by_language(
        &self,
    ) -> cas_store::error::Result<std::collections::HashMap<Language, usize>> {
        Ok(HashMap::new())
    }
    fn close(&self) -> cas_store::error::Result<()> {
        Ok(())
    }
}

/// Mock vector store
struct MockVectorStore {
    vectors: HashMap<String, Vec<f32>>,
}

impl MockVectorStore {
    fn new() -> Self {
        Self {
            vectors: HashMap::new(),
        }
    }

    fn add(&mut self, key: &str, _vec: Vec<f32>) {
        self.vectors.insert(key.to_string(), vec![0.0; 128]);
    }
}

impl VectorStore for MockVectorStore {
    fn store(&self, _: &str, _: &[f32]) -> Result<()> {
        Ok(())
    }
    fn get(&self, _: &str) -> Result<Option<Vec<f32>>> {
        Ok(None)
    }
    fn delete(&self, _: &str) -> Result<()> {
        Ok(())
    }
    fn search(&self, _: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
        Ok(self
            .vectors
            .keys()
            .take(k)
            .map(|k| (k.clone(), 0.8))
            .collect())
    }
    fn exists(&self, _: &str) -> Result<bool> {
        Ok(false)
    }
    fn count(&self) -> Result<usize> {
        Ok(self.vectors.len())
    }
    fn list_ids(&self) -> Result<Vec<String>> {
        Ok(self.vectors.keys().cloned().collect())
    }
    fn dimension(&self) -> usize {
        128
    }
}

/// Mock BM25 index
struct MockBm25Index {
    docs: HashMap<String, String>,
}

impl MockBm25Index {
    fn new() -> Self {
        Self {
            docs: HashMap::new(),
        }
    }

    fn add(&mut self, id: &str, content: &str) {
        self.docs.insert(id.to_string(), content.to_string());
    }
}

impl TextIndex for MockBm25Index {
    fn index(&self, _: &dyn crate::traits::SearchDocument) -> Result<()> {
        Ok(())
    }
    fn remove(&self, _: &str) -> Result<()> {
        Ok(())
    }
    fn search(&self, query: &str, limit: usize) -> Result<Vec<(String, f64)>> {
        let query_lower = query.to_lowercase();
        Ok(self
            .docs
            .iter()
            .filter(|(_, content)| content.to_lowercase().contains(&query_lower))
            .take(limit)
            .enumerate()
            .map(|(i, (id, _))| (id.clone(), 1.0 - i as f64 * 0.1))
            .collect())
    }
    fn search_with_type(
        &self,
        query: &str,
        _doc_type: &str,
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        self.search(query, limit)
    }
    fn search_with_tags(
        &self,
        query: &str,
        _tags: &[&str],
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        self.search(query, limit)
    }
    fn commit(&self) -> Result<()> {
        Ok(())
    }
}

/// Mock embedder
struct MockEmbedder;

impl Embedder for MockEmbedder {
    fn embed(&self, _: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; 128])
    }
    fn dimension(&self) -> usize {
        128
    }
}

#[test]
fn test_code_search_options_default() {
    let opts = CodeSearchOptions::default();
    assert!(opts.query.is_empty());
    assert_eq!(opts.limit, 0);
    assert!(!opts.include_source);
    assert!(!opts.semantic);
}

#[test]
fn test_create_snippet() {
    // Short source
    let snippet = CodeSearchResult::create_snippet("fn foo() {}");
    assert_eq!(snippet, "fn foo() {}");

    // Multi-line source
    let source = "fn foo() {\n    bar();\n    baz();\n    qux();\n}";
    let snippet = CodeSearchResult::create_snippet(source);
    assert!(snippet.ends_with("..."));
    assert!(snippet.contains("bar()"));
    assert!(!snippet.contains("qux()"));

    // Long line
    let long = "a".repeat(300);
    let snippet = CodeSearchResult::create_snippet(&long);
    assert_eq!(snippet.len(), 200);
    assert!(snippet.ends_with("..."));
}

#[test]
fn test_create_snippet_truncates_multibyte_text_on_char_boundary() {
    for character in ['─', '🦀', '界'] {
        let source: String = std::iter::repeat_n(character, 201).collect();
        let snippet = CodeSearchResult::create_snippet(&source);

        assert!(snippet.ends_with("..."), "missing ellipsis for {character:?}");
        assert!(
            snippet.len() <= 200,
            "snippet exceeded the existing 200-byte cap for {character:?}: {} bytes",
            snippet.len()
        );
        assert!(
            snippet
                .strip_suffix("...")
                .unwrap()
                .chars()
                .all(|value| value == character),
            "truncation corrupted {character:?}"
        );
    }
}

#[test]
fn test_code_search_stats_default() {
    let stats = CodeSearchStats::default();
    assert_eq!(stats.file_count, 0);
    assert_eq!(stats.symbol_count, 0);
}

/// Test that search uses batch fetching, not N+1 individual queries
#[test]
fn test_no_n_plus_1_with_1000_symbols() {
    // Create mock stores
    let mut code_store = MockCodeStore::new();
    let mut bm25_index = MockBm25Index::new();
    let vector_store = MockVectorStore::new();

    // Add 1000 symbols
    for i in 0..1000 {
        let symbol = CodeSymbol {
            id: format!("sym-{i:04}"),
            qualified_name: format!("module::function_{i}"),
            name: format!("function_{i}"),
            kind: SymbolKind::Function,
            language: Language::Rust,
            file_path: format!("src/mod_{}.rs", i % 10),
            file_id: format!("file-{}", i % 10),
            line_start: i * 10,
            line_end: i * 10 + 5,
            source: format!("fn function_{i}() {{ /* body */ }}"),
            documentation: Some(format!("Documentation for function {i}")),
            ..Default::default()
        };
        bm25_index.add(
            &symbol.id,
            &format!("{} {}", symbol.qualified_name, symbol.source),
        );
        code_store.insert_symbol(symbol);
    }

    // Create search instance
    let code_store = Arc::new(code_store);
    let search = CodeSearch::new(
        code_store.clone(),
        Arc::new(vector_store),
        Arc::new(bm25_index),
        None,
    );

    // Search for symbols
    let opts = CodeSearchOptions {
        query: "function".to_string(),
        limit: 100,
        include_source: false,
        ..Default::default()
    };

    let results = search.pattern_search(&opts).unwrap();

    // Verify we got results
    assert!(!results.is_empty(), "Should find matching symbols");
    assert!(results.len() <= 100, "Should respect limit");

    // Verify batch fetching was used (1+ batch calls, no single calls)
    assert_eq!(
        code_store.single_fetches(),
        0,
        "Expected batch fetches only"
    );
    assert!(
        code_store.batch_fetches() > 0,
        "Expected at least one batch fetch"
    );
    println!("Search completed with {} results", results.len());
}

/// Test that semantic search also uses batch fetching
#[test]
fn test_semantic_search_batch_fetching() {
    let mut code_store = MockCodeStore::new();
    let mut vector_store = MockVectorStore::new();
    let bm25_index = MockBm25Index::new();

    // Add 100 symbols with embeddings
    for i in 0..100 {
        let symbol = CodeSymbol {
            id: format!("sym-{i:04}"),
            qualified_name: format!("module::semantic_func_{i}"),
            name: format!("semantic_func_{i}"),
            kind: SymbolKind::Function,
            language: Language::Rust,
            file_path: "src/lib.rs".to_string(),
            file_id: "file-1".to_string(),
            line_start: i * 10,
            line_end: i * 10 + 5,
            source: format!("fn semantic_func_{i}() {{}}"),
            ..Default::default()
        };
        // Vector key format: code:{symbol_id}:full
        vector_store.add(&format!("code:{}:full", symbol.id), vec![0.0; 128]);
        code_store.insert_symbol(symbol);
    }

    let code_store = Arc::new(code_store);
    let search = CodeSearch::new(
        code_store.clone(),
        Arc::new(vector_store),
        Arc::new(bm25_index),
        Some(Arc::new(MockEmbedder)),
    );

    let opts = CodeSearchOptions {
        query: "semantic function".to_string(),
        limit: 50,
        semantic: true,
        ..Default::default()
    };

    let results = search.search(&opts).unwrap();
    assert!(!results.is_empty(), "Should find semantic matches");
    assert_eq!(
        code_store.single_fetches(),
        0,
        "Expected batch fetches only"
    );
    assert!(
        code_store.batch_fetches() > 0,
        "Expected at least one batch fetch"
    );

    println!("Semantic search returned {} results", results.len());
}

/// Verify no EmbeddingModel::load() calls inside CodeSearch
#[test]
fn test_shared_embedder_no_loading() {
    // This test verifies the architecture - CodeSearch accepts Arc<dyn Embedder>
    // and doesn't load models internally

    let code_store = MockCodeStore::new();
    let vector_store = MockVectorStore::new();
    let bm25_index = MockBm25Index::new();

    // Without embedder - semantic search falls back to pattern
    let search_no_embed: CodeSearch<MockCodeStore, MockVectorStore, MockBm25Index> =
        CodeSearch::new(
            Arc::new(code_store),
            Arc::new(vector_store),
            Arc::new(bm25_index),
            None,
        );
    assert!(!search_no_embed.has_semantic());

    // With shared embedder - no internal loading
    let code_store = MockCodeStore::new();
    let vector_store = MockVectorStore::new();
    let bm25_index = MockBm25Index::new();
    let shared_embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder);

    let search_with_embed = CodeSearch::new(
        Arc::new(code_store),
        Arc::new(vector_store),
        Arc::new(bm25_index),
        Some(shared_embedder.clone()),
    );
    assert!(search_with_embed.has_semantic());

    // The same embedder can be shared across multiple CodeSearch instances
    // without reloading the model
    let code_store2 = MockCodeStore::new();
    let vector_store2 = MockVectorStore::new();
    let bm25_index2 = MockBm25Index::new();

    let search2 = CodeSearch::new(
        Arc::new(code_store2),
        Arc::new(vector_store2),
        Arc::new(bm25_index2),
        Some(shared_embedder), // Reuse the same Arc
    );
    assert!(search2.has_semantic());
}
