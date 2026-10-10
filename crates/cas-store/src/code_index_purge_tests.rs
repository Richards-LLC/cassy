//! cas-8256: bounded purge of non-canonical code-index copies.

use super::*;
use crate::CodeStore;
use crate::code_vector_store::SqliteCodeVectorStore;
use crate::sqlite_code_store::SqliteCodeStore;
use cas_code::{
    CodeFile, CodeMemoryLink, CodeMemoryLinkType, CodeRelationType, CodeRelationship, CodeSymbol,
    Language, SymbolKind,
};
use chrono::Utc;

const CANONICAL: &str = "cassy";
const WORKER: &str = "crisp-jay-9";
const OTHER_WORKER: &str = "brave-hound-32";

fn file(repository: &str, index: usize) -> CodeFile {
    let path = format!("/repo/{repository}/src/f{index}.rs");
    CodeFile {
        id: format!("file-{repository}-{index}"),
        path,
        repository: repository.to_string(),
        language: Language::Rust,
        size: 10,
        line_count: 1,
        commit_hash: None,
        content_hash: format!("fh-{index}"),
        created: Utc::now(),
        updated: Utc::now(),
        scope: "project".to_string(),
    }
}

fn symbol(repository: &str, file: &CodeFile, index: usize) -> CodeSymbol {
    CodeSymbol {
        id: format!("sym-{repository}-{index}"),
        qualified_name: format!("{repository}::f{index}"),
        name: format!("f{index}"),
        kind: SymbolKind::Function,
        language: Language::Rust,
        file_path: file.path.clone(),
        file_id: file.id.clone(),
        line_start: 1,
        line_end: 2,
        source: "fn f() {}".to_string(),
        documentation: None,
        signature: None,
        parent_id: None,
        repository: repository.to_string(),
        commit_hash: None,
        created: Utc::now(),
        updated: Utc::now(),
        content_hash: format!("sh-{index}"),
        scope: "project".to_string(),
    }
}

/// Seed `files` files of `per_file` symbols for one repository, with a queue
/// row per symbol, one relationship per symbol and one memory link per symbol.
fn seed_repository(root: &Path, repository: &str, files: usize, per_file: usize) -> Vec<String> {
    let store = SqliteCodeStore::open(root).unwrap();
    let vectors = SqliteCodeVectorStore::open(root).unwrap();
    let mut ids = Vec::new();
    let mut counter = 0;
    for f in 0..files {
        let file = file(repository, f);
        store.add_file(&file).unwrap();
        let symbols: Vec<CodeSymbol> = (0..per_file)
            .map(|_| {
                counter += 1;
                symbol(repository, &file, counter)
            })
            .collect();
        store.add_symbols_batch(&symbols).unwrap();
        vectors.sync_file_symbols(&symbols, &[]).unwrap();
        let relationships: Vec<CodeRelationship> = symbols
            .windows(2)
            .map(|pair| CodeRelationship {
                id: format!("rel-{}-{}", pair[0].id, pair[1].id),
                source_id: pair[0].id.clone(),
                target_id: pair[1].id.clone(),
                relation_type: CodeRelationType::Calls,
                weight: 1.0,
                created: Utc::now(),
            })
            .collect();
        store.add_relationships_batch(&relationships).unwrap();
        for symbol in &symbols {
            store
                .link_to_memory(&CodeMemoryLink {
                    code_id: symbol.id.clone(),
                    entry_id: format!("entry-{}", symbol.id),
                    link_type: CodeMemoryLinkType::Reference,
                    confidence: 0.8,
                    created: Utc::now(),
                })
                .unwrap();
            ids.push(symbol.id.clone());
        }
    }
    vectors
        .record_scan(
            &format!("worktree:/repo/{repository}"),
            files,
            files,
            0,
            0,
            None,
            None,
            None,
        )
        .unwrap();
    ids
}

fn count(root: &Path, sql: &str) -> usize {
    let conn = Connection::open(root.join("cas.db")).unwrap();
    conn.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap() as usize
}

