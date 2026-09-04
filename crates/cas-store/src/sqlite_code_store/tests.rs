use crate::CodeStore;
use crate::sqlite_code_store::SqliteCodeStore;
use cas_code::{
    CodeFile, CodeMemoryLink, CodeMemoryLinkType, CodeRelationType, CodeRelationship, CodeSymbol,
    Language, SymbolKind,
};
use chrono::Utc;
use rusqlite::Connection;
use tempfile::TempDir;

fn setup_test_db() -> (TempDir, SqliteCodeStore) {
    let temp = TempDir::new().unwrap();
    let cas_dir = temp.path();

    // Create the tables
    let db_path = cas_dir.join("cas.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS code_files (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                repository TEXT NOT NULL,
                language TEXT NOT NULL,
                size INTEGER NOT NULL DEFAULT 0,
                line_count INTEGER NOT NULL DEFAULT 0,
                commit_hash TEXT,
                content_hash TEXT NOT NULL,
                created TEXT NOT NULL,
                updated TEXT NOT NULL,
                scope TEXT NOT NULL DEFAULT 'project',
                UNIQUE(repository, path)
            );
            CREATE TABLE IF NOT EXISTS code_symbols (
                id TEXT PRIMARY KEY,
                qualified_name TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                language TEXT NOT NULL,
                file_path TEXT NOT NULL,
                file_id TEXT NOT NULL,
                line_start INTEGER NOT NULL,
                line_end INTEGER NOT NULL,
                source TEXT NOT NULL,
                documentation TEXT,
                signature TEXT,
                parent_id TEXT,
                repository TEXT NOT NULL,
                created TEXT NOT NULL,
                updated TEXT NOT NULL,
                commit_hash TEXT,
                content_hash TEXT NOT NULL,
                scope TEXT NOT NULL DEFAULT 'project'
            );
            CREATE TABLE IF NOT EXISTS code_relationships (
                id TEXT PRIMARY KEY,
                source_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                relation_type TEXT NOT NULL,
                weight REAL NOT NULL DEFAULT 1.0,
                created TEXT NOT NULL,
                UNIQUE(source_id, target_id, relation_type)
            );
            CREATE TABLE IF NOT EXISTS code_memory_links (
                code_id TEXT NOT NULL,
                entry_id TEXT NOT NULL,
                link_type TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 0.8,
                created TEXT NOT NULL,
                PRIMARY KEY (code_id, entry_id, link_type)
            );",
    )
    .unwrap();
    drop(conn);

    let store = SqliteCodeStore::open(cas_dir).unwrap();
    (temp, store)
}

#[test]
fn test_file_crud() {
    let (_temp, store) = setup_test_db();

    let file = CodeFile {
        id: store.generate_file_id().unwrap(),
        path: "src/lib.rs".to_string(),
        repository: "test-repo".to_string(),
        language: Language::Rust,
        size: 1024,
        line_count: 50,
        commit_hash: Some("abc123".to_string()),
        content_hash: "hash123".to_string(),
        ..Default::default()
    };

    // Add
    store.add_file(&file).unwrap();

    // Get
    let retrieved = store.get_file(&file.id).unwrap();
    assert_eq!(retrieved.path, "src/lib.rs");
    assert_eq!(retrieved.language, Language::Rust);

    // Get by path
    let by_path = store
        .get_file_by_path("test-repo", "src/lib.rs")
        .unwrap()
        .unwrap();
    assert_eq!(by_path.id, file.id);

    // List
    let files = store.list_files("test-repo", None).unwrap();
    assert_eq!(files.len(), 1);

    // Delete
    store.delete_file(&file.id).unwrap();
    assert!(store.get_file(&file.id).is_err());
}

#[test]
fn test_symbol_crud() {
    let (_temp, store) = setup_test_db();

    let file = CodeFile {
        id: store.generate_file_id().unwrap(),
        path: "src/lib.rs".to_string(),
        repository: "test-repo".to_string(),
        language: Language::Rust,
        content_hash: "hash123".to_string(),
        ..Default::default()
    };
    store.add_file(&file).unwrap();

    let symbol = CodeSymbol {
        id: store.generate_symbol_id().unwrap(),
        qualified_name: "my_crate::my_func".to_string(),
        name: "my_func".to_string(),
        kind: SymbolKind::Function,
        language: Language::Rust,
        file_path: "src/lib.rs".to_string(),
        file_id: file.id.clone(),
        line_start: 10,
        line_end: 20,
        source: "fn my_func() {}".to_string(),
        documentation: Some("A test function".to_string()),
        signature: Some("fn my_func()".to_string()),
        repository: "test-repo".to_string(),
        content_hash: "symhash".to_string(),
        ..Default::default()
    };

    // Add
    store.add_symbol(&symbol).unwrap();

    // Get
    let retrieved = store.get_symbol(&symbol.id).unwrap();
    assert_eq!(retrieved.name, "my_func");
    assert_eq!(retrieved.kind, SymbolKind::Function);

    // Get by name
    let by_name = store.get_symbols_by_name("my_crate::my_func").unwrap();
    assert_eq!(by_name.len(), 1);

    // Get in file
    let in_file = store.get_symbols_in_file(&file.id).unwrap();
    assert_eq!(in_file.len(), 1);

    // Search
    let search_results = store.search_symbols("%my_func%", None, None, 10).unwrap();
    assert_eq!(search_results.len(), 1);

    // Count
    assert_eq!(store.count_symbols().unwrap(), 1);

    // Delete
    store.delete_symbol(&symbol.id).unwrap();
    assert!(store.get_symbol(&symbol.id).is_err());
}

