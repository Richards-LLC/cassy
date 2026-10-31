//! Canonical queue schema and metadata for deferred runtime compatibility.
use super::*;

// A reserved, versioned snapshot. CREATE deliberately refuses name collisions;
// copying metadata must never overwrite a previous snapshot or unrelated table.
const LEGACY_PROCESSED_SNAPSHOT_SCHEMA: &str = "CREATE TABLE cas_prompt_queue_legacy_processed (
    prompt_id INTEGER PRIMARY KEY,
    processed_at TEXT NOT NULL,
    snapshot_kind TEXT NOT NULL DEFAULT 'cas-24d3:m279'
        CHECK(snapshot_kind = 'cas-24d3:m279')
)";

// This freezes the shape required by doctor SLO/relay/retry readers for m279.
// Missing queues are intentionally lazy. Existing queues must have all these
// columns before the migration can be detected as complete.
pub const PROMPT_QUEUE_HEALTH_SCHEMA_DETECT: &str = "SELECT CASE WHEN
    NOT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='prompt_queue')
    OR (SELECT COUNT(*) FROM pragma_table_info('prompt_queue') WHERE name IN
        ('kind','highest_stage','delivery_attempts','first_attempt_at','factory_session',
         'summary','last_pending_reason','last_pending_detail','transport_delivered_at',
         'wake_gate_declines','acked_at','id','source','target','prompt','created_at','processed_at'))=17
    THEN 1 ELSE 0 END";

/// Inspect only; SQLite failures remain errors rather than proving absence.
pub fn missing_prompt_queue_columns(conn: &Connection) -> Result<Vec<String>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='prompt_queue')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare("PRAGMA table_info(prompt_queue)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    Ok([
        "kind",
        "highest_stage",
        "delivery_attempts",
        "first_attempt_at",
        "factory_session",
        "summary",
        "last_pending_reason",
        "last_pending_detail",
        "transport_delivered_at",
        "wake_gate_declines",
        "acked_at",
        "id",
        "source",
        "target",
        "prompt",
        "created_at",
        "processed_at",
    ]
    .into_iter()
    .filter(|name| !columns.contains(*name))
    .map(str::to_owned)
    .collect())
}

/// Explicit repair and compatibility metadata snapshot in the caller's transaction.
/// Original queue/receipt rows remain unchanged until runtime init.
pub fn repair_legacy_prompt_queue_schema(conn: &Connection) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='prompt_queue')",
        [],
        |row| row.get(0),
    )?;
    if exists {
        if conn.is_autocommit() {
            return Err(StoreError::Other(
                "prompt queue schema repair requires a caller-owned transaction".into(),
            ));
        }
        if !crate::shared_db::column_exists(conn, "prompt_queue", "highest_stage") {
            conn.execute_batch(LEGACY_PROCESSED_SNAPSHOT_SCHEMA)?;
            conn.execute(
                "INSERT INTO cas_prompt_queue_legacy_processed(prompt_id,processed_at)
                 SELECT id,processed_at FROM prompt_queue WHERE processed_at IS NOT NULL",
                [],
            )?;
        }
        ensure_schema(conn)?;
    }
    Ok(())
}

/// Validate ownership before consuming metadata, including empty snapshots.
/// Unknown objects or versions stay untouched rather than becoming evidence.
fn legacy_processed_pending(conn: &Connection) -> Result<bool> {
    let kind: Option<String> = conn
        .query_row(
            "SELECT type FROM sqlite_master WHERE name='cas_prompt_queue_legacy_processed'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(kind) = kind else {
        return Ok(false);
    };
    let mut stmt = conn.prepare("PRAGMA table_info(cas_prompt_queue_legacy_processed)")?;
    let columns = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, Option<String>>(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let triggers: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'
         AND tbl_name='cas_prompt_queue_legacy_processed'",
        [],
        |row| row.get(0),
    )?;
    if kind != "table"
        || columns
            != [
                ("prompt_id".into(), None),
                ("processed_at".into(), None),
                ("snapshot_kind".into(), Some("'cas-24d3:m279'".into())),
            ]
        || triggers != 0
    {
        return Err(StoreError::Other(
            "cannot consume unrecognized cas_prompt_queue_legacy_processed metadata; inspect the reserved snapshot before retrying".into(),
        ));
    }
    let wrong_version: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM cas_prompt_queue_legacy_processed
         WHERE snapshot_kind IS NOT 'cas-24d3:m279')",
        [],
        |row| row.get(0),
    )?;
    if wrong_version {
        return Err(StoreError::Other(
            "unsupported legacy processed snapshot version".into(),
        ));
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM cas_prompt_queue_legacy_processed)",
        [],
        |row| row.get(0),
    )?)
}