fn counts_for(root: &Path, repository: &str) -> (usize, usize, usize, usize, usize) {
    let conn = Connection::open(root.join("cas.db")).unwrap();
    let q = |sql: &str| -> usize {
        conn.query_row(sql, [repository], |row| row.get::<_, i64>(0))
            .unwrap() as usize
    };
    (
        q("SELECT COUNT(*) FROM code_files WHERE repository = ?1"),
        q("SELECT COUNT(*) FROM code_symbols WHERE repository = ?1"),
        q(
            "SELECT COUNT(*) FROM code_vector_queue q JOIN code_symbols s ON s.id = q.symbol_id \
           WHERE s.repository = ?1",
        ),
        q(
            "SELECT COUNT(*) FROM code_relationships r JOIN code_symbols s ON s.id = r.source_id \
           WHERE s.repository = ?1",
        ),
        q(
            "SELECT COUNT(*) FROM code_memory_links l JOIN code_symbols s ON s.id = l.code_id \
           WHERE s.repository = ?1",
        ),
    )
}

/// The caller's loop, as the CLI drives it (minus the secondary indexes).
fn purge(
    store: &SqliteCodeIndexPurge,
    scope: RepositoryScope<'_>,
    batching: WriteBatching,
) -> (CodeIndexPurgeStats, Vec<String>) {
    let mut stats = CodeIndexPurgeStats::default();
    let mut retired = Vec::new();
    let mut after = 0;
    loop {
        let batch = store
            .next_symbol_batch(scope, after, batching.batch_size)
            .unwrap();
        if batch.is_empty() {
            break;
        }
        assert!(batch.len() <= batching.batch_size);
        after = batch.last().unwrap().rowid;
        retired.extend(batch.iter().map(|symbol| symbol.id.clone()));
        store.delete_symbols(&batch, batching, &mut stats).unwrap();
    }
    store.delete_files(scope, batching, &mut stats).unwrap();
    (stats, retired)
}

#[test]
fn purge_removes_every_foreign_row_and_keeps_the_canonical_index_cas_8256() {
    let root = tempfile::tempdir().unwrap();
    let canonical_ids = seed_repository(root.path(), CANONICAL, 3, 10);
    let worker_ids = seed_repository(root.path(), WORKER, 5, 20);
    let other_ids = seed_repository(root.path(), OTHER_WORKER, 2, 15);

    let store = SqliteCodeIndexPurge::open_existing(root.path())
        .unwrap()
        .expect("store exists");
    let keep = vec![CANONICAL.to_string()];
    let (stats, retired) = purge(
        &store,
        RepositoryScope::AllExcept(&keep),
        WriteBatching::default(),
    );

    assert_eq!(counts_for(root.path(), CANONICAL), (3, 30, 30, 27, 30));
    assert_eq!(counts_for(root.path(), WORKER), (0, 0, 0, 0, 0));
    assert_eq!(counts_for(root.path(), OTHER_WORKER), (0, 0, 0, 0, 0));
    assert_eq!(stats.symbols_deleted, worker_ids.len() + other_ids.len());
    assert_eq!(stats.files_deleted, 7);
    assert_eq!(stats.queue_rows_deleted, 130);
    // 5 files x 19 adjacent pairs + 2 files x 14 adjacent pairs.
    assert_eq!(stats.relationships_deleted, 95 + 28);
    assert_eq!(stats.memory_links_deleted, 130);
    let mut expected: Vec<String> = worker_ids.into_iter().chain(other_ids).collect();
    expected.sort();
    let mut retired = retired;
    retired.sort();
    assert_eq!(retired, expected, "the caller saw every purged id once");
    // Nothing left that a queue row or a link could still point at.
    assert_eq!(
        count(root.path(), "SELECT COUNT(*) FROM code_vector_queue"),
        canonical_ids.len()
    );
    assert_eq!(
        count(
            root.path(),
            "SELECT COUNT(*) FROM code_vector_queue q \
             WHERE NOT EXISTS (SELECT 1 FROM code_symbols s WHERE s.id = q.symbol_id)"
        ),
        0
    );

    let (second, _) = purge(
        &store,
        RepositoryScope::AllExcept(&keep),
        WriteBatching::default(),
    );
    assert!(second.is_noop(), "second purge changed rows: {second:?}");
    assert_eq!(second.writes.transactions, 0);
}