#[test]
fn test_relationship_operations() {
    let (_temp, store) = setup_test_db();

    // Create file and symbols
    let file = CodeFile {
        id: store.generate_file_id().unwrap(),
        path: "src/lib.rs".to_string(),
        repository: "test-repo".to_string(),
        language: Language::Rust,
        content_hash: "hash123".to_string(),
        ..Default::default()
    };
    store.add_file(&file).unwrap();

    let caller = CodeSymbol {
        id: store.generate_symbol_id().unwrap(),
        qualified_name: "my_crate::caller".to_string(),
        name: "caller".to_string(),
        kind: SymbolKind::Function,
        file_id: file.id.clone(),
        repository: "test-repo".to_string(),
        content_hash: "h1".to_string(),
        ..Default::default()
    };
    let callee = CodeSymbol {
        id: store.generate_symbol_id().unwrap(),
        qualified_name: "my_crate::callee".to_string(),
        name: "callee".to_string(),
        kind: SymbolKind::Function,
        file_id: file.id.clone(),
        repository: "test-repo".to_string(),
        content_hash: "h2".to_string(),
        ..Default::default()
    };
    store.add_symbol(&caller).unwrap();
    store.add_symbol(&callee).unwrap();

    // Add relationship
    let rel = CodeRelationship {
        id: store.generate_relationship_id().unwrap(),
        source_id: caller.id.clone(),
        target_id: callee.id.clone(),
        relation_type: CodeRelationType::Calls,
        weight: 1.0,
        created: Utc::now(),
    };
    store.add_relationship(&rel).unwrap();

    // Get callers of callee
    let callers = store.get_callers(&callee.id).unwrap();
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].name, "caller");

    // Get callees of caller
    let callees = store.get_callees(&caller.id).unwrap();
    assert_eq!(callees.len(), 1);
    assert_eq!(callees[0].name, "callee");

    // Get relationships
    let rels_from = store.get_relationships_from(&caller.id).unwrap();
    assert_eq!(rels_from.len(), 1);

    let rels_to = store.get_relationships_to(&callee.id).unwrap();
    assert_eq!(rels_to.len(), 1);
}

#[test]
fn test_memory_link_operations() {
    let (_temp, store) = setup_test_db();

    let link = CodeMemoryLink {
        code_id: "sym-12345".to_string(),
        entry_id: "2024-01-15-123".to_string(),
        link_type: CodeMemoryLinkType::Documents,
        confidence: 0.9,
        created: Utc::now(),
    };

    // Add link
    store.link_to_memory(&link).unwrap();

    // Get linked memories
    let memories = store.get_linked_memories("sym-12345").unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0], "2024-01-15-123");

    // Get linked code
    let code = store.get_linked_code("2024-01-15-123").unwrap();
    assert_eq!(code.len(), 1);
    assert_eq!(code[0], "sym-12345");

    // Get memory links
    let links = store.get_memory_links("sym-12345").unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].link_type, CodeMemoryLinkType::Documents);

    // Delete link
    store
        .delete_memory_link("sym-12345", "2024-01-15-123", CodeMemoryLinkType::Documents)
        .unwrap();
    let memories = store.get_linked_memories("sym-12345").unwrap();
    assert!(memories.is_empty());
}

#[test]
fn test_batch_operations() {
    let (_temp, store) = setup_test_db();

    let file = CodeFile {
        id: store.generate_file_id().unwrap(),
        path: "src/lib.rs".to_string(),
        repository: "test-repo".to_string(),
        content_hash: "hash123".to_string(),
        ..Default::default()
    };
    store.add_file(&file).unwrap();

    let symbols: Vec<CodeSymbol> = (0..5)
        .map(|i| CodeSymbol {
            id: format!("sym-{i}"),
            qualified_name: format!("my_crate::func_{i}"),
            name: format!("func_{i}"),
            kind: SymbolKind::Function,
            file_id: file.id.clone(),
            repository: "test-repo".to_string(),
            content_hash: format!("h{i}"),
            ..Default::default()
        })
        .collect();

    store.add_symbols_batch(&symbols).unwrap();
    assert_eq!(store.count_symbols().unwrap(), 5);
}

