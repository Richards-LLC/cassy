-- A factory session with one of each measured path and two worker-hours.
-- Also loaded by the store test against real initialized production tables.
UPDATE communication_observation_epoch SET started_at='2026-01-01T00:00:00Z';
INSERT INTO agents(id,name,role,registered_at,last_heartbeat,factory_session)
VALUES('fixture-worker','worker','worker','2026-01-02T00:00:00Z',
       '2026-01-02T02:00:00Z','fixture-session');
INSERT INTO prompt_queue(id,source,target,prompt,created_at,factory_session,highest_stage,
                         kind,wake_gate_declines,transport_delivered_at)
VALUES(1,'supervisor','worker','one body','2026-01-02T00:00:01Z','fixture-session',
       'gated',NULL,1,'2026-01-02T00:00:02Z'),
      (2,'director','worker','nudge','2026-01-02T00:01:00Z','fixture-session',
       'delivered','worker_stall_nudge',0,'2026-01-02T00:01:02Z'),
      (3,'supervisor','gone-worker','ended unread','2026-01-02T00:02:00Z',
       'fixture-session','abandoned',NULL,0,NULL);
UPDATE prompt_queue SET last_pending_reason='recipient_ended_unread' WHERE id=3;
INSERT INTO prompt_queue_recipient_seen(prompt_id,recipient,seen_at,source)
VALUES(1,'worker','2026-01-02T00:00:02Z','hook_surfaced');
INSERT INTO communication_body_observations VALUES(1,'worker',1);
INSERT INTO operator_delivery_outbox(event_id,prompt_id,factory_session,payload_snapshot,
                                   created_at,relay_receipt,retained_at)
VALUES('fixture-event',1,'fixture-session','{}','2026-01-02T00:00:01Z',
       'fixture-receipt','2026-01-02T00:00:02Z');