#[test]
fn no_purge_transaction_changes_more_rows_than_the_batch_size_cas_8256() {
    let root = tempfile::tempdir().unwrap();
    seed_repository(root.path(), CANONICAL, 1, 5);
    // 2,500 symbols, 2,500 queue rows, 2,450 relationships, 2,500 links, 50 files.
    seed_repository(root.path(), WORKER, 50, 50);
    let before = count(root.path(), "SELECT COUNT(*) FROM code_symbols")
        + count(root.path(), "SELECT COUNT(*) FROM code_vector_queue")
        + count(root.path(), "SELECT COUNT(*) FROM code_relationships")
        + count(root.path(), "SELECT COUNT(*) FROM code_memory_links")
        + count(root.path(), "SELECT COUNT(*) FROM code_files");

    let store = SqliteCodeIndexPurge::open_existing(root.path())
        .unwrap()
        .unwrap();
    let keep = vec![CANONICAL.to_string()];
    let batching = WriteBatching::new(100, Duration::ZERO);
    let (stats, _) = purge(&store, RepositoryScope::AllExcept(&keep), batching);

    let after = count(root.path(), "SELECT COUNT(*) FROM code_symbols")
        + count(root.path(), "SELECT COUNT(*) FROM code_vector_queue")
        + count(root.path(), "SELECT COUNT(*) FROM code_relationships")
        + count(root.path(), "SELECT COUNT(*) FROM code_memory_links")
        + count(root.path(), "SELECT COUNT(*) FROM code_files");
    let removed = before - after;
    let reported = stats.symbols_deleted
        + stats.queue_rows_deleted
        + stats.relationships_deleted
        + stats.memory_links_deleted
        + stats.files_deleted;
    assert_eq!(removed, 2_500 + 2_500 + 2_450 + 2_500 + 50);
    assert_eq!(
        reported, removed,
        "every deleted row was counted in some transaction"
    );
    assert!(
        stats.writes.largest_transaction_rows <= 100,
        "a purge transaction changed {} rows",
        stats.writes.largest_transaction_rows
    );
    assert!(
        stats.writes.transactions >= removed.div_ceil(100),
        "{} transactions cannot carry {removed} rows at <=100 each",
        stats.writes.transactions
    );
    assert_eq!(counts_for(root.path(), CANONICAL), (1, 5, 5, 4, 5));
}

#[test]
fn purge_with_an_empty_keep_set_is_refused_cas_8256() {
    let root = tempfile::tempdir().unwrap();
    seed_repository(root.path(), CANONICAL, 1, 3);
    let store = SqliteCodeIndexPurge::open_existing(root.path())
        .unwrap()
        .unwrap();
    let keep: Vec<String> = Vec::new();
    assert!(
        store
            .next_symbol_batch(RepositoryScope::AllExcept(&keep), 0, 10)
            .is_err()
    );
    let mut stats = CodeIndexPurgeStats::default();
    assert!(
        store
            .delete_files(
                RepositoryScope::AllExcept(&keep),
                WriteBatching::default(),
                &mut stats
            )
            .is_err()
    );
    assert_eq!(counts_for(root.path(), CANONICAL), (1, 3, 3, 2, 3));
}

#[test]
fn removed_worktree_scope_touches_only_that_repository_cas_8256() {
    let root = tempfile::tempdir().unwrap();
    seed_repository(root.path(), CANONICAL, 1, 4);
    seed_repository(root.path(), WORKER, 2, 4);
    seed_repository(root.path(), OTHER_WORKER, 2, 4);
    let store = SqliteCodeIndexPurge::open_existing(root.path())
        .unwrap()
        .unwrap();
    let (stats, _) = purge(
        &store,
        RepositoryScope::Only(WORKER),
        WriteBatching::new(3, Duration::ZERO),
    );
    assert_eq!(stats.symbols_deleted, 8);
    assert_eq!(counts_for(root.path(), WORKER), (0, 0, 0, 0, 0));
    assert_eq!(counts_for(root.path(), OTHER_WORKER), (2, 8, 8, 6, 8));
    assert_eq!(counts_for(root.path(), CANONICAL), (1, 4, 4, 3, 4));
    assert!(stats.writes.largest_transaction_rows <= 3);
}

