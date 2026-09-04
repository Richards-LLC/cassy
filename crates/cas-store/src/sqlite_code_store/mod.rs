//! SQLite-based code storage implementation.
//!
//! Implements the CodeStore trait for persistent storage of indexed source code,
//! symbols, relationships, and memory links.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, Row};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use cas_code::{
    CodeFile, CodeMemoryLink, CodeMemoryLinkType, CodeRelationType, CodeRelationship, CodeSymbol,
    Language, SymbolKind,
};

/// SQLite DDL for the code-store tables (`code_files`, `code_symbols`,
/// `code_relationships`, `code_memory_links`).
///
/// **IMPORTANT:** This constant is used ONLY by `SqliteCodeStore::open()` —
/// it is DELIBERATELY EXCLUDED from the migration-runner bootstrap
/// (`Subsystem::Code::ensure_base_schema` returns `(None, None)`).
///
/// Rationale: the code-store tables are owned by the migration ledger
/// (`m131_code_files_create_table` … `m134_code_memory_links_create_table`
/// plus several follow-on ALTERs). This constant represents the modern
/// post-migration shape, NOT the m131-m134 baseline. Installing this DDL
/// before the migration chain runs would race the create-table migrations
/// and potentially shadow later ALTERs against historical column layouts.
///
/// If you ever want to add `Code` to the migration-runner bootstrap path,
/// you must FIRST either: (a) shape this constant to match the m131-m134
/// baselines, or (b) rewrite the migration chain to stop depending on
/// intermediate shapes.
///
/// See cas-bdb9 / EPIC cas-9fdb.
pub const CODE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS code_files (
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
);
"#;

/// SQLite-based implementation of CodeStore.
pub struct SqliteCodeStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteCodeStore {
    /// Open or create a SQLite code store.
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        // Ensure code tables exist
        {
            let c = crate::shared_db::lock_connection(&conn)?;
            c.execute_batch(CODE_SCHEMA)?;
        }

