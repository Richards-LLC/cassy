#!/usr/bin/env bash
# cas-9d92 — factory communication + efficiency mining.
#
# SAFETY: never touches the live DB. Snapshot first:
#   cp .cas/cas.db  /tmp/cas-mining/snap.db
#   cp .cas/cas.db-wal /tmp/cas-mining/snap.db-wal
#   cp .cas/cas.db-shm /tmp/cas-mining/snap.db-shm
#   sqlite3 /tmp/cas-mining/snap.db "PRAGMA integrity_check;"   # expect: ok
# (`sqlite3 .backup` stalls forever here: the daemon writes continuously and
# each write restarts the backup. A plain fs copy of db+wal+shm is read-only
# w.r.t. the source and replays the WAL locally on first open.)
#
# Usage: mine_comm_efficiency.sh [snapshot.db]
set -euo pipefail
DB="${1:-/tmp/cas-mining/snap.db}"
q() { sqlite3 -header -column "$DB" "$1"; }

echo "############ F1: supervisor_queue relay delivery by event_type (silent-relay sweep, FULL history)"
q "SELECT event_type, COUNT(*) n, SUM(prompt_delivered_at IS NULL) never_delivered,
     ROUND(100.0*SUM(prompt_delivered_at IS NULL)/COUNT(*),1) pct,
     MIN(date(created_at)) first, MAX(date(created_at)) last
   FROM supervisor_queue GROUP BY 1 ORDER BY never_delivered DESC;"

echo "############ F2: prompt_queue delivery latency percentiles (created -> transport_delivered), seconds"
q "WITH l AS (SELECT (julianday(transport_delivered_at)-julianday(created_at))*86400 s
             FROM prompt_queue WHERE transport_delivered_at IS NOT NULL)
   SELECT COUNT(*) n, ROUND(AVG(s),2) avg,
     ROUND((SELECT s FROM l ORDER BY s LIMIT 1 OFFSET (SELECT COUNT(*)*50/100 FROM l)),2) p50,
     ROUND((SELECT s FROM l ORDER BY s LIMIT 1 OFFSET (SELECT COUNT(*)*90/100 FROM l)),2) p90,
     ROUND((SELECT s FROM l ORDER BY s LIMIT 1 OFFSET (SELECT COUNT(*)*99/100 FROM l)),2) p99,
     ROUND(MAX(s),2) max FROM l;"

echo "############ F3: terminal failure reasons (messages that never reached a human/agent)"
q "SELECT last_pending_reason, COUNT(*) n, MIN(date(created_at)) first, MAX(date(created_at)) last
   FROM prompt_queue WHERE last_pending_reason IS NOT NULL
   GROUP BY 1 ORDER BY n DESC;"

echo "############ F4: undelivered rate by day (last 21 active days)"
q "SELECT date(created_at) d, COUNT(*) n, SUM(transport_delivered_at IS NULL) undelivered,
     ROUND(100.0*SUM(transport_delivered_at IS NULL)/COUNT(*),1) pct, SUM(urgent) urgent
   FROM prompt_queue GROUP BY 1 ORDER BY d DESC LIMIT 21;"

echo "############ F5: misrouted messages — target never existed as an agent"
q "SELECT p.target, COUNT(*) n, MIN(date(p.created_at)) first, MAX(date(p.created_at)) last
   FROM prompt_queue p
   WHERE p.target NOT IN (SELECT name FROM agents)
     AND p.target NOT IN (SELECT id FROM agents)
     AND p.target NOT IN ('supervisor','director','cas','all_workers')
   GROUP BY 1 ORDER BY n DESC LIMIT 25;"

echo "############ F6: supervisor re-ask rate — identical prompt text sent to same target 2+ times"
q "SELECT source, target, COUNT(*) repeats, MIN(created_at) first_sent, MAX(created_at) last_sent,
     ROUND((julianday(MAX(created_at))-julianday(MIN(created_at)))*1440,1) span_min,
     substr(replace(prompt,char(10),' '),1,90) prompt_head
   FROM prompt_queue WHERE length(prompt) > 40
   GROUP BY source, target, prompt HAVING COUNT(*) > 1
   ORDER BY repeats DESC LIMIT 25;"

echo "############ F7: urgent interrupts — volume by day and source (each discards an in-flight turn)"
q "SELECT date(created_at) d, source, COUNT(*) n FROM prompt_queue WHERE urgent=1
   GROUP BY 1,2 ORDER BY d DESC, n DESC LIMIT 25;"

echo "############ F8: delivery retry burden — attempts per message"
q "SELECT delivery_attempts, COUNT(*) n FROM prompt_queue GROUP BY 1 ORDER BY delivery_attempts DESC LIMIT 15;"

echo "############ F9: worker cold-start — registration -> first lease claim, minutes"
q "WITH f AS (SELECT agent_id, MIN(timestamp) t FROM task_lease_history
             WHERE event_type='claimed' GROUP BY 1)
   SELECT a.name, ROUND((julianday(f.t)-julianday(a.registered_at))*1440,1) idle_min,
     date(a.registered_at) d
   FROM agents a JOIN f ON f.agent_id=a.id
   WHERE idle_min IS NOT NULL ORDER BY idle_min DESC LIMIT 20;"

echo "############ F10: lease churn — tasks re-claimed many times (rework / restart cost)"
q "SELECT task_id, COUNT(*) claims, COUNT(DISTINCT agent_id) distinct_agents,
     MIN(timestamp) first_claim, MAX(timestamp) last_claim
   FROM task_lease_history WHERE event_type='claimed'
   GROUP BY 1 HAVING claims > 2 ORDER BY claims DESC LIMIT 20;"

echo "############ F11: lease event mix (expired = work abandoned by a dead/stalled worker)"
q "SELECT event_type, COUNT(*) n FROM task_lease_history GROUP BY 1 ORDER BY n DESC;"

echo "############ F12: reminders that never fired"
q "SELECT status, COUNT(*) n FROM reminders GROUP BY 1 ORDER BY n DESC;"

echo "############ F13: ack latency for messages that were acked, seconds"
q "WITH l AS (SELECT (julianday(acked_at)-julianday(transport_delivered_at))*86400 s
             FROM prompt_queue WHERE acked_at IS NOT NULL AND transport_delivered_at IS NOT NULL)
   SELECT COUNT(*) n, ROUND(AVG(s),1) avg,
     ROUND((SELECT s FROM l ORDER BY s LIMIT 1 OFFSET (SELECT COUNT(*)*50/100 FROM l)),1) p50,
     ROUND((SELECT s FROM l ORDER BY s LIMIT 1 OFFSET (SELECT COUNT(*)*90/100 FROM l)),1) p90,
     ROUND(MAX(s),1) max FROM l;"

echo "############ F14: worker_died volume by day (spurious-death detector)"
q "SELECT date(created_at) d, COUNT(*) n,
     COUNT(DISTINCT json_extract(payload,'\$.worker_name')) distinct_workers
   FROM supervisor_queue WHERE event_type='worker_died'
   GROUP BY 1 ORDER BY d DESC LIMIT 15;"

echo "############ F15: repeated death notices for the SAME worker (duplicate storm)"
q "SELECT json_extract(payload,'\$.worker_name') worker, COUNT(*) death_notices,
     MIN(created_at) first, MAX(created_at) last
   FROM supervisor_queue WHERE event_type='worker_died'
   GROUP BY 1 ORDER BY death_notices DESC LIMIT 15;"