#[test]
fn repository_file_counts_name_every_copy_cas_8256() {
    let root = tempfile::tempdir().unwrap();
    seed_repository(root.path(), CANONICAL, 3, 1);
    seed_repository(root.path(), WORKER, 2, 1);
    let store = SqliteCodeIndexPurge::open_existing(root.path())
        .unwrap()
        .unwrap();
    assert_eq!(
        store.repository_file_counts().unwrap(),
        vec![(CANONICAL.to_string(), 3), (WORKER.to_string(), 2)]
    );
}

#[test]
fn scan_receipts_keep_the_canonical_key_and_legacy_names_cas_8256() {
    let root = tempfile::tempdir().unwrap();
    seed_repository(root.path(), CANONICAL, 1, 1);
    seed_repository(root.path(), WORKER, 1, 1);
    seed_repository(root.path(), OTHER_WORKER, 1, 1);
    let vectors = SqliteCodeVectorStore::open(root.path()).unwrap();
    vectors
        .record_scan("cas-src", 1, 1, 0, 0, None, None, None)
        .unwrap();
    let store = SqliteCodeIndexPurge::open_existing(root.path())
        .unwrap()
        .unwrap();
    let keep = vec![format!("worktree:/repo/{CANONICAL}")];
    let mut stats = CodeIndexPurgeStats::default();
    let removed = store
        .delete_scan_receipts(
            ScanReceiptScope::WorktreeKeysExcept(&keep),
            WriteBatching::new(1, Duration::ZERO),
            &mut stats,
        )
        .unwrap();
    assert_eq!(removed, 2);
    assert_eq!(stats.scan_receipts_deleted, 2);
    assert!(stats.writes.largest_transaction_rows <= 1);
    assert!(vectors.index_state(&keep[0]).unwrap().is_some());
    assert!(vectors.index_state("cas-src").unwrap().is_some());
    assert!(
        vectors
            .index_state(&format!("worktree:/repo/{WORKER}"))
            .unwrap()
            .is_none()
    );

    let exact = format!("worktree:/repo/{CANONICAL}");
    store
        .delete_scan_receipts(
            ScanReceiptScope::Exact(&exact),
            WriteBatching::default(),
            &mut stats,
        )
        .unwrap();
    assert!(vectors.index_state(&exact).unwrap().is_none());
}

#[test]
fn open_existing_never_creates_a_database_cas_8256() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        SqliteCodeIndexPurge::open_existing(root.path())
            .unwrap()
            .is_none()
    );
    assert!(!root.path().join("cas.db").exists());
}

#[test]
fn purge_on_a_store_without_code_tables_is_a_noop_cas_8256() {
    let root = tempfile::tempdir().unwrap();
    // A cas.db that has never seen the code store.
    Connection::open(root.path().join("cas.db"))
        .unwrap()
        .execute_batch("CREATE TABLE unrelated (id INTEGER)")
        .unwrap();
    let store = SqliteCodeIndexPurge::open_existing(root.path())
        .unwrap()
        .expect("database exists");
    let keep = vec![CANONICAL.to_string()];
    let (stats, retired) = purge(
        &store,
        RepositoryScope::AllExcept(&keep),
        WriteBatching::default(),
    );
    assert!(stats.is_noop());
    assert!(retired.is_empty());
    assert!(store.repository_file_counts().unwrap().is_empty());
}

