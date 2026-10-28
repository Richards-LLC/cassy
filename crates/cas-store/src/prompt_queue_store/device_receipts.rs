//! Authenticated application persistence is distinct from transport and read.
use super::*;

pub const OPERATOR_REPLY_RECEIPTS_SCHEMA_STATEMENTS: &[&str] = &[
    r#"
CREATE TABLE IF NOT EXISTS operator_reply_device_receipts (
    prompt_id INTEGER NOT NULL REFERENCES prompt_queue(id) ON DELETE CASCADE,
    factory_session TEXT NOT NULL,
    device_id TEXT NOT NULL CHECK(length(device_id) BETWEEN 1 AND 128),
    persisted_at TEXT NOT NULL,
    PRIMARY KEY (prompt_id, factory_session, device_id)
);
"#,
    r#"CREATE INDEX IF NOT EXISTS idx_operator_reply_receipt_device
    ON operator_reply_device_receipts(factory_session, device_id, prompt_id);
"#,
];

impl SqlitePromptQueueStore {
    pub(super) fn persist_operator_receipt(
        &self,
        prompt_id: i64,
        factory_session: &str,
        device_id: &str,
        notice: bool,
    ) -> Result<()> {
        if device_id.is_empty() || device_id.len() > 128 || device_id == "*" {
            return Err(StoreError::Other(
                "application receipt needs one authenticated device".into(),
            ));
        }
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            // Match the daemon's authoritative notice classification, not
            // browser-supplied metadata. Neither receipt can consume the other
            // lane, even if the client supplies a valid queue ID.
            let row: Option<(Option<String>, Option<String>)> = tx.query_row(
                "SELECT recipient_device_id, processed_at FROM prompt_queue
                 WHERE id=? AND factory_session=? AND lower(target)='operator'
                   AND (source != 'supervisor' AND COALESCE(origin_kind, '') = 'daemon') = ?",
                params![prompt_id, factory_session, notice], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            let Some((recipient, processed_at)) = row else {
                return Err(StoreError::Other(
                    "application receipt does not match this session's operator row class".into(),
                ));
            };
            tx.execute(
                "INSERT INTO operator_reply_device_receipts(prompt_id,factory_session,device_id,persisted_at) VALUES (?,?,?,?) ON CONFLICT DO NOTHING",
                params![prompt_id, factory_session, device_id, Utc::now().to_rfc3339()],
            )?;
            // Every authenticated viewer can store a reply. Only its addressee
            // (or one viewer of a legacy broadcast) closes the delivery lane.
            // Other devices replay durable history, with independent receipts.
            if processed_at.is_none() && recipient.as_deref().is_none_or(|recipient| recipient == device_id) {
                Self::atomic_stage_stamp_in_tx(
                    &tx,
                    prompt_id,
                    DeliveryStage::Delivered,
                    AtomicStampOpts {
                        reason: None,
                        detail: None,
                        set_processed: true,
                        broadcast_attempted: None,
                        broadcast_succeeded: None,
                        broadcast_failed: None,
                    },
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_receipts_are_per_device_and_do_not_mark_operator_read() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqlitePromptQueueStore::open(directory.path()).unwrap();
        store.init().unwrap();
        let id = store
            .enqueue_with_session("supervisor", "operator", "reply", "session-a")
            .unwrap();
        store.stamp_recipient_device(id, "phone").unwrap();
        store
            .record_operator_reply_persisted(id, "session-a", "desktop")
            .unwrap();
        assert!(
            store
                .queued_prompt(id)
                .unwrap()
                .unwrap()
                .processed_at
                .is_none()
        );
        assert!(
            store
                .record_operator_reply_persisted(id, "session-b", "phone")
                .is_err()
        );
        assert!(
            store
                .record_operator_reply_persisted(id, "session-a", "*")
                .is_err()
        );
        store
            .record_operator_reply_persisted(id, "session-a", "phone")
            .unwrap();
        store
            .record_operator_reply_persisted(id, "session-a", "phone")
            .unwrap();
        let row = store.queued_prompt(id).unwrap().unwrap();
        assert!(row.processed_at.is_some());
        assert!(row.acked_at.is_none());
        // A conversation ACK must never consume a daemon attention notice.
        let EnqueueIdempotentResult::Created(notice) = store.enqueue_idempotent(
            "relay-watchdog", "operator", "notice", Some("session-a"), None, None,
            "notice", Some(&QueueOrigin::Daemon),
        ).unwrap() else { panic!("expected a new notice"); };
        assert!(store.record_operator_reply_persisted(notice, "session-a", "phone").is_err());
        assert!(store.queued_prompt(notice).unwrap().unwrap().processed_at.is_none());
        store.stamp_recipient_device(notice, "phone").unwrap();
        for (session, device) in [("session-b", "phone"), ("session-a", "*")] {
            assert!(store.record_operator_notice_persisted(notice, session, device).is_err());
        }
        store.record_operator_notice_persisted(notice, "session-a", "desktop").unwrap();
        assert!(store.queued_prompt(notice).unwrap().unwrap().processed_at.is_none());
        store.record_operator_notice_persisted(notice, "session-a", "phone").unwrap();
        let saved = store.queued_prompt(notice).unwrap().unwrap();
        assert!(saved.processed_at.is_some());
        assert!(saved.acked_at.is_none());
        store.record_operator_notice_persisted(notice, "session-a", "phone").unwrap();
        assert_eq!(store.queued_prompt(notice).unwrap().unwrap().processed_at, saved.processed_at);
        assert!(SqlitePromptQueueStore::open(directory.path()).unwrap().peek_operator_replies("session-a", 10).unwrap().is_empty());
        let conn = crate::shared_db::lock_connection(&store.conn).unwrap();
        let receipts: i64 = conn
            .query_row(
                "SELECT count(*) FROM operator_reply_device_receipts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receipts, 4);
    }

    #[test]
    fn receipt_classes_follow_durable_provenance_in_both_directions() {
        let directory = tempfile::tempdir().unwrap();
        let store = SqlitePromptQueueStore::open(directory.path()).unwrap();
        store.init().unwrap();
        for (source, origin, notice) in [
            ("supervisor", None, false),
            ("supervisor", Some(QueueOrigin::Daemon), false),
            ("relay-watchdog", None, false),
            ("relay-watchdog", Some(QueueOrigin::Daemon), true),
        ] {
            for target in ["operator", "supervisor"] {
                let id = store.enqueue_urgent_with_outcome(source, target, &format!("receipt classification {origin:?}"),
                    Some("session"), None, None, false, origin.as_ref()).unwrap().id();
                let wrong = if notice { store.record_operator_reply_persisted(id, "session", "phone") }
                    else { store.record_operator_notice_persisted(id, "session", "phone") };
                assert!(wrong.is_err(), "{source}/{origin:?}/{target} accepted the other receipt class");
                let right = if notice { store.record_operator_notice_persisted(id, "session", "phone") }
                    else { store.record_operator_reply_persisted(id, "session", "phone") };
                assert_eq!(right.is_ok(), target == "operator", "{source}/{origin:?}/{target}");
                let row = store.queued_prompt(id).unwrap().unwrap();
                assert_eq!(row.processed_at.is_some(), target == "operator");
                assert!(row.acked_at.is_none());
            }
        }
    }

}
