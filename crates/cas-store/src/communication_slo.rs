//! Store-backed communication SLOs (cas-7e24).

#[cfg(test)]
mod tests {
    use crate::{PromptQueueStore, SqlitePromptQueueStore};

    #[test]
    fn cas_7e24_false_abandon_pass_and_breach() {
        let temp = tempfile::tempdir().unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let id = queue.enqueue_with_session("supervisor", "worker", "body", "session").unwrap();
        queue.surface_unseen_for_recipient("worker", Some("session"), 10).unwrap();
        let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        assert_eq!(super::false_abandon(&conn, "session"), 0);
        conn.execute("UPDATE prompt_queue SET highest_stage='abandoned', last_pending_reason='abandoned_unknown_target' WHERE id=?", [id]).unwrap();
        assert_eq!(super::false_abandon(&conn, "session"), 1,
            "a surfaced body labelled abandoned is an SLO breach");
    }
}

#[cfg(test)]
fn false_abandon(_: &rusqlite::Connection, _: &str) -> u64 { 0 }