/// Runtime-only compatibility, not a schema repair or a new acknowledgement.
pub(super) fn hydrate_legacy_processed(conn: &Connection) -> Result<()> {
    // Avoid even a no-op write lock on repeated/current init (cas-1bc7).
    if !legacy_processed_pending(conn)? {
        return Ok(());
    }
    let tx = ImmediateTx::new(conn)?;
    // A concurrent initializer may have consumed the snapshot while we waited.
    if legacy_processed_pending(&tx)? {
        tx.execute(
            "UPDATE prompt_queue AS queue SET highest_stage='delivered',
                transport_delivered_at=legacy.processed_at
             FROM cas_prompt_queue_legacy_processed AS legacy
             WHERE queue.id=legacy.prompt_id AND queue.processed_at=legacy.processed_at
               AND queue.highest_stage IS NULL",
            [],
        )?;
        // Consume stale and already-advanced candidates too. A later timestamp
        // reversal or stage change must never reactivate old evidence.
        tx.execute("DELETE FROM cas_prompt_queue_legacy_processed", [])?;
    }
    tx.commit()?;
    Ok(())
}

/// Canonical schema creation for runtime init and explicit legacy repair.
/// DDL only: no message/receipt writes, lifecycle hydration or demotion.
pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(PROMPT_QUEUE_SCHEMA)?;
    crate::communication_slo::ensure_schema(&conn)?;
    for sql in OPERATOR_REPLY_RECEIPTS_SCHEMA_STATEMENTS {
        conn.execute(sql, [])?;
    }

    for (col, mig) in [
        ("factory_session", PROMPT_QUEUE_SESSION_MIGRATION),
        ("summary", PROMPT_QUEUE_SUMMARY_MIGRATION),
        ("priority", PROMPT_QUEUE_PRIORITY_MIGRATION),
        ("acked_at", PROMPT_QUEUE_ACKED_AT_MIGRATION),
        ("urgent", PROMPT_QUEUE_URGENT_MIGRATION),
        ("attribution_json", PROMPT_QUEUE_ATTRIBUTION_MIGRATION),
        ("origin_agent_id", PROMPT_QUEUE_ORIGIN_MIGRATION),
        ("origin_kind", PROMPT_QUEUE_ORIGIN_KIND_MIGRATION),
        ("operator_label", PROMPT_QUEUE_OPERATOR_LABEL_MIGRATION),
        (
            "operator_device_id",
            PROMPT_QUEUE_OPERATOR_DEVICE_ID_MIGRATION,
        ),
        (
            "operator_device_label",
            PROMPT_QUEUE_OPERATOR_DEVICE_LABEL_MIGRATION,
        ),
        ("operator_scopes", PROMPT_QUEUE_OPERATOR_SCOPES_MIGRATION),
        (
            "operator_verified",
            PROMPT_QUEUE_OPERATOR_VERIFIED_MIGRATION,
        ),
        (
            "recipient_device_id",
            PROMPT_QUEUE_RECIPIENT_DEVICE_MIGRATION,
        ),
        ("kind", PROMPT_QUEUE_KIND_MIGRATION),
        ("attachments", PROMPT_QUEUE_ATTACHMENTS_MIGRATION),
        ("selected_at", PROMPT_QUEUE_SELECTED_AT_MIGRATION),
        ("last_pending_reason", PROMPT_QUEUE_PENDING_REASON_MIGRATION),
        ("last_pending_detail", PROMPT_QUEUE_PENDING_DETAIL_MIGRATION),
        ("holding_detail", PROMPT_QUEUE_HOLDING_DETAIL_MIGRATION),
        (
            "transport_delivered_at",
            PROMPT_QUEUE_TRANSPORT_DELIVERED_AT_MIGRATION,
        ),
        ("highest_stage", PROMPT_QUEUE_HIGHEST_STAGE_MIGRATION),
        (
            "broadcast_attempted",
            PROMPT_QUEUE_BROADCAST_ATTEMPTED_MIGRATION,
        ),
        (
            "broadcast_succeeded",
            PROMPT_QUEUE_BROADCAST_SUCCEEDED_MIGRATION,
        ),
        ("broadcast_failed", PROMPT_QUEUE_BROADCAST_FAILED_MIGRATION),
        (
            "delivery_attempts",
            PROMPT_QUEUE_DELIVERY_ATTEMPTS_MIGRATION,
        ),
        ("next_attempt_at", PROMPT_QUEUE_NEXT_ATTEMPT_AT_MIGRATION),
        ("first_attempt_at", PROMPT_QUEUE_FIRST_ATTEMPT_AT_MIGRATION),
        ("acked_via", PROMPT_QUEUE_ACKED_VIA_MIGRATION),
        ("assumed_seen_at", PROMPT_QUEUE_ASSUMED_SEEN_AT_MIGRATION),
        ("supersede_key", PROMPT_QUEUE_SUPERSEDE_KEY_MIGRATION),
        ("sticky", PROMPT_QUEUE_STICKY_MIGRATION),
        ("dedupe_key", PROMPT_QUEUE_DEDUPE_KEY_MIGRATION),
        ("wake_attempt", PROMPT_QUEUE_WAKE_ATTEMPT_MIGRATION),
        ("wake_attempt_at", PROMPT_QUEUE_WAKE_ATTEMPT_AT_MIGRATION),
        (
            "wake_attempt_detail",
            PROMPT_QUEUE_WAKE_ATTEMPT_DETAIL_MIGRATION,
        ),
        (
            "wake_gate_declines",
            PROMPT_QUEUE_WAKE_GATE_DECLINES_MIGRATION,
        ),
        (
            "delivery_stalled_notified_at",
            PROMPT_QUEUE_DELIVERY_STALLED_NOTIFIED_AT_MIGRATION,
        ),
        (
            "deferred_inbox_at",
            PROMPT_QUEUE_DEFERRED_INBOX_AT_MIGRATION,
        ),
        (
            "deferred_inbox_bytes",
            PROMPT_QUEUE_DEFERRED_INBOX_BYTES_MIGRATION,
        ),
    ] {
        crate::shared_db::ensure_column(&conn, "prompt_queue", col, mig)?;
    }

    // cas-7a01 (GH #155): the receipt table predates the surfacing
    // path, so existing databases need the provenance column added.
    crate::shared_db::ensure_column(
        &conn,
        "prompt_queue_recipient_seen",
        "source",
        PROMPT_QUEUE_RECIPIENT_SEEN_SOURCE_MIGRATION,
    )?;
    crate::shared_db::ensure_column(
        &conn,
        "prompt_queue_recipient_seen",
        "teams_body_fallback_at",
        PROMPT_QUEUE_RECIPIENT_SEEN_TEAMS_BODY_FALLBACK_MIGRATION,
    )?;
    crate::shared_db::ensure_column(
        &conn,
        "prompt_queue_recipient_seen",
        "poll_settled",
        PROMPT_QUEUE_RECIPIENT_SEEN_POLL_SETTLED_MIGRATION,
    )?;
    crate::shared_db::ensure_column(
        &conn,
        "prompt_queue_recipient_seen",
        "delivered_at",
        PROMPT_QUEUE_RECIPIENT_SEEN_DELIVERED_MIGRATION,
    )?;
    crate::shared_db::ensure_column(
        &conn,
        "prompt_queue_recipient_transport",
        "device_id",
        PROMPT_QUEUE_RECIPIENT_TRANSPORT_DEVICE_MIGRATION,
    )?;

    // Indexes are IF NOT EXISTS — safe under concurrency once columns exist.
    conn.execute_batch(PROMPT_QUEUE_TWO_LANE_INDEXES)?;
    conn.execute_batch(PROMPT_QUEUE_DEDUPE_KEY_INDEX)?;
    conn.execute_batch(PROMPT_QUEUE_MESSAGE_HOT_PATH_INDEXES_MIGRATION)?;
    conn.execute_batch(PROMPT_QUEUE_SESSION_HISTORY_INDEX)?;
    conn.execute_batch(PROMPT_QUEUE_MAINTENANCE_INDEXES)?;
    for statement in OPERATOR_DELIVERY_SCHEMA_STATEMENTS {
        conn.execute_batch(statement)?;
    }
    for statement in OPERATOR_CLOUD_SCHEMA_STATEMENTS {
        conn.execute_batch(statement)?;
    }
    Ok(())
}
