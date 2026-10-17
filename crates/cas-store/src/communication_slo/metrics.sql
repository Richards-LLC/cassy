-- cas-447d xs.sql / gate.sql + review §2.4; dups.py groups body reads by id.
-- ?1 = exact factory session; ?2 = observation time (RFC3339 UTC).
WITH messages AS (
    SELECT * FROM prompt_queue WHERE factory_session = ?1
), surfaced AS (
    SELECT s.* FROM prompt_queue_recipient_seen s JOIN messages p ON p.id=s.prompt_id
    WHERE s.source IN ('hook_surfaced', 'inbox_poll', 'observed_wake')
      AND (s.recipient=p.target OR p.target='all_workers')
), retirements AS (
    SELECT entity_id id,
      json_extract(metadata, '$.retired_agent.registered_at') started_at,
      created_at ended_at
    FROM events WHERE event_type='agent_shutdown' AND json_valid(metadata)
      AND json_extract(metadata, '$.retired_agent.factory_session')=?1
      AND json_extract(metadata, '$.retired_agent.role')='worker'
), workers AS (
    SELECT id, registered_at started_at,
      CASE WHEN status IN ('shutdown','dead','stale') THEN last_heartbeat ELSE ?2 END ended_at
    FROM agents WHERE factory_session=?1 AND role='worker'
    UNION ALL SELECT id, started_at, ended_at FROM retirements
), exposure AS (
    -- A retained live row and its retirement event may coexist during shutdown.
    SELECT id, started_at, MIN(ended_at) ended_at FROM workers GROUP BY id, started_at
)
SELECT
    (SELECT COUNT(*) FROM messages p
      WHERE (highest_stage='abandoned' OR last_pending_reason='abandoned_unknown_target')
        AND last_pending_reason IS NOT 'recipient_ended_unread'
        AND EXISTS(SELECT 1 FROM surfaced s WHERE s.prompt_id=p.id)) false_abandon,
    -- cas-5c12: only the enrolled cloud lane owes a relay receipt. Its row is
    -- written with the local snapshot while a binding is active and is deleted
    -- only with that snapshot, so a local row without one was never enrolled.
    -- A receipt proves the cloud retained the event, not that anyone read it;
    -- an expired tombstone is not retention.
    (SELECT COUNT(*) FROM operator_cloud_outbox c JOIN operator_delivery_outbox o
      ON o.event_id=c.event_id WHERE o.factory_session=?1) operator_events,
    (SELECT COUNT(*) FROM operator_cloud_outbox c JOIN operator_delivery_outbox o
      ON o.event_id=c.event_id WHERE o.factory_session=?1
      AND c.receipt_kind IN ('stored','duplicate')) operator_receipts,
    (SELECT COUNT(*) FROM operator_delivery_outbox o WHERE o.factory_session=?1
      AND NOT EXISTS(SELECT 1 FROM operator_cloud_outbox c WHERE c.event_id=o.event_id))
      operator_unenrolled,
    (SELECT COALESCE(SUM(o.body_count-1),0) FROM communication_body_observations o
      JOIN messages p ON p.id=o.prompt_id) duplicate_body_reads,
    (SELECT COUNT(*) FROM messages WHERE source='director' AND kind='worker_stall_nudge') director_nudges,
    (SELECT COALESCE(SUM(MAX(0.0, (MIN(julianday(ended_at),julianday(?2))
      - julianday(started_at))*24.0)),0.0) FROM exposure) worker_hours,
    (SELECT COUNT(*) FROM messages p WHERE wake_gate_declines>0
      AND acked_at IS NULL AND transport_delivered_at IS NULL
      AND COALESCE(highest_stage,'') NOT IN
          ('abandoned','dropped','suppressed','superseded','confirmed')
      AND last_pending_reason IS NOT 'recipient_ended_unread'
      AND NOT EXISTS(SELECT 1 FROM surfaced s WHERE s.prompt_id=p.id)) gated_without_outcome,
    (SELECT started_at FROM communication_observation_epoch WHERE singleton=1) body_observed_since,
    (SELECT CASE WHEN MIN(julianday(created_at)) IS NULL THEN 1 ELSE
      MIN(julianday(created_at)) >=
       (SELECT julianday(started_at) FROM communication_observation_epoch WHERE singleton=1)
     END FROM messages) body_coverage_complete;
