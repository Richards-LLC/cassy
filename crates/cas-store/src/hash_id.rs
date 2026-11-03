//! Shared hash-based IDs for tasks, skills, specs and indexed code.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::Result;

// A clock tick and PID alone can repeat, even within a single store instance.
// Share the counter across all stores and connections in this process.
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) enum IdKind {
    Task,
    Skill,
    Spec,
    File,
    Symbol,
    Relationship,
}

/// Keep each store's existing prefix, shortening sequence and fallback length.
/// Every candidate, including the longest fallback, must be unoccupied.
pub(crate) fn generate_hash_id(conn: &Connection, kind: IdKind) -> Result<String> {
    let (table, prefix, lengths, offset): (&str, &str, &[usize], usize) = match kind {
        IdKind::Task => ("tasks", "cas-", &[4, 5, 6, 8], 0),
        IdKind::Skill => ("skills", "cas-sk", &[2, 3, 4, 6], 0),
        IdKind::Spec => ("specs", "spec-", &[4, 5, 6, 7, 8, 12], 0),
        // Code IDs historically use the low 32 bits of the hash.
        IdKind::File => ("code_files", "file-", &[8], 8),
        IdKind::Symbol => ("code_symbols", "sym-", &[8], 8),
        IdKind::Relationship => ("code_relationships", "rel-", &[8], 8),
    };
    // Table names come only from the closed enum above.
    let mut exists = conn.prepare_cached(&format!("SELECT 1 FROM {table} WHERE id = ?1"))?;
    loop {
        let mut hasher = DefaultHasher::new();
        timestamp_nanos().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        ID_COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
        let hash = format!("{:016x}", hasher.finish());

        for &len in lengths {
            let id = format!("{prefix}{}", &hash[offset..offset + len]);
            if !exists
                .query_row([&id], |_| Ok(true))
                .optional()?
                .unwrap_or(false)
            {
                return Ok(id);
            }
        }
        // All lengths were occupied: try a new sequence rather than return a
        // duplicate ID (or replace another file through INSERT OR REPLACE).
    }
}

#[cfg(test)]
thread_local! {
    static FIXED_TIMESTAMP: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

fn timestamp_nanos() -> Option<i64> {
    #[cfg(test)]
    if let Some(timestamp) = FIXED_TIMESTAMP.get() {
        return Some(timestamp);
    }
    Utc::now().timestamp_nanos_opt()
}

#[cfg(test)]
pub(crate) fn with_fixed_timestamp<T>(timestamp: i64, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<i64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            FIXED_TIMESTAMP.set(self.0);
        }
    }
    let _restore = Restore(FIXED_TIMESTAMP.replace(Some(timestamp)));
    body()
}
