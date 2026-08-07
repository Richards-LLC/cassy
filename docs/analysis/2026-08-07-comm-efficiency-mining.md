# Factory communication & efficiency mining — 2026-08-07

Task: cas-9d92 (spike). Corpus mined: coordination DB (452 MB), daemon logs (458 MB,
2026-03 → 2026-08-07), session transcript inventory. All processing scripted; no bulk log
content was read into model context.

## Method & safety

**Snapshot, never the live file.** `sqlite3 .backup` does not terminate against this DB: the
daemon writes continuously and every write restarts the backup (observed stalling at 341 MB of
452 MB after 6 minutes). A plain filesystem copy of `cas.db` + `cas.db-wal` + `cas.db-shm` is
read-only with respect to the source and replays the WAL locally on first open:

    cp .cas/cas.db /tmp/cas-mining/snap.db
    cp .cas/cas.db-wal /tmp/cas-mining/snap.db-wal
    cp .cas/cas.db-shm /tmp/cas-mining/snap.db-shm
    sqlite3 /tmp/cas-mining/snap.db "PRAGMA integrity_check;"   # -> ok

`PRAGMA integrity_check` returned `ok`. **Zero writes to `.cas/`**; all scratch output lives in
`/tmp/cas-mining/`. Reproduce every table below with
`docs/analysis/scripts/mine_comm_efficiency.sh [snapshot.db]` (sections keyed `F1`–`F15`);
log-side findings carry their `grep` inline.

