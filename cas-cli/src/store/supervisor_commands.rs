//! Local Commander command receipts in the existing SQLite metadata store.
//! Keys bind a reference to its factory session, supervisor and device. Claim
//! commits before PTY input: a crashed writer must never replay uncertain bytes.
use std::path::Path;
use rusqlite::params;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) enum Write { Reserve, Claim, Update }

/// Longest one receipt write waits for the shared connection or SQLite lock.
const RECEIPT_STORE_WAIT_BUDGET: std::time::Duration = std::time::Duration::from_millis(500);

fn prefix(session: &str, supervisor: &str) -> String {
    format!("commander.command.v1.{}.", hex::encode(Sha256::digest(
        serde_json::to_vec(&(session, supervisor)).expect("command scope"))))
}

fn transaction<T>(root: &Path, body: impl FnOnce(&rusqlite::Connection) -> cas_store::Result<T>) -> cas_store::Result<T> {
    // A receipt write gets its own short budget (cas-8c4f): a loop pass that
    // already spent its budget must not turn a brief holder into a failed
    // command, and an unbounded caller still never waits indefinitely.
    let _grant = cas_store::wait_budget::grant_waits_for(RECEIPT_STORE_WAIT_BUDGET);
    let _budget = cas_store::wait_budget::bound_waits_for(RECEIPT_STORE_WAIT_BUDGET);
    let shared = cas_store::shared_db::shared_connection(&root.join("cas.db"))?;
    cas_store::shared_db::with_immediate_write_txn_pooled(&shared, |conn| {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)")?;
        conn.execute("DELETE FROM metadata WHERE key LIKE 'commander.command.v1.%'
            AND json_extract(value, '$.receipt.phase') NOT IN ('queued', 'submitted')
            AND json_extract(value, '$.at') < ?1",
            [ (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339() ])?;
        body(conn)
    })
}

pub(crate) fn load(root: &Path, session: &str, supervisor: &str) -> cas_store::Result<Vec<Value>> {
    transaction(root, |conn| {
        let mut query = conn.prepare("SELECT value FROM metadata WHERE key LIKE ?1 ORDER BY key")?;
        let rows = query.query_map([format!("{}%", prefix(session, supervisor))], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    })
}

fn key(session: &str, supervisor: &str, device_id: &Value, client_ref: &Value) -> cas_store::Result<String> {
    Ok(format!("{}{}", prefix(session, supervisor), hex::encode(Sha256::digest(serde_json::to_vec(&(device_id, client_ref))?))))
}

/// One caller's receipt, for a reader that must not change the store: no
/// retention pass, no schema creation, and a short bounded wait.
pub(crate) fn read(root: &Path, session: &str, supervisor: &str, device_id: &str, client_ref: &str) -> cas_store::Result<Option<Value>> {
    let key = key(session, supervisor, &Value::from(device_id), &Value::from(client_ref))?;
    if !root.join("cas.db").exists() { return Ok(None); }
    let conn = rusqlite::Connection::open_with_flags(root.join("cas.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(std::time::Duration::from_millis(250))?;
    let found = conn.query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| row.get::<_, String>(0));
    match found {
        // Unreadable bytes are a missing row, as they are to the daemon.
        Ok(value) => Ok(serde_json::from_str(&value).ok()),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(rusqlite::Error::SqliteFailure(_, Some(message))) if message.starts_with("no such table") => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn write(root: &Path, session: &str, supervisor: &str, value: &Value, mode: Write) -> cas_store::Result<bool> {
    let receipt = &value["receipt"];
    let key = key(session, supervisor, &receipt["device_id"], &receipt["client_ref"])?;
    let value = serde_json::to_string(value)?;
    transaction(root, |conn| {
        let sql = match mode {
            Write::Reserve => "INSERT OR IGNORE INTO metadata (key, value) VALUES (?1, ?2)",
            Write::Claim => "UPDATE metadata SET value=?2 WHERE key=?1 AND json_extract(value, '$.receipt.phase')='queued'",
            Write::Update => "UPDATE metadata SET value=?2 WHERE key=?1 AND value!=?2
                AND (json_extract(?2, '$.receipt.phase')!='queued' OR json_extract(value, '$.receipt.phase')='queued')",
        };
        Ok(conn.execute(sql, params![key, value])? != 0)
    })
}

#[cfg(test)]
mod tests {
    /// cas-8c4f: Commander showed a working /mcp as failed because the receipt
    /// write ran on a loop pass whose budget was spent, behind a 6 ms holder.
    #[test]
    fn receipt_write_outlasts_a_brief_holder_after_the_pass_budget_is_spent() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let shared = cas_store::shared_db::shared_connection(&root.join("cas.db")).unwrap();
        let (held_tx, held) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = shared.lock().unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(40));
        });
        held.recv().unwrap();
        let _spent = cas_store::wait_budget::bound_waits_until(std::time::Instant::now());
        let row = serde_json::json!({"receipt": {"device_id": "d", "client_ref": "r", "phase": "submitted"}, "at": chrono::Utc::now()});
        let written = super::write(&root, "session", "supervisor", &row, super::Write::Reserve);
        holder.join().unwrap();
        assert!(written.unwrap(), "a brief holder must not fail a receipt write");
    }
}
