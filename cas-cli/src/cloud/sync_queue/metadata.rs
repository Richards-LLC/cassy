use rusqlite::{OptionalExtension, params};

use crate::cloud::sync_queue::SyncQueue;
use crate::error::CasError;

impl SyncQueue {
    /// List all sync metadata entries in stable key order.
    pub fn list_metadata(&self) -> Result<Vec<(String, String)>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, value FROM sync_metadata ORDER BY key")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Get sync metadata value.
    pub fn get_metadata(&self, key: &str) -> Result<Option<String>, CasError> {
        let conn = self.conn.lock().unwrap();
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM sync_metadata WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    /// Set sync metadata value.
    pub fn set_metadata(&self, key: &str, value: &str) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO sync_metadata (key, value) VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
            params![key, value],
        )?;
        Ok(())
    }

    /// Delete sync metadata value.
    pub fn delete_metadata(&self, key: &str) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sync_metadata WHERE key = ?1", params![key])?;
        Ok(())
    }

    /// Delete all metadata keys sharing a prefix and return the number removed.
    ///
    /// Prefix deletion is used when a destructive content cleanup invalidates
    /// every team-pull watermark. `substr` keeps the prefix literal (unlike a
    /// `LIKE` expression, where `_` and `%` are wildcards).
    pub fn delete_metadata_with_prefix(&self, prefix: &str) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            "DELETE FROM sync_metadata WHERE substr(key, 1, length(?1)) = ?1",
            params![prefix],
        )?;
        Ok(deleted)
    }
}
