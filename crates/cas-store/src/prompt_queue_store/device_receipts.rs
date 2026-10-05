//! Authenticated application persistence is distinct from transport and read.
use super::*;

pub const OPERATOR_REPLY_RECEIPTS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS operator_reply_device_receipts (
    prompt_id INTEGER NOT NULL REFERENCES prompt_queue(id) ON DELETE CASCADE,
    factory_session TEXT NOT NULL,
    device_id TEXT NOT NULL CHECK(length(device_id) BETWEEN 1 AND 128),
    persisted_at TEXT NOT NULL,
    PRIMARY KEY (prompt_id, factory_session, device_id)
);
CREATE INDEX IF NOT EXISTS idx_operator_reply_receipt_device
    ON operator_reply_device_receipts(factory_session, device_id, prompt_id);
"#;

impl SqlitePromptQueueStore {
    pub(super) fn persist_operator_reply_receipt(
        &self,
        prompt_id: i64,
        factory_session: &str,
        device_id: &str,
    ) -> Result<()> {
        if device_id.is_empty() || device_id.len() > 128 || device_id == "*" {
            return Err(StoreError::Other(
                "application receipt needs one authenticated device".into(),
            ));
        }
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            let recipient: Option<Option<String>> = tx.query_row(
                "SELECT recipient_device_id FROM prompt_queue WHERE id=? AND factory_session=? AND lower(target)='operator'",
                params![prompt_id, factory_session], |row| row.get(0),
            ).optional()?;
            let Some(recipient) = recipient else {
                return Err(StoreError::Other(
                    "application receipt does not belong to this session's operator reply".into(),
                ));
            };
            tx.execute(
                "INSERT INTO operator_reply_device_receipts(prompt_id,factory_session,device_id,persisted_at) VALUES (?,?,?,?) ON CONFLICT DO NOTHING",
                params![prompt_id, factory_session, device_id, Utc::now().to_rfc3339()],
            )?;
            // Every authenticated viewer can store a reply. Only its addressee
            // (or one viewer of a legacy broadcast) closes the delivery lane.
            // Other devices replay durable history, with independent receipts.
            if recipient
                .as_deref()
                .is_none_or(|recipient| recipient == device_id)
            {
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
        let conn = crate::shared_db::lock_connection(&store.conn).unwrap();
        let receipts: i64 = conn
            .query_row(
                "SELECT count(*) FROM operator_reply_device_receipts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receipts, 2);
    }
}
