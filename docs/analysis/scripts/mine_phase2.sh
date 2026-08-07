#!/usr/bin/env bash
# Phase-2 queries for cas-9d92. READ-ONLY: never query the live .cas/cas.db.
set -euo pipefail
SNAP=${1:-/tmp/cas-p2/snap.db}
[ -f "$SNAP" ] || { echo "snapshot first: cp .cas/cas.db $SNAP"; exit 1; }

# Epoch boundaries are MEASURED, not assumed. Install time = binary mtime;
# clean-post = after the last PRE-install daemon stops heartbeating.
INSTALL='2026-08-07T21:02:26'
CLEAN='2026-08-07T21:36:35'

echo "== binary + live daemons (confirm no '(deleted)' exe links) =="
stat -c '%y %n' "$(command -v cas)"
for p in $(pgrep -f 'cas serve'); do printf '%s %s\n' "$p" "$(readlink /proc/$p/exe 2>/dev/null)"; done

echo "== last heartbeat of any PRE-install daemon (defines the clean boundary) =="
sqlite3 -header "$SNAP" "select max(last_heartbeat) from daemon_instances where started_at<'$INSTALL';"

echo "== three-epoch stratification =="
sqlite3 -header "$SNAP" "
select case when created_at<'$INSTALL' then '1_PRE'
            when created_at<'$CLEAN'   then '2_MIXED'
            else '3_CLEANPOST' end epoch,
       count(*) n, sum(transport_delivered_at is null) undeliv,
       sum(last_pending_reason='suppressed_idle') supp_idle,
       sum(transport_delivered_at is not null and acked_at is null) unreconciled,
       sum(acked_at is not null) acked
from prompt_queue where created_at>='2026-08-07' group by 1 order by 1;"

echo "== falsifiable test: do suppressed_idle rows drop BEFORE transport? =="
sqlite3 -header "$SNAP" "
select last_pending_reason, count(*) n,
       sum(transport_delivered_at is null) transport_null,
       sum(transport_delivered_at is not null) transport_set,
       min(created_at) first, max(created_at) last
from prompt_queue where last_pending_reason is not null group by 1 order by n desc;"

echo "== attribution: what makes up today's undelivered rows =="
sqlite3 -header "$SNAP" "
select coalesce(last_pending_reason,'(null)') r, count(*) n from prompt_queue
where created_at>='2026-08-07' and transport_delivered_at is null group by 1 order by n desc;"

echo "== undelivered rate by day (regression shape) =="
sqlite3 -header "$SNAP" "
select substr(created_at,1,10) d, count(*) n, sum(transport_delivered_at is null) undeliv,
       round(100.0*sum(transport_delivered_at is null)/count(*),1) pct
from prompt_queue where created_at>='2026-07-25' group by 1 order by 1;"

echo "== worker_died emitter: notices vs processed =="
sqlite3 -header "$SNAP" "select count(*) notices, sum(processed_at is null) unprocessed
from supervisor_queue where event_type like '%died%';"

echo "== delivery_attempts dead instrumentation =="
sqlite3 -header "$SNAP" "select count(*) rows_total, sum(delivery_attempts=0) at_zero from prompt_queue;"

echo "== hot-loop: most-repeated message_id in today's log =="
grep -oE 'message_id=[0-9]+' "${CAS_LOG:-/home/pippenz/Petrastella/cas-src/.cas/logs/cas-2026-08-07.log}" \
  | sort | uniq -c | sort -rn | head -5

echo "== transcript mining (scripted; no bulk log text into model context) =="
echo "python3 $(dirname "$0")/mine_transcripts.py; python3 $(dirname "$0")/mine_relay_dupes.py"
