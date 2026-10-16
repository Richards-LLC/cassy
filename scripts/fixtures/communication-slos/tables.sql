-- Minimal projection of existing tables; new schema is loaded from production.
CREATE TABLE prompt_queue(id INTEGER PRIMARY KEY,source TEXT,target TEXT,prompt TEXT,
 created_at TEXT,factory_session TEXT,highest_stage TEXT,last_pending_reason TEXT,
 kind TEXT,wake_gate_declines INTEGER,acked_at TEXT,transport_delivered_at TEXT);
CREATE TABLE prompt_queue_recipient_seen(prompt_id INTEGER,recipient TEXT,seen_at TEXT,source TEXT);
CREATE TABLE operator_delivery_outbox(event_id TEXT,prompt_id INTEGER,factory_session TEXT,
 payload_snapshot TEXT,created_at TEXT,relay_receipt TEXT,retained_at TEXT);
CREATE TABLE agents(id TEXT,name TEXT,role TEXT,status TEXT DEFAULT 'active',
 registered_at TEXT,last_heartbeat TEXT,factory_session TEXT);
CREATE TABLE events(id INTEGER PRIMARY KEY,event_type TEXT,entity_id TEXT,metadata TEXT,created_at TEXT);