#[test]
fn test_normalize_path() {
    // Test leading ./
    assert_eq!(
        SqliteCodeStore::normalize_path("./src/lib.rs"),
        "src/lib.rs"
    );
    // Test leading /
    assert_eq!(SqliteCodeStore::normalize_path("/src/lib.rs"), "src/lib.rs");
    // Test multiple leading ./
    assert_eq!(
        SqliteCodeStore::normalize_path("././src/lib.rs"),
        "src/lib.rs"
    );
    // Test embedded .
    assert_eq!(
        SqliteCodeStore::normalize_path("src/./lib.rs"),
        "src/lib.rs"
    );
    // Test no prefix
    assert_eq!(SqliteCodeStore::normalize_path("src/lib.rs"), "src/lib.rs");
    // Test absolute path normalization
    assert_eq!(
        SqliteCodeStore::normalize_path("/Users/test/project/src/lib.rs"),
        "Users/test/project/src/lib.rs"
    );
    // Test Windows-style paths
    assert_eq!(
        SqliteCodeStore::normalize_path(".\\src\\lib.rs"),
        "src/lib.rs"
    );
    // Test whitespace trimming
    assert_eq!(
        SqliteCodeStore::normalize_path("  ./src/lib.rs  "),
        "src/lib.rs"
    );
}

#[test]
fn test_deterministic_symbol_id_normalization() {
    // Same file with different path representations should produce same ID
    let id1 =
        SqliteCodeStore::generate_deterministic_symbol_id("my_crate::func", "./src/lib.rs", "repo");
    let id2 =
        SqliteCodeStore::generate_deterministic_symbol_id("my_crate::func", "src/lib.rs", "repo");
    let id3 = SqliteCodeStore::generate_deterministic_symbol_id(
        "my_crate::func",
        "././src/lib.rs",
        "repo",
    );
    assert_eq!(id1, id2);
    assert_eq!(id2, id3);
}

/// cas-8a03: the retirement path's SQLite writes were bare autocommit
/// statements, so the connection's `busy_timeout` was the only thing standing
/// between them and failure. A writer that outlived one window produced
/// `failed to retire deleted source file: database error: database is locked`
/// and a permanent file failure. The busy timeout is shortened here so the
/// holder demonstrably outlives a full window without a multi-second test.
#[test]
fn code_store_writes_wait_out_a_foreign_writer_past_one_busy_timeout_window() {
    use std::sync::mpsc;
    use std::time::Duration;

    let (temp, store) = setup_test_db();
    let file = CodeFile {
        id: store.generate_file_id().unwrap(),
        path: "src/retired.rs".to_string(),
        repository: "test-repo".to_string(),
        language: Language::Rust,
        content_hash: "hash".to_string(),
        ..Default::default()
    };
    store.add_file(&file).unwrap();
    let symbol = CodeSymbol {
        id: "sym-retired".to_string(),
        qualified_name: "retired::thing".to_string(),
        name: "thing".to_string(),
        kind: SymbolKind::Function,
        language: Language::Rust,
        file_path: file.path.clone(),
        file_id: file.id.clone(),
        repository: file.repository.clone(),
        content_hash: "sym-hash".to_string(),
        created: Utc::now(),
        updated: Utc::now(),
        ..Default::default()
    };
    store
        .add_symbols_batch(std::slice::from_ref(&symbol))
        .unwrap();

    // Same shared connection the store holds; a 50 ms window makes "outlives
    // one busy_timeout window" reproducible in a fraction of a second.
    let db_path = temp.path().join("cas.db");
    let shared = crate::shared_db::shared_connection(&db_path).unwrap();
    shared
        .lock()
        .unwrap()
        .busy_timeout(Duration::from_millis(50))
        .unwrap();

    let holder_path = db_path.clone();
    let (holding, held) = mpsc::channel();
    let blocker = std::thread::spawn(move || {
        // A distinct connection: the in-process pool would otherwise serialize
        // on the mutex and never produce SQLITE_BUSY.
        let conn = Connection::open(&holder_path).unwrap();
        conn.busy_timeout(Duration::from_millis(50)).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO code_files
             (id, path, repository, language, size, line_count, commit_hash, content_hash, created, updated, scope)
             VALUES ('foreign', 'src/foreign.rs', 'other-repo', 'rust', 0, 0, NULL, 'h', '2026-01-01', '2026-01-01', 'project')",
            [],
        )
        .unwrap();
        holding.send(()).unwrap();
        // Six full busy_timeout windows: without the application-level retry
        // every write below fails outright.
        std::thread::sleep(Duration::from_millis(300));
        conn.execute_batch("COMMIT").unwrap();
    });

    held.recv().unwrap();
    store
        .delete_symbols_in_file(&file.id)
        .expect("delete_symbols_in_file must wait out the foreign writer");
    store
        .delete_file(&file.id)
        .expect("delete_file must wait out the foreign writer");
    blocker.join().unwrap();

    assert!(store.get_file(&file.id).is_err(), "retirement did not land");
    assert!(
        store.get_symbols_in_file(&file.id).unwrap().is_empty(),
        "symbol retirement did not land"
    );
}