/// The migration-built schema (unlike `CODE_SCHEMA`) declares foreign keys,
/// including `code_symbols.parent_id REFERENCES code_symbols(id) ON DELETE
/// SET NULL` with no index on `parent_id`. With enforcement on, every deleted
/// symbol scans the whole symbol table. The purge suspends enforcement for its
/// own transactions only: no referential action fires (the canonical child's
/// `parent_id` is left as written) and the connection's setting is restored.
#[test]
fn purge_runs_without_foreign_key_scans_and_restores_enforcement_cas_8256() {
    let root = tempfile::tempdir().unwrap();
    Connection::open(root.path().join("cas.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE code_files (
                 id TEXT PRIMARY KEY, path TEXT NOT NULL, repository TEXT NOT NULL,
                 language TEXT NOT NULL, size INTEGER NOT NULL DEFAULT 0,
                 line_count INTEGER NOT NULL DEFAULT 0, commit_hash TEXT,
                 content_hash TEXT NOT NULL, created TEXT NOT NULL, updated TEXT NOT NULL,
                 scope TEXT NOT NULL DEFAULT 'project', UNIQUE(repository, path));
             CREATE TABLE code_symbols (
                 id TEXT PRIMARY KEY, qualified_name TEXT NOT NULL, name TEXT NOT NULL,
                 kind TEXT NOT NULL, language TEXT NOT NULL, file_path TEXT NOT NULL,
                 file_id TEXT NOT NULL, line_start INTEGER NOT NULL, line_end INTEGER NOT NULL,
                 source TEXT NOT NULL, documentation TEXT, signature TEXT, parent_id TEXT,
                 repository TEXT NOT NULL, commit_hash TEXT, created TEXT NOT NULL,
                 updated TEXT NOT NULL, content_hash TEXT NOT NULL,
                 scope TEXT NOT NULL DEFAULT 'project',
                 FOREIGN KEY (file_id) REFERENCES code_files(id) ON DELETE CASCADE,
                 FOREIGN KEY (parent_id) REFERENCES code_symbols(id) ON DELETE SET NULL);
             CREATE TABLE code_relationships (
                 id TEXT PRIMARY KEY, source_id TEXT NOT NULL, target_id TEXT NOT NULL,
                 relation_type TEXT NOT NULL, weight REAL NOT NULL DEFAULT 1.0,
                 created TEXT NOT NULL,
                 FOREIGN KEY (source_id) REFERENCES code_symbols(id) ON DELETE CASCADE,
                 FOREIGN KEY (target_id) REFERENCES code_symbols(id) ON DELETE CASCADE,
                 UNIQUE(source_id, target_id, relation_type));",
        )
        .unwrap();
    seed_repository(root.path(), CANONICAL, 1, 3);
    seed_repository(root.path(), WORKER, 2, 4);
    // A canonical symbol naming a worker symbol as its parent: with
    // enforcement on, deleting the parent would rewrite this row.
    let probe = Connection::open(root.path().join("cas.db")).unwrap();
    probe
        .execute(
            "UPDATE code_symbols SET parent_id = ?1 WHERE id = ?2",
            [format!("sym-{WORKER}-1"), format!("sym-{CANONICAL}-1")],
        )
        .unwrap();

    let store = SqliteCodeIndexPurge::open_existing(root.path())
        .unwrap()
        .unwrap();
    let shared = crate::shared_db::shared_connection(&root.path().join("cas.db")).unwrap();
    let enforced = |conn: &std::sync::Arc<std::sync::Mutex<Connection>>| -> i64 {
        conn.lock()
            .unwrap()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap()
    };
    assert_eq!(
        enforced(&shared),
        1,
        "shared connections enforce foreign keys"
    );

    let keep = vec![CANONICAL.to_string()];
    let (stats, _) = purge(
        &store,
        RepositoryScope::AllExcept(&keep),
        WriteBatching::new(2, Duration::ZERO),
    );
    assert_eq!(stats.symbols_deleted, 8);
    assert_eq!(stats.files_deleted, 2);
    assert_eq!(counts_for(root.path(), WORKER), (0, 0, 0, 0, 0));
    assert_eq!(counts_for(root.path(), CANONICAL), (1, 3, 3, 2, 3));
    let parent: Option<String> = probe
        .query_row(
            "SELECT parent_id FROM code_symbols WHERE id = ?1",
            [format!("sym-{CANONICAL}-1")],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        parent.as_deref(),
        Some(format!("sym-{WORKER}-1").as_str()),
        "a referential action ran inside the purge"
    );
    assert_eq!(enforced(&shared), 1, "enforcement was not restored");
}