        Ok(Self { conn })
    }

    /// Run one self-contained write statement with the shared bounded busy
    /// retry.
    ///
    /// The connection's 5s `busy_timeout` is the only thing that used to stand
    /// between these statements and a hard failure, so a foreign writer that
    /// outlived one window turned an ordinary retirement into
    /// `failed to retire deleted source file: database error: database is
    /// locked` — a permanent file failure for a file that no longer exists
    /// (cas-8a03). Every body passed here is a single idempotent statement, so
    /// re-running it after a busy verdict cannot double-apply.
    fn write_with_retry<F>(&self, body: F) -> Result<()>
    where
        F: Fn(&Connection) -> Result<()>,
    {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            body(&conn)
        })
    }

    /// Parse a datetime string into DateTime<Utc>.
    fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Some(Utc.from_utc_datetime(&dt));
        }
        None
    }

    /// Generate a hash-based ID with the given prefix.
    fn generate_hash_id(&self, prefix: &str) -> Result<String> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        Utc::now().timestamp_nanos_opt().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        let hash = hasher.finish();
        Ok(format!("{}-{:08x}", prefix, hash as u32))
    }

    /// Normalize a file path for consistent ID generation.
    /// Strips leading `./`, converts to forward slashes, and removes redundant components.
    ///
    /// **This runs on every write**, so the value in `code_files.path` /
    /// `code_symbols.file_path` is NOT byte-identical to what the indexer
    /// passed — most consequentially, an absolute path loses its leading `/`.
    /// Any reader joining on those columns must normalize its probe the same
    /// way, which is why this is `pub(crate)` rather than private (cas-0562:
    /// the history↔symbol join silently matched nothing until it did).
    pub(crate) fn normalize_path(path: &str) -> String {
        let path = path.trim();
        // Strip leading ./
        let path = path.strip_prefix("./").unwrap_or(path);
        // Strip leading /
        let path = path.strip_prefix('/').unwrap_or(path);
        // Normalize path separators and remove redundant components
        let parts: Vec<&str> = path
            .split(['/', '\\'])
            .filter(|p| !p.is_empty() && *p != ".")
            .collect();
        parts.join("/")
    }

    /// Generate a deterministic ID for a symbol based on its identity.
    /// This ensures re-indexing the same symbol produces the same ID.
    fn generate_deterministic_symbol_id(
        qualified_name: &str,
        file_path: &str,
        repository: &str,
    ) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let normalized_path = Self::normalize_path(file_path);
        let mut hasher = DefaultHasher::new();
        qualified_name.hash(&mut hasher);
        normalized_path.hash(&mut hasher);
        repository.hash(&mut hasher);
        let hash = hasher.finish();
        format!("sym-{hash:016x}")
    }

    /// Generate a deterministic ID for a file based on its identity.
    fn generate_deterministic_file_id(repository: &str, path: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let normalized_path = Self::normalize_path(path);
        let mut hasher = DefaultHasher::new();
        repository.hash(&mut hasher);
        normalized_path.hash(&mut hasher);
        let hash = hasher.finish();
        format!("file-{hash:016x}")
    }

    /// Parse a CodeFile from a database row.
    fn row_to_code_file(row: &Row) -> rusqlite::Result<CodeFile> {
        let language_str: String = row.get(3)?;
        let created_str: String = row.get(8)?;
        let updated_str: String = row.get(9)?;

        Ok(CodeFile {
            id: row.get(0)?,
            path: row.get(1)?,
            repository: row.get(2)?,
            language: language_str.parse().unwrap_or(Language::Unknown),
            size: row.get::<_, i64>(4)? as usize,
            line_count: row.get::<_, i64>(5)? as usize,
            commit_hash: row.get(6)?,
            content_hash: row.get(7)?,
            created: Self::parse_datetime(&created_str).unwrap_or_else(Utc::now),
            updated: Self::parse_datetime(&updated_str).unwrap_or_else(Utc::now),
            scope: row.get(10)?,
        })
    }

    /// Parse a CodeSymbol from a database row.
    fn row_to_code_symbol(row: &Row) -> rusqlite::Result<CodeSymbol> {
        let kind_str: String = row.get(3)?;
        let language_str: String = row.get(4)?;
        let created_str: String = row.get(14)?;
        let updated_str: String = row.get(15)?;

        Ok(CodeSymbol {
            id: row.get(0)?,
            qualified_name: row.get(1)?,
            name: row.get(2)?,
            kind: kind_str.parse().unwrap_or(SymbolKind::Function),
            language: language_str.parse().unwrap_or(Language::Unknown),
            file_path: row.get(5)?,
            file_id: row.get(6)?,
            line_start: row.get::<_, i64>(7)? as usize,
            line_end: row.get::<_, i64>(8)? as usize,
            source: row.get(9)?,
            documentation: row.get(10)?,
            signature: row.get(11)?,
            parent_id: row.get(12)?,
            repository: row.get(13)?,
            commit_hash: row.get::<_, Option<String>>(16)?,
            created: Self::parse_datetime(&created_str).unwrap_or_else(Utc::now),
            updated: Self::parse_datetime(&updated_str).unwrap_or_else(Utc::now),
            content_hash: row.get(17)?,
            scope: row.get(18)?,
        })
    }

    /// Parse a CodeRelationship from a database row.
    fn row_to_relationship(row: &Row) -> rusqlite::Result<CodeRelationship> {
        let relation_type_str: String = row.get(3)?;
        let created_str: String = row.get(5)?;

        Ok(CodeRelationship {
            id: row.get(0)?,
            source_id: row.get(1)?,
            target_id: row.get(2)?,
            relation_type: relation_type_str
                .parse()
                .unwrap_or(CodeRelationType::References),
            weight: row.get(4)?,
            created: Self::parse_datetime(&created_str).unwrap_or_else(Utc::now),
        })
    }

    /// Parse a CodeMemoryLink from a database row.
    fn row_to_memory_link(row: &Row) -> rusqlite::Result<CodeMemoryLink> {
        let link_type_str: String = row.get(2)?;
        let created_str: String = row.get(4)?;

        Ok(CodeMemoryLink {
            code_id: row.get(0)?,
            entry_id: row.get(1)?,
            link_type: link_type_str
                .parse()
                .unwrap_or(CodeMemoryLinkType::Reference),
            confidence: row.get(3)?,
            created: Self::parse_datetime(&created_str).unwrap_or_else(Utc::now),
        })
    }
}

mod trait_impl;

#[cfg(test)]
mod tests;
