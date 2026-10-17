//! Canonical prompt-queue DDL, separated from runtime receipt backfills.
use super::*;

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

/// Explicit migration repair on the caller-owned connection/transaction.
/// Do not construct a queue store: init also changes historical receipts.
pub fn repair_legacy_prompt_queue_schema(conn: &Connection) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='prompt_queue')",
        [],
        |row| row.get(0),
    )?;
    if exists {
        ensure_schema(conn)?;
    }
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
    for statement in OPERATOR_DELIVERY_SCHEMA_STATEMENTS {
        conn.execute_batch(statement)?;
    }
    for statement in OPERATOR_CLOUD_SCHEMA_STATEMENTS {
        conn.execute_batch(statement)?;
    }
    Ok(())
}