**Metric definitions adopted from prior art.** One bounded Exa pass
(`exa-search "multi-agent LLM orchestration observability metrics message delivery latency
percentiles retry amplification log storm SRE"`, cost $0.007) surfaced the Harn orchestrator
observability spec (<https://harnlang.com/orchestrator/observability.html>). Adopted from it:
*oldest-pending-age* as a queue-health gauge rather than mean latency (→ F7), *accepted-to-DLQ
latency* as a first-class metric (→ F2/F8), and *queue-age-at-dispatch* separated from
*dispatch-runtime* (→ F6 vs F7). Its `backpressure_events_total{dimension,action}` counter shape
is the direct model for the fix proposed in F9. Nothing else in the pass was worth adopting.

---

## Findings, ranked by quantified cost

### 1. Redelivery hot-loop amplifies one message into 55,868 log lines — 464 MB/day (P0)

**Cost:** 464,485,228 bytes of log in a single day vs ~1 MB on adjacent days (**443×**), plus
sustained daemon CPU for 57 minutes, plus the diagnostic cost of a log that lies.

558 distinct messages produced **704,901** `stage="delivered"` lines on 2026-08-06:

    grep -c 'stage="delivered"' cas-2026-08-06.log        # 704901
    grep -c 'stage="wake_deferred"' cas-2026-08-06.log    # 704465
    grep -oE 'message_id=[0-9]+' cas-2026-08-06.log | sort -u | wc -l   # 558
    grep -oE 'message_id=[0-9]+' cas-2026-08-06.log | sort | uniq -c | sort -rn | head -3
    #  55868 message_id=7070
    #  55858 message_id=7072
    #  55454 message_id=7074

Each poll tick emits a `delivered` + `wake_deferred` **pair** without transitioning the row.
Message 7070 looped 17:55:30 → 18:52:53 at ~16 iterations/second. Hourly line counts show the
blowup: 192 (11:00) → 634,569 (17:00) → 636 (21:00).

**The log label is false.** The emitted line claims success —
`prompt_queue message delivered to inbox stage="delivered" ... deliver_ms=81` — while the DB row
for that same id says it was never delivered and was ultimately abandoned:

    id=7070  target=loyal-heron-7  transport_delivered_at=NULL
    highest_stage=abandoned  last_pending_reason=abandoned_unknown_target

A message that never arrived was logged as "delivered to inbox" 55,868 times. That mislabel is
why the storm resisted diagnosis.

**Self-referential detail worth reading twice:** the three looped messages are the spawn brief
for worker `loyal-heron-7`, spawned for task **cas-ceae — "Delivery storm regression:
worker-inbox 385× redelivery + supervisor-batch duplication"**. The worker sent to fix the
385× storm never registered, and its own brief became a 55,868× storm — 145× worse than the bug
it was dispatched against.

**Proposed fix task:** *Gate redelivery logging and back off pending rows.* (a) Emit
`stage="delivered"` only when `transport_delivered_at` actually transitions NULL→value; log
retries at `debug` under a distinct `stage="redelivery_attempt"`. (b) Apply exponential backoff
keyed on `delivery_attempts` (see F9) instead of re-polling at 16 Hz. (c) Cap per-message
delivery attempts and dead-letter past the cap. (d) Regression test: a pending row whose target
never registers must produce O(log n) not O(n) log lines.

---

### 2. Message-loss regression: undelivered rate jumped 10–30× starting 2026-08-04 (P0)

**Cost:** 397 undelivered messages across 4 days, against a prior baseline of ~1%.

| Date | Messages | Undelivered | % |
|---|---|---|---|
| 2026-07-22 | 310 | 3 | 1.0 |
| 2026-07-29 | 475 | 6 | 1.3 |
| 2026-07-30 | 496 | 2 | 0.4 |
| 2026-07-31 | 520 | 5 | 1.0 |
| **2026-08-04** | 184 | 64 | **34.8** |
| **2026-08-05** | 92 | 34 | **37.0** |
| **2026-08-06** | 553 | 185 | **33.5** |
| **2026-08-07** | 591 | 114 | **19.3** |

The regression window opens exactly when `suppressed_idle` first appears (F3) — same date, and
356 of the 397 losses carry that reason. Treat F3 as the probable cause of F2.

**Proposed fix task:** *Bisect the 2026-08-04 delivery regression.* Diff coordination-path
commits between 2026-08-01 and 2026-08-04; the idle-suppression gate is the prime suspect. Add a
daily-undelivered-rate assertion (<5%) to the factory smoke suite so this class cannot regress
silently again.

---

### 3. Idle-gate silently discards messages — 356 dropped, 100% never delivered (P0)

**Cost:** 356 messages destroyed with no retry and no sender-visible error.

    SELECT SUM(transport_delivered_at IS NULL), COUNT(*), SUM(urgent)
    FROM prompt_queue WHERE last_pending_reason='suppressed_idle';
    -- 356 | 356 | 0

Every single message marked `suppressed_idle` was **never delivered** — the gate is a drop, not a
defer, despite the sibling `wake_deferred` path existing for exactly this case. First occurrence
2026-08-04, last 2026-08-07: this is new, and it is the message-loss face of the GH #147 wake
false-positive class (a target judged idle that is not).

**Proposed fix task:** *Make idle suppression a defer, not a drop.* Route suppressed rows through
the same pending-retry path as `wake_deferred` with a bounded TTL; on TTL expiry dead-letter with
a sender-visible error rather than dropping. Cross-reference GH #147.

---

### 4. Worker-death notices never reach the supervisor — 2,044 critical alerts, 100% silent (P1)

**Cost:** every worker death since 2026-07-21 was invisible unless the supervisor manually
polled. Silent death is the dominant cause of stalled lanes.

    SELECT event_type, COUNT(*), SUM(prompt_delivered_at IS NULL) FROM supervisor_queue GROUP BY 1;
    -- worker_died    | 2044 | 2044  (100%)
    -- reminder_fired |   51 |   51  (100%)
    -- task_lifecycle | 1322 |    0  (0%)

**Code-verified, not inferred.** `emit_worker_died_signals`
(`cas-cli/src/mcp/tools/service/orphan_recovery.rs:188-266`) writes to the event store and calls
`queue.notify(...)` on `supervisor_queue` at `NotificationPriority::Critical` — and **never
enqueues to `prompt_queue`**. The daemon's auto-drain
(`cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs:330`) recovers only `task_lifecycle` rows.
So nothing injects a death notice into the supervisor's session.

Contrast `fire_reminder` (`cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs:4338-4375`),
which notifies `supervisor_queue` *and* calls `queue.enqueue_with_session(...)` for PTY injection.
`reminder_fired`'s 100% figure is therefore **benign** — those rows are structured-data mirrors of
a message that did get delivered. `worker_died` has no such second path. The asymmetry is the bug.

**Proposed fix task:** *Give `worker_died` the reminder's dual-write.* Add a
`prompt_queue.enqueue_with_session` call to `emit_worker_died_signals` targeting each resolved
supervisor pane, with the held/recovered task list in the body. Test: kill a leased worker, assert
a prompt row lands for the supervisor.

---

### 5. Death notices for the same worker repeat unboundedly — 1,452 for one agent (P1)

**Cost:** 2,044 rows where ~30 distinct deaths occurred; on 2026-07-28 alone, 1,153 notices for
2 distinct workers.

    SELECT json_extract(payload,'$.worker_name'), COUNT(*) FROM supervisor_queue
    WHERE event_type='worker_died' GROUP BY 1 ORDER BY 2 DESC;
    -- test-supervisor | 1452   (2026-07-27 -> 2026-08-06)
    -- supervisor      |  551   (2026-07-27 -> 2026-07-31)

`test-supervisor` was re-declared dead 1,452 times over 10 days. Reason payloads read
`"daemon maintenance: heartbeat stale"` — a stale-heartbeat agent is re-detected every maintenance
tick because nothing reaps it or marks the notice handled. Corroborating: **all 3,417
`supervisor_queue` rows have `processed_at IS NULL`** — the mark-processed path
(`crates/cas-store/src/supervisor_queue_store.rs:398,444`) exists but is effectively never
exercised in production.

**Proposed fix task:** *Reap dead agents and dedupe death notices.* Transition the agent row to a
terminal status on first death detection so maintenance skips it; add a `transition_key`-style
unique guard (the mechanism `task_lifecycle` already uses) so one death yields one notice. Also
audit why `processed_at` is never stamped.

---

### 6. Acknowledgement latency: p90 = 22.7 min, worst = 8.9 h (P1)

**Cost:** the supervisor's mean wait for a worker reply is ~10 min; the tail blocks lanes for
most of a working day.

    n=1131  avg=625.8s  p50=67.4s  p90=1361.0s  max=31951.3s

p50 of 67 s is healthy; p90 of 22.7 min and a 8.9-hour worst case are not. Per the adopted Harn
framing this is *queue-age-at-dispatch*, distinct from transport latency (F7) — the message
arrived promptly and then sat unread, which points at turn-scheduling and idle-gating rather than
transport.

**Proposed fix task:** *Surface ack age in `worker_status`.* Show oldest-unacked-message age per
worker and flag any worker with unacked mail older than 10 min, so the supervisor sees the stall
without polling.

---

### 7. Transport latency tail: p99 = 258 s, worst = 2.5 h against a p50 of 0.02 s (P2)

**Cost:** 1% of all messages take >4 minutes to reach a pane that normally receives in 20 ms — a
12,900× spread.

    n=3486  avg=16.55s  p50=0.02s  p90=0.95s  p99=258.43s  max=8999.96s

The bimodality (p50 20 ms, p90 under 1 s, then a cliff) indicates a distinct stuck path, not
general slowness — consistent with the F1 pending-row loop, where rows sit until an unrelated
poll happens to grant a turn.

**Proposed fix task:** *Add an oldest-pending-age gauge with a 30 s alert*, per the Harn
`oldest_pending_age_seconds` pattern; alert routes to the supervisor pane.

---

### 8. Spawn/registration race abandons task briefs — 49 messages to 15 never-registered targets (P2)

**Cost:** 49 abandoned messages, including entire task assignments. This is the trigger for F1.

    SELECT target, COUNT(*) FROM prompt_queue
    WHERE last_pending_reason='abandoned_unknown_target' GROUP BY 1 ORDER BY 2 DESC;
    -- test-supervisor 13 | bright-eagle-91 9 | team-lead 6 | proud-leopard-24 3
    -- loyal-heron-7 3 | loyal-dragon-99 3 | supervisor 2 | ... (15 targets, 2026-07-28 -> 2026-08-07)

Messages are enqueued against a worker name before the worker registers; if registration never
completes, the brief is orphaned. `loyal-heron-7`'s three rows are the F1 storm. Note `supervisor`
and `team-lead` appear as unknown targets — logical-name resolution fails too, not just
generated worker names.

**Proposed fix task:** *Resolve targets at enqueue time.* Reject or park messages whose target has
no registered agent and no pending spawn; on spawn timeout, dead-letter the brief and report the
failed spawn to the supervisor instead of leaving rows to loop.

---

### 9. `delivery_attempts` is dead instrumentation — 0 for all 7,857 rows (P2)

**Cost:** the single metric that would have caught F1 on day one has never been written.

    SELECT delivery_attempts, COUNT(*) FROM prompt_queue GROUP BY 1;
    -- 0 | 7857     (only one row group: the column is never incremented)

The column, plus `next_attempt_at` and `first_attempt_at`, exist in the schema for exactly this
purpose. A message redelivered 55,868 times still reports 0 attempts.

**Proposed fix task:** *Wire the retry counters.* Increment `delivery_attempts` and stamp
`first_attempt_at`/`next_attempt_at` on every delivery attempt; add a
`backpressure_events_total{dimension,action}`-style counter (Harn shape) for gate/suppress/defer
decisions. This is a prerequisite for the F1 backoff fix.

---

### 10. Lease churn — 115 unclean lease terminations; one task claimed by 4 agents in 2 h (P2)

**Cost:** each revoke/expire is discarded worker context. 67 revoked + 48 expired out of 1,094
claims (10.5%).

    SELECT event_type, COUNT(*) FROM task_lease_history GROUP BY 1;
    -- claimed 1094 | released 949 | revoked 67 | expired 48

Worst live example: **cas-edee** — 4 claims by 4 distinct agents between 17:02 and 19:18 on
2026-08-07 (2 h 16 min of repeated re-onboarding onto one task). Historical worst: cas-7f6f, 6
claims in 42 min by a single agent (claim-loop, not handoff).

**Proposed fix task:** *Alert on lease re-claim count.* Flag any task exceeding 2 claims or 2
distinct assignees in a 24 h window; require a handoff note on re-assignment so the next agent
does not re-derive context.

---

### 11. Urgent interrupts are the supervisor's default channel — 117 of 122 in 4 days (P2)

**Cost:** each urgent interrupt discards the target's in-flight reasoning by design. 122 total
across all history, but 117 concentrated in 2026-08-04 → 08-07.

| Date | Source | Count |
|---|---|---|
| 2026-08-07 | supervisor | 58 |
| 2026-08-06 | supervisor | 59 |
| 2026-08-05 | supervisor | 1 |
| 2026-08-04 | supervisor | 4 |

Two days at ~58/day, against ≤15/day historically. The timing coincides with F2/F3 — plausibly
the supervisor escalating to `urgent` *because* normal delivery was silently failing. That makes
urgent-interrupt volume a useful leading indicator of delivery health, and means fixing F2/F3
should reduce discarded turns as a side effect.

**Proposed fix task:** *Track urgent-interrupt rate as a health metric*; warn the supervisor when
urgent sends exceed 10/day, prompting a delivery-health check rather than more interrupts.

---

### 12. Reminders expire without firing — 12 of 72 (17%) (P3)

**Cost:** 12 scheduled follow-ups silently never happened.

    SELECT status, COUNT(*) FROM reminders GROUP BY 1;
    -- fired 51 | expired 12 | cancelled 7 | pending 2

An expired reminder is a supervisor intention that evaporated. Note also `fire_reminder` falls
back to the literal pane name `"supervisor"` when `target_id` is absent from the name map
(`queue_and_events.rs:4356-4359`) — given F8 shows `supervisor` itself failing target resolution,
some reminders likely misroute rather than reach the intended worker.

**Proposed fix task:** *Report reminder expiry.* On TTL expiry, notify the owner that the
reminder never fired instead of silently marking it expired; fix the fallback to error rather
than guess a pane.

---

### 13. Worker cold-start idle — up to 150 min between registration and first claim (P3)

**Cost:** paid-for worker capacity sitting idle.

    -- witty-finch-14   150.0 min   2026-08-06
    -- smooth-stork-50    2.3 min   2026-08-07
    -- swift-tiger-11     0.6 min   2026-08-07
    -- cosmic-eagle-24    0.1 min   2026-08-07

Current-generation spawns are healthy (<3 min); `witty-finch-14`'s 150 min on 2026-08-06 falls
inside the F1 storm window, so this is most likely a *symptom* of undelivered task briefs rather
than an independent defect. Recheck after F1/F2 land before filing separately.

---

## Vector store decision: **not built**

**Decision: no semantic index.** SQL and grep covered every query class this corpus poses, and
the one class that looked semantic turned out not to be.

The corpus is strongly structured: `stage=`, `message_id=`, `target_agent=` in logs;
`highest_stage`, `last_pending_reason`, `wake_attempt`, `event_type` in the DB. Every finding
above resolved to an exact-match GROUP BY or a `grep -c`. The candidate justification for
embeddings was *"recurring confusion themes / instruction drift"* — supervisor re-asks that are
semantically but not textually identical. I tested whether SQL could reach that class before
concluding, using prefix-bucketing as a cheap near-duplicate proxy:

    SELECT source,target,COUNT(*) n, substr(replace(prompt,char(10),' '),1,60) head
    FROM prompt_queue WHERE created_at>='2026-08-04'
    GROUP BY source,target,head HAVING n>1 ORDER BY n DESC;

It cleanly recovered the re-ask cluster (`jolly-wolf-30 → quiet-crow-26` ×5, `young-jaguar-37 →
quiet-crow-26` ×4, all "Fresh after draining unread inbox…"). Full-text search over message
bodies is also already available via the `recordings_fts` / `knowledge_pages_fts` FTS5 tables in
this same DB, giving keyword recall with zero build cost.

**Honest cost accounting for the road not taken:** embedding ~7,857 prompt bodies plus transcript
turns would take an estimated 20–40 min of local embedding time and ~1 GB of scratch index, to
answer questions the above one-liner answered in 40 ms. It would not have found F1, F2, F3, F4 or
F9 — all of which are counting problems, not meaning problems. **Live knowledge stores were not
touched**; no ingestion of any kind occurred.

Revisit if a future question requires clustering *free-text worker reasoning* across transcripts
(e.g. "which instructions do workers most often misinterpret") — that is a genuine semantic
query, and it is out of scope for a defect sweep.

## Cross-references to known issues

- **GH #160** (silent relay; cas-7787 in flight) — the full-history sweep is F4: the silent class
  is `worker_died`, **2,044 instances across 2026-07-21 → 2026-08-07**, not a single
  18:35–19:30 window. `task_lifecycle` relay is by contrast 0% silent (1,322/1,322 delivered), so
  the fix belongs in `emit_worker_died_signals`, not the lifecycle path. Breadth + quantification
  only — no overlap with that task's fix.
- **GH #147** (wake false-positive) — F3 is the message-loss form of the same idle-gate
  misjudgement: 356 messages dropped, all since 2026-08-04.
- **GH #155** — **RETRACTED CLAIM.** An earlier revision of this report read the drop in
  undelivered rate from 33.5% (08-06) to 19.3% (08-07) as evidence that v2.49.0's fix was
  working. That inference is invalid and is withdrawn. Per the running-binary analysis below,
  the v2.49.0 binary was installed at 21:02 UTC and **no daemon has restarted since** — every
  daemon process serving the factory today started at or before 20:59 UTC and is therefore
  executing pre-v2.49.0 code. No observation in this corpus can speak to v2.49.0's fix in either
  direction. The 08-06 → 08-07 improvement has some other cause and remains unexplained.
  Classification: **FIXED-UNVERIFIED**. See "Temporal classification" below.

## Corpus coverage

| Source | Size | Coverage |
|---|---|---|
| `.cas/cas.db` (snapshot) | 452 MB | Full history, all coordination tables |
| `.cas/logs/*.log` | 458 MB | Full, 2026-03-20 → 2026-08-07 |
| `~/.claude-alt` transcripts | 66 MB, 25 sessions | Inventoried; not required — DB/logs carried every finding |
| `~/.claude` transcripts | 96 MB, 25 sessions | Inventoried; same |

Transcripts were deliberately not mined: all 13 findings resolved from structured sources, and
per the task's own instruction ("structured first"), transcript mining is the escalation path for
the semantic questions deferred with the vector-store decision above.

---

# AC7 — Temporal classification against running-binary epochs

Six releases (v2.44 → v2.49.0) shipped inside the mining window, so raw historical counts would
report ghosts. Findings are classified against the **running-binary timeline**, not tag dates.

## The running-binary epoch (the trap, measured)

| Event | Local (EDT) | UTC | Source |
|---|---|---|---|
| v2.47.0 tagged | 08-06 22:53:38 | 08-07 02:53 | `git tag --sort=-creatordate` |
| v2.48.0 → v2.48.3 tagged | 08-07 09:01 → 10:07 | 13:01 → 14:07 | same |
| **v2.49.0 tagged** | **08-07 16:31:24** | **20:31** | same |
| Last daemon restarts | 08-07 16:51 / 16:54 | **20:51 / 20:54** | `daemon_instances`, log init lines |
| **v2.49.0 binary installed** | **08-07 17:02** | **21:02** | `ls -la $(which cas)`, `cas --version` → 2.49.0 (3d58332) |
| Snapshot taken | 08-07 17:05 | 21:05 | this run |

**The binary was installed 8 minutes after the last daemon started.** Every `cas serve` process
serving the factory (PIDs 910163, 1541273, 1543040, 1548852, 1549075) began at or before
20:59 UTC; `daemon_instances` confirms all six live rows started ≤ 20:59:29 UTC. Therefore:

> **No data in this corpus observes v2.49.0 behaviour.** Every v2.49.0 fix — #155 turn-start
> surfacing, m220 wake observability, #145 clear_context, #152 review-ownership — is
> **FIXED-UNVERIFIED**. Absence of improvement today is *expected*, not a regression, and must
> not be filed as one.

Epoch boundaries usable for classification (daemon restarts, UTC): 08-06 16:41, 18:53, 19:43,
22:16; 08-07 12:25, 15:32, 16:36–16:37, 20:51–20:54. Historical binary-install times are not
recoverable (only the current binary's mtime survives), so pre-08-07 epochs are bounded by
restart times alone — stated here rather than papered over.

## Headline: the amendment incident — GH #155's class, reproduced on this task

While attempting to close this task, the MERGE-REQUIRED remediation forced an `inbox_poll`,
which returned **7 unread messages** — including **five binding amendments to this very task**
(AC7 temporal stratification, AC8 issue filing, the Phase-1 checkpoint gate, and the
vector-store pre-approval) that had never reached the session. Their DB rows:

    SELECT id,source,transport_delivered_at IS NULL undel,acked_at IS NULL unacked,highest_stage
    FROM prompt_queue WHERE target='cosmic-eagle-24';
    -- 7869 cas        | 0 | 1 | delivered
    -- 7871 director   | 0 | 1 | delivered
    -- 7877 supervisor | 0 | 1 | delivered   <- kickoff, never surfaced
    -- 7879 supervisor | 0 | 1 | delivered   <- AC7 amendment, never surfaced
    -- 7880 supervisor | 0 | 1 | delivered   <- AC8 amendment, never surfaced
    -- 7881 supervisor | 0 | 1 | delivered   <- phase-gate amendment, never surfaced

All six stamped `transport_delivered_at`, `highest_stage=delivered`, and every one re-issued by
the poll marked `[redelivery] — already delivered`. Four of them never entered the session's
context; the work proceeded for over an hour against a spec that had been amended four times.
This is precisely GH #155 — *"a message could be marked delivered to a session that never saw
it"* — **behaviourally confirmed on the pre-2.49.0 binary at 21:01–21:09 UTC today**, and it is
the single most expensive defect in this report: it silently invalidated the task's own
requirements.

**Classification: STILL-LIVE on the running binary; FIXED-UNVERIFIED against v2.49.0.** The
v2.49.0 turn-start drain is designed to fix exactly this and has never executed here. The
actionable ask is therefore *restart the daemon on the new binary and re-measure*, not a new fix.

**Screening signature (with its honest limit).** The pattern is
`transport_delivered_at IS NOT NULL AND acked_at IS NULL`:

| Date | Msgs | Stamped-delivered-never-acked | % |
|---|---|---|---|
| 2026-08-07 | 591 | 422 | 71.4 |
| 2026-08-06 | 553 | 179 | 32.4 |
| 2026-08-05 | 92 | 30 | 32.6 |
| 2026-08-04 | 184 | 58 | 31.5 |
| 2026-07-29 | 475 | 383 | 80.6 |
| 2026-07-22 | 310 | 307 | 99.0 |

**This is a screening metric, not a defect count.** Ack is not mandatory for every message type,
so unacked ≠ unseen, and the rate swings too wildly (99% → 25% → 71%) to carry a cost claim on
its own. What it is good for is ranking days for inspection. The only *behaviourally confirmed*
instances are the four amendments above — those, I can stand behind, because I am the recipient
that never saw them.

## Classification of the 13 findings

Actionable = STILL-LIVE + REGRESSED only.

| # | Finding | Last occurrence (UTC) | Class | Actionable |
|---|---|---|---|---|
| — | **Amendment incident** (stamped-delivered, never surfaced) | 08-07 21:09 | **STILL-LIVE** | **yes** |
| 1 | Redelivery hot-loop / log amplification | 08-06 18:52 | **STILL-LIVE** (unfixed; no release targets it) | yes |
| 2 | Undelivered-rate regression (~1% → 19–37%) | 08-07 20:59 | **STILL-LIVE** | yes |
| 3 | Idle-gate drops instead of defers (`suppressed_idle`) | **08-07 20:59:38** | **STILL-LIVE** | yes |
| 4 | `worker_died` never PTY-delivered | 08-07 19:37 | **STILL-LIVE** (code-verified, no fix exists) | yes |
| 5 | Death-notice duplication (1,452 for one agent) | 08-07 | **STILL-LIVE** | yes |
| 6 | Ack latency p90 22.7 min | 08-07 | STILL-LIVE | yes |
| 7 | Transport latency tail (p99 258 s) | 08-07 | STILL-LIVE | yes |
| 8 | `abandoned_unknown_target` spawn race | 08-07 13:58 | **STILL-LIVE** | yes |
| 9 | `delivery_attempts` dead instrumentation | 08-07 (all rows) | **STILL-LIVE** | yes |
| 10 | Lease churn (cas-edee, 4 agents/2 h) | 08-07 19:18 | STILL-LIVE | yes |
| 11 | Urgent-interrupt escalation (117 in 4 days) | 08-07 20:xx | STILL-LIVE (symptom of 2/3) | secondary |
| 12 | Reminders expire without firing | — | STILL-LIVE | yes |
| 13 | Worker cold-start idle (150 min) | 08-06 | **symptom of F1**, not independent | no |

### Confirmed-resolved appendix (validation evidence, NOT actionable)

- **`dead-lettered` — FIXED-VERIFIED.** 513 occurrences spanning 2026-03-20 → **2026-07-27
  17:30:44**, then zero across 11 subsequent active days carrying 2,800+ messages. A clean stop
  at a boundary with substantial post-boundary traffic is real extinction, not absence of data.
  This is the largest single symptom class in the corpus and it is genuinely gone.
- **Wake fix (v2.47.0, "an ordinary message wakes an idle worker again")** — partial support:
  worker cold-start times fell to <3 min for all 2026-08-07 spawns versus 150 min on 08-06.
  Sample size is 5 workers, so this is suggestive, not proof. **FIXED-UNVERIFIED.**

### Corrections this classification forced

1. The GH #155 improvement claim is **retracted** (above) — the fix was never live.
2. Finding 13 is **demoted** from a defect to a symptom of Finding 1.
3. `reminder_fired`'s 100%-undelivered figure was ruled **benign** by code read before it could
   become a false finding (`fire_reminder` dual-writes to `prompt_queue`; `emit_worker_died_signals`
   does not).

Three of thirteen findings changed status under temporal analysis — the amendment was worth it.
