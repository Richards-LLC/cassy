//! Recipient intent is separate from body observations and transport claims.
use super::*;

/// A renderable copy. References contain no recoverable original body; callers
/// cannot accidentally expand a repeat by choosing a different renderer.
#[derive(Debug, Clone)]
pub enum RecipientDelivery {
    Full(QueuedPrompt),
    Reference(QueuedPrompt),
}

impl RecipientDelivery {
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full(_))
    }

    pub fn into_prompt(self) -> QueuedPrompt {
        match self {
            Self::Full(row) | Self::Reference(row) => row,
        }
    }
}

impl SqlitePromptQueueStore {
    pub(super) fn recipient_delivery_on_conn(
        conn: &Connection,
        mut row: QueuedPrompt,
        recipient: &str,
        named_read: bool,
    ) -> Result<RecipientDelivery> {
        let terminal: Option<String> = conn.query_row(
            "SELECT disposition FROM prompt_queue_recipient_ack
             WHERE prompt_id = ?1 AND recipient = ?2",
            params![row.id, recipient], |r| r.get(0),
        ).optional()?;
        // A legacy direct ACK applies only to its addressed recipient. Broadcast
        // row-level ACKs never stand in for any recipient's own confirmation.
        let legacy_ack: bool = conn.query_row(
            "SELECT acked_at IS NOT NULL AND acked_via IN ('explicit_ack','hook_surfaced')
             FROM prompt_queue WHERE id = ?1", [row.id], |r| r.get::<_, Option<bool>>(0),
        )?.unwrap_or(false);
        let terminal = terminal.or_else(|| {
            (row.target != "all_workers" && legacy_ack)
                .then(|| "acknowledged".to_string())
        });
        let body_seen: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM prompt_queue_recipient_seen
             WHERE prompt_id = ?1 AND recipient = ?2
               AND source IN ('inbox_poll','hook_surfaced','observed_wake'))",
            params![row.id, recipient], |r| r.get(0),
        )?;
        let status = if let Some(terminal) = terminal {
            terminal
        } else if row.urgent || row.priority == NotificationPriority::Critical
            || !body_seen || named_read
        {
            return Ok(RecipientDelivery::Full(row));
        } else {
            "already surfaced; not acknowledged".to_string()
        };
        row.reference_only = true;
        row.prompt = format!("Message {}: {status}. No new body; delivery is not evidence of task action.", row.id);
        // A summary can itself contain the original message. Keep the reference
        // bounded even when callers print both summary and prompt.
        row.summary = Some(format!("Message {}: {status}", row.id));
        Ok(RecipientDelivery::Reference(row))
    }

    pub(super) fn settle_recipient_message(
        &self, prompt_id: i64, recipients: &[String], reply_prompt_id: Option<i64>,
    ) -> Result<()> {
        if recipients.is_empty() {
            return Err(StoreError::Other("message acknowledgement requires a recipient".into()));
        }
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            let row = Self::queued_prompt_on_conn(&tx, prompt_id)?
                .ok_or_else(|| StoreError::Other("message does not exist".into()))?;
            if row.target != "all_workers" && !recipients.iter().any(|r| r == &row.target) {
                return Err(StoreError::Other("message is not addressed to this recipient".into()));
            }
            if let Some(reply_id) = reply_prompt_id {
                let reply = Self::queued_prompt_on_conn(&tx, reply_id)?
                    .ok_or_else(|| StoreError::Other("reply has not been durably queued".into()))?;
                if !recipients.contains(&reply.source) {
                    return Err(StoreError::Other("reply endpoints do not match message".into()));
                }
            }
            let now = Utc::now().to_rfc3339();
            for recipient in recipients {
                tx.execute(
                    "INSERT INTO prompt_queue_recipient_ack
                     (prompt_id,recipient,settled_at,disposition,reply_prompt_id)
                     VALUES (?1,?2,?3,?4,?5)
                     ON CONFLICT(prompt_id,recipient) DO UPDATE SET
                       disposition = CASE WHEN prompt_queue_recipient_ack.reply_prompt_id IS NOT NULL
                           THEN 'answered' ELSE excluded.disposition END,
                       reply_prompt_id = COALESCE(excluded.reply_prompt_id, reply_prompt_id)",
                    params![prompt_id, recipient, now,
                        if reply_prompt_id.is_some() { "answered" } else { "acknowledged" }, reply_prompt_id],
                )?;
            }
            if row.target != "all_workers" {
                tx.execute("UPDATE prompt_queue SET acked_at = COALESCE(acked_at, ?1),
                    acked_via = 'explicit_ack' WHERE id = ?2", params![now, prompt_id])?;
                let _ = Self::atomic_stage_stamp_in_tx(&tx, prompt_id,
                    DeliveryStage::Confirmed, AtomicStampOpts::clear_reason());
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
    fn recipient_state_survives_restart_without_inventing_body_observations() {
        let temp = tempfile::tempdir().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let body = "Named details of the work.\n".repeat(100);
        let id = store.enqueue("supervisor", "worker-a", &body).unwrap();
        // A handoff alone never replaces the first body with a reference.
        store.mark_transport_delivered(id).unwrap();
        assert!(store.recipient_delivery(id, "worker-a").unwrap().unwrap().is_full());
        assert_eq!(store.poll_unseen_for_recipient("worker-a", None, 1).unwrap()[0].prompt, body);
        let report = store.message_delivery_report(id).unwrap().unwrap();
        let reference = store.surface_for_recipient_within_budget("worker-a", None,
            HookDrain::PolledBefore(Utc::now() + chrono::Duration::seconds(1)), 1, 500).unwrap();
        assert_eq!(reference.rows.len(), 1, "budget the notice, not the old body");
        assert!(!reference.rows[0].prompt.contains("Named details"));
        assert!(reference.rows[0].prompt.contains("not acknowledged"));
        assert_eq!(store.recipient_receipt(id, "worker-a").unwrap(), Some(SurfacingSource::InboxPoll));
        let observations: i64 = store.conn.lock().unwrap().query_row(
            "SELECT body_count FROM communication_body_observations WHERE prompt_id=?1 AND recipient='worker-a'",
            [id], |r| r.get(0)).unwrap();
        assert_eq!(observations, 1, "references never count as body observations");
        assert_eq!(store.message_delivery_report(id).unwrap().unwrap().confirmation_source, report.confirmation_source);
        assert!(store.surface_for_recipient_within_budget("worker-a", None,
            HookDrain::PolledBefore(Utc::now() + chrono::Duration::seconds(2)), 1, 500).unwrap().rows.is_empty());
        drop(store);
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        assert!(!store.recipient_delivery(id, "worker-a").unwrap().unwrap().is_full());
        store.acknowledge_for_recipients(id, &["worker-a".into()], None).unwrap();
        let named = store.poll_named_for_recipient("worker-a", None, &[id]).unwrap();
        assert!(!named[0].prompt.contains("Named details"));
        assert!(named[0].prompt.contains("acknowledged"));
        assert!(store.poll_unseen_for_recipient("worker-a", None, 10).unwrap().is_empty());
        drop(store);
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        assert!(store.recipient_delivery(id, "worker-a").unwrap().unwrap().into_prompt().prompt.contains("acknowledged"));
        store.conn.lock().unwrap().execute("DELETE FROM prompt_queue WHERE id = ?1", [id]).unwrap();
        let retained: i64 = store.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM prompt_queue_recipient_ack WHERE prompt_id = ?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(retained, 0, "retention prunes recipient intent with its message");
    }

    #[test]
    fn recipient_intent_precedes_criticality_and_isolated_broadcast_copies() {
        let temp = tempfile::tempdir().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        for (urgent, priority) in [(true, NotificationPriority::Normal), (false, NotificationPriority::Critical)] {
            let id = store.enqueue_urgent("supervisor", "worker-a", "Unresolved critical instruction",
                None, None, Some(priority), urgent).unwrap();
            store.poll_unseen_for_recipient("worker-a", None, 10).unwrap();
            assert!(store.recipient_delivery(id, "worker-a").unwrap().unwrap().is_full());
            let reply = store.enqueue("worker-a", "supervisor", "Actual answer").unwrap();
            store.acknowledge_for_recipients(id, &["worker-a".into()], Some(reply)).unwrap();
            let delivery = store.recipient_delivery(id, "worker-a").unwrap().unwrap();
            assert!(!delivery.is_full());
            assert!(delivery.into_prompt().prompt.contains("answered"));
            store.acknowledge_for_recipients(id, &["worker-a".into()], None).unwrap();
            assert!(store.recipient_delivery(id, "worker-a").unwrap().unwrap().into_prompt().prompt.contains("answered"),
                "a later ACK must preserve the stronger actual answer");
            let named = store.poll_named_for_recipient("worker-a", None, &[id]).unwrap();
            assert!(!named[0].prompt.contains("Unresolved critical instruction"));
        }
        let id = store.enqueue("supervisor", "all_workers", "Shared instruction").unwrap();
        store.acknowledge_for_recipients(id, &["worker-a".into()], None).unwrap();
        assert_eq!(store.recipient_receipt(id, "worker-a").unwrap(), None, "ACK before read invents no observation");
        assert!(store.poll_unseen_for_recipient("worker-a", None, 10).unwrap().is_empty());
        assert_eq!(store.poll_unseen_for_recipient("worker-b", None, 10).unwrap()[0].prompt, "Shared instruction");
        assert!(!store.recipient_delivery(id, "worker-a").unwrap().unwrap().is_full());
        assert!(store.acknowledge_for_recipients(id + 1000, &["worker-a".into()], None).is_err());
        let direct = store.enqueue("supervisor", "worker-b", "Private instruction").unwrap();
        assert!(store.acknowledge_for_recipients(direct, &["worker-a".into()], None).is_err());
        assert!(store.acknowledge_for_recipients(direct, &["worker-b".into()], Some(id + 1000)).is_err());
    }
}
