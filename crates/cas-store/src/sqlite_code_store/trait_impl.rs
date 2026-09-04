use crate::error::StoreError;
use crate::sqlite_code_store::SqliteCodeStore;
use crate::{CodeStore, Result};
use cas_code::{
    CodeFile, CodeMemoryLink, CodeMemoryLinkType, CodeRelationship, CodeSymbol, Language,
    SymbolKind,
};
use rusqlite::{OptionalExtension, params};

impl CodeStore for SqliteCodeStore {
    fn init(&self) -> Result<()> {
        // Tables are created by migrations (m131-m140)
        // This just verifies the tables exist
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.execute_batch(
            "SELECT 1 FROM code_files LIMIT 0;
             SELECT 1 FROM code_symbols LIMIT 0;
             SELECT 1 FROM code_relationships LIMIT 0;
             SELECT 1 FROM code_memory_links LIMIT 0;",
        )?;
        Ok(())
    }

    // ========== File Operations ==========

    fn generate_file_id(&self) -> Result<String> {
        self.generate_hash_id("file")
    }

    fn generate_file_id_for(&self, repository: &str, path: &str) -> String {
        Self::generate_deterministic_file_id(repository, path)
    }

    fn add_file(&self, file: &CodeFile) -> Result<()> {
        let normalized_path = Self::normalize_path(&file.path);
        self.write_with_retry(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO code_files
                 (id, path, repository, language, size, line_count, commit_hash, content_hash, created, updated, scope)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    file.id,
                    normalized_path,
                    file.repository,
                    file.language.to_string(),
                    file.size as i64,
                    file.line_count as i64,
                    file.commit_hash,
                    file.content_hash,
                    file.created.to_rfc3339(),
                    file.updated.to_rfc3339(),
                    file.scope,
                ],
            )?;
            Ok(())
        })
    }

    fn get_file(&self, id: &str) -> Result<CodeFile> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.query_row(
            "SELECT id, path, repository, language, size, line_count, commit_hash, content_hash, created, updated, scope
             FROM code_files WHERE id = ?1",
            params![id],
            Self::row_to_code_file,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(format!("file: {id}")),
            _ => StoreError::from(e),
        })
    }

    fn get_file_by_path(&self, repository: &str, path: &str) -> Result<Option<CodeFile>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let normalized_path = Self::normalize_path(path);
        conn.query_row(
            "SELECT id, path, repository, language, size, line_count, commit_hash, content_hash, created, updated, scope
             FROM code_files WHERE repository = ?1 AND path = ?2",
            params![repository, normalized_path],
            Self::row_to_code_file,
        )
        .optional()
        .map_err(StoreError::from)
    }

    fn list_files(&self, repository: &str, language: Option<Language>) -> Result<Vec<CodeFile>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;

        let sql = if language.is_some() {
            "SELECT id, path, repository, language, size, line_count, commit_hash, content_hash, created, updated, scope
             FROM code_files WHERE repository = ?1 AND language = ?2 ORDER BY path"
        } else {
            "SELECT id, path, repository, language, size, line_count, commit_hash, content_hash, created, updated, scope
             FROM code_files WHERE repository = ?1 ORDER BY path"
        };

        let mut stmt = conn.prepare_cached(sql)?;
        let rows = if let Some(lang) = language {
            stmt.query_map(
                params![repository, lang.to_string()],
                Self::row_to_code_file,
            )?
        } else {
            stmt.query_map(params![repository], Self::row_to_code_file)?
        };

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn delete_file(&self, id: &str) -> Result<()> {
        self.write_with_retry(|conn| {
            // Foreign key cascades will delete symbols and relationships
            conn.execute("DELETE FROM code_files WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    // ========== Symbol Operations ==========

    fn generate_symbol_id(&self) -> Result<String> {
        self.generate_hash_id("sym")
    }

    fn generate_symbol_id_for(
        &self,
        qualified_name: &str,
        file_path: &str,
        repository: &str,
    ) -> String {
        Self::generate_deterministic_symbol_id(qualified_name, file_path, repository)
    }

    fn add_symbol(&self, symbol: &CodeSymbol) -> Result<()> {
        let normalized_path = Self::normalize_path(&symbol.file_path);
        self.write_with_retry(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO code_symbols
                 (id, qualified_name, name, kind, language, file_path, file_id, line_start, line_end,
                  source, documentation, signature, parent_id, repository, created, updated, commit_hash, content_hash, scope)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                params![
                    symbol.id,
                    symbol.qualified_name,
                    symbol.name,
                    symbol.kind.to_string(),
                    symbol.language.to_string(),
                    normalized_path,
                    symbol.file_id,
                    symbol.line_start as i64,
                    symbol.line_end as i64,
                    symbol.source,
                    symbol.documentation,
                    symbol.signature,
                    symbol.parent_id,
                    symbol.repository,
                    symbol.created.to_rfc3339(),
                    symbol.updated.to_rfc3339(),
                    symbol.commit_hash,
                    symbol.content_hash,
                    symbol.scope,
                ],
            )?;
            Ok(())
        })
    }

    fn get_symbol(&self, id: &str) -> Result<CodeSymbol> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.query_row(
            "SELECT id, qualified_name, name, kind, language, file_path, file_id, line_start, line_end,
                    source, documentation, signature, parent_id, repository, created, updated, commit_hash, content_hash, scope
             FROM code_symbols WHERE id = ?1",
            params![id],
            Self::row_to_code_symbol,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(format!("symbol: {id}")),
            _ => StoreError::from(e),
        })
    }

    fn get_symbols_by_name(&self, qualified_name: &str) -> Result<Vec<CodeSymbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, qualified_name, name, kind, language, file_path, file_id, line_start, line_end,
                    source, documentation, signature, parent_id, repository, created, updated, commit_hash, content_hash, scope
             FROM code_symbols WHERE qualified_name = ?1",
        )?;
        let rows = stmt.query_map(params![qualified_name], Self::row_to_code_symbol)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn get_symbols_in_file(&self, file_id: &str) -> Result<Vec<CodeSymbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, qualified_name, name, kind, language, file_path, file_id, line_start, line_end,
                    source, documentation, signature, parent_id, repository, created, updated, commit_hash, content_hash, scope
             FROM code_symbols WHERE file_id = ?1 ORDER BY line_start",
        )?;
        let rows = stmt.query_map(params![file_id], Self::row_to_code_symbol)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn search_symbols(
        &self,
        name_pattern: &str,
        kind: Option<SymbolKind>,
        language: Option<Language>,
        limit: usize,
    ) -> Result<Vec<CodeSymbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;

        // Build query dynamically based on filters
        let mut sql = String::from(
            "SELECT id, qualified_name, name, kind, language, file_path, file_id, line_start, line_end,
                    source, documentation, signature, parent_id, repository, created, updated, commit_hash, content_hash, scope
             FROM code_symbols WHERE (qualified_name LIKE ?1 OR name LIKE ?1)",
        );

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(name_pattern.to_string())];

        if let Some(k) = kind {
            sql.push_str(" AND kind = ?");
            params_vec.push(Box::new(k.to_string()));
        }
        if let Some(l) = language {
            sql.push_str(" AND language = ?");
            params_vec.push(Box::new(l.to_string()));
        }

        sql.push_str(" ORDER BY name LIMIT ?");
        params_vec.push(Box::new(limit as i64));

        let mut stmt = conn.prepare_cached(&sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), Self::row_to_code_symbol)?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn search_symbols_paginated(
        &self,
        name_pattern: &str,
        kind: Option<SymbolKind>,
        language: Option<Language>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<CodeSymbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;

        // Build query dynamically based on filters
        let mut sql = String::from(
            "SELECT id, qualified_name, name, kind, language, file_path, file_id, line_start, line_end,
                    source, documentation, signature, parent_id, repository, created, updated, commit_hash, content_hash, scope
             FROM code_symbols WHERE (qualified_name LIKE ?1 OR name LIKE ?1)",
        );

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(name_pattern.to_string())];

        if let Some(k) = kind {
            sql.push_str(" AND kind = ?");
            params_vec.push(Box::new(k.to_string()));
        }
        if let Some(l) = language {
            sql.push_str(" AND language = ?");
            params_vec.push(Box::new(l.to_string()));
        }

        sql.push_str(" ORDER BY name LIMIT ? OFFSET ?");
        params_vec.push(Box::new(limit as i64));
        params_vec.push(Box::new(offset as i64));

        let mut stmt = conn.prepare_cached(&sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), Self::row_to_code_symbol)?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn delete_symbol(&self, id: &str) -> Result<()> {
        self.write_with_retry(|conn| {
            conn.execute("DELETE FROM code_symbols WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    fn delete_symbols_in_file(&self, file_id: &str) -> Result<()> {
        self.write_with_retry(|conn| {
            conn.execute(
                "DELETE FROM code_symbols WHERE file_id = ?1",
                params![file_id],
            )?;
            Ok(())
        })
    }

    // ========== Relationship Operations ==========

    fn generate_relationship_id(&self) -> Result<String> {
        self.generate_hash_id("rel")
    }

    fn add_relationship(&self, rel: &CodeRelationship) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO code_relationships
             (id, source_id, target_id, relation_type, weight, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                rel.id,
                rel.source_id,
                rel.target_id,
                rel.relation_type.to_string(),
                rel.weight,
                rel.created.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn get_callers(&self, symbol_id: &str) -> Result<Vec<CodeSymbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT s.id, s.qualified_name, s.name, s.kind, s.language, s.file_path, s.file_id,
                    s.line_start, s.line_end, s.source, s.documentation, s.signature, s.parent_id,
                    s.repository, s.created, s.updated, s.commit_hash, s.content_hash, s.scope
             FROM code_symbols s
             INNER JOIN code_relationships r ON s.id = r.source_id
             WHERE r.target_id = ?1 AND r.relation_type = 'calls'",
        )?;
        let rows = stmt.query_map(params![symbol_id], Self::row_to_code_symbol)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn get_callees(&self, symbol_id: &str) -> Result<Vec<CodeSymbol>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT s.id, s.qualified_name, s.name, s.kind, s.language, s.file_path, s.file_id,
                    s.line_start, s.line_end, s.source, s.documentation, s.signature, s.parent_id,
                    s.repository, s.created, s.updated, s.commit_hash, s.content_hash, s.scope
             FROM code_symbols s
             INNER JOIN code_relationships r ON s.id = r.target_id
             WHERE r.source_id = ?1 AND r.relation_type = 'calls'",
        )?;
        let rows = stmt.query_map(params![symbol_id], Self::row_to_code_symbol)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn get_relationships_from(&self, symbol_id: &str) -> Result<Vec<CodeRelationship>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, source_id, target_id, relation_type, weight, created
             FROM code_relationships WHERE source_id = ?1",
        )?;
        let rows = stmt.query_map(params![symbol_id], Self::row_to_relationship)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn get_relationships_to(&self, symbol_id: &str) -> Result<Vec<CodeRelationship>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, source_id, target_id, relation_type, weight, created
             FROM code_relationships WHERE target_id = ?1",
        )?;
        let rows = stmt.query_map(params![symbol_id], Self::row_to_relationship)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn delete_relationships_for_symbol(&self, symbol_id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.execute(
            "DELETE FROM code_relationships WHERE source_id = ?1 OR target_id = ?1",
            params![symbol_id],
        )?;
        Ok(())
    }

    // ========== Memory Link Operations ==========

    fn link_to_memory(&self, link: &CodeMemoryLink) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO code_memory_links
             (code_id, entry_id, link_type, confidence, created)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                link.code_id,
                link.entry_id,
                link.link_type.to_string(),
                link.confidence,
                link.created.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn get_linked_memories(&self, code_id: &str) -> Result<Vec<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached("SELECT entry_id FROM code_memory_links WHERE code_id = ?1")?;
        let rows = stmt.query_map(params![code_id], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<String>, _>>()
            .map_err(StoreError::from)
    }

    fn get_linked_code(&self, entry_id: &str) -> Result<Vec<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached("SELECT code_id FROM code_memory_links WHERE entry_id = ?1")?;
        let rows = stmt.query_map(params![entry_id], |row| row.get(0))?;
        rows.collect::<std::result::Result<Vec<String>, _>>()
            .map_err(StoreError::from)
    }

    fn get_memory_links(&self, code_id: &str) -> Result<Vec<CodeMemoryLink>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT code_id, entry_id, link_type, confidence, created
             FROM code_memory_links WHERE code_id = ?1",
        )?;
        let rows = stmt.query_map(params![code_id], Self::row_to_memory_link)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn delete_memory_link(
        &self,
        code_id: &str,
        entry_id: &str,
        link_type: CodeMemoryLinkType,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.execute(
            "DELETE FROM code_memory_links WHERE code_id = ?1 AND entry_id = ?2 AND link_type = ?3",
            params![code_id, entry_id, link_type.to_string()],
        )?;
        Ok(())
    }

    fn delete_memory_links_for_code(&self, code_id: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.execute(
            "DELETE FROM code_memory_links WHERE code_id = ?1",
            params![code_id],
        )?;
        Ok(())
    }

    // ========== Bulk Operations ==========

    fn add_symbols_batch(&self, symbols: &[CodeSymbol]) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        // Bounded retry on acquisition: a single `ImmediateTx::new` gave up the
        // moment a foreign writer held the lock, and the indexer's fallback is
        // to re-insert every symbol one at a time (cas-8a03).
        crate::shared_db::with_immediate_write_txn(&conn, |tx| {
            for symbol in symbols {
                let normalized_path = Self::normalize_path(&symbol.file_path);
                tx.execute(
                    "INSERT OR REPLACE INTO code_symbols
                     (id, qualified_name, name, kind, language, file_path, file_id, line_start, line_end,
                      source, documentation, signature, parent_id, repository, created, updated, commit_hash, content_hash, scope)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                    params![
                        symbol.id,
                        symbol.qualified_name,
                        symbol.name,
                        symbol.kind.to_string(),
                        symbol.language.to_string(),
                        normalized_path,
                        symbol.file_id,
                        symbol.line_start as i64,
                        symbol.line_end as i64,
                        symbol.source,
                        symbol.documentation,
                        symbol.signature,
                        symbol.parent_id,
                        symbol.repository,
                        symbol.created.to_rfc3339(),
                        symbol.updated.to_rfc3339(),
                        symbol.commit_hash,
                        symbol.content_hash,
                        symbol.scope,
                    ],
                )?;
            }
            Ok(())
        })
    }

    fn add_relationships_batch(&self, relationships: &[CodeRelationship]) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        crate::shared_db::with_immediate_write_txn(&conn, |tx| {
            for rel in relationships {
                tx.execute(
                    "INSERT OR REPLACE INTO code_relationships
                 (id, source_id, target_id, relation_type, weight, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        rel.id,
                        rel.source_id,
                        rel.target_id,
                        rel.relation_type.to_string(),
                        rel.weight,
                        rel.created.to_rfc3339(),
                    ],
                )?;
            }
            Ok(())
        })
    }

    fn get_symbols_batch(&self, ids: &[&str]) -> Result<Vec<CodeSymbol>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;

        // Build IN clause with placeholders
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT id, qualified_name, name, kind, language, file_path, file_id, line_start, line_end,
                    source, documentation, signature, parent_id, repository, created, updated, commit_hash, content_hash, scope
             FROM code_symbols WHERE id IN ({})",
            placeholders.join(", ")
        );

        let mut stmt = conn.prepare_cached(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), Self::row_to_code_symbol)?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    // ========== Stats ==========

    fn count_files(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.query_row("SELECT COUNT(*) FROM code_files", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|c| c as usize)
        .map_err(StoreError::from)
    }

    fn count_symbols(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        conn.query_row("SELECT COUNT(*) FROM code_symbols", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|c| c as usize)
        .map_err(StoreError::from)
    }

    fn count_files_by_language(&self) -> Result<std::collections::HashMap<Language, usize>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| StoreError::Other("lock poisoned".to_string()))?;
        let mut stmt =
            conn.prepare_cached("SELECT language, COUNT(*) FROM code_files GROUP BY language")?;
        let rows = stmt.query_map([], |row| {
            let lang_str: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((lang_str, count as usize))
        })?;

        let mut counts = std::collections::HashMap::new();
        for row in rows {
            let (lang_str, count) = row?;
            if let Ok(lang) = lang_str.parse::<Language>() {
                counts.insert(lang, count);
            }
        }
        Ok(counts)
    }

    fn close(&self) -> Result<()> {
        // Connection will be closed when dropped
        Ok(())
    }
}
