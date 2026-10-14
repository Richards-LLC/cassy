# Cassy agent-to-agent communication review

Task cas-447d · 2026-10-14 · crisp-sparrow-96 · Cassy 3.51.0 (1c201ff) on soundwave

**Sources**
- Data: a read-only `.backup` snapshot of `.cas/cas.db` taken 2026-10-14 11:51Z, at `/mnt/rewind/cas/scratch/cas447d/snap.db`.
  - `.cas/prompt_queue.db` is a 0-byte file. The live queue tables are in `cas.db`.
- Transcripts: the zen-viper-42 supervisor transcript, plus six Claude worker transcripts.
- Code: mapped at the worktree tip `1c201ff06`.
- Primary session: `cassy-swift-gazelle-77`, 2026-10-13 20:48Z to 2026-10-14 11:49Z. It has 870 prompt_queue rows, 167 supervisor_queue rows and 18 workers.
- Five earlier sessions are used for comparison.

---

## Executive summary

**What works**
- The routing core is fast and well guarded. Worker-to-supervisor and supervisor-to-worker transport completes in 0.1–0.2 s on average.
- Every row carries a stage and a pending reason.
- Unknown targets fail at once with a named error instead of queueing silently.
- Lifecycle relays are durable and deduped by key.
- Typed envelopes for blockers, merge requests and verification decide what may wake an idle supervisor.
- External reminders fired on time; #2904 fired 26 minutes after it was set, when the PR landed.
- The worker return contract, when followed, produces messages that can be checked in one read. 71% of worker delivery messages followed it.

**What is broken**
1. **The operator never gets the supervisor's replies, and the queue says they were abandoned.**
   - 100 of 105 supervisor→operator rows this session were swept as `abandoned_unknown_target` about 930 s after creation, because "operator" is missing from the sweep's list of valid targets.
   - All 202 operator outbox events ever recorded have no relay receipt, and the cloud outbox has never held a row.
   - This is the root of cas-5c89. Filed as **cas-d88c**.
2. **Delivery bookkeeping is wrong.**
   - Gated rows get a "seen" receipt about 0.1 s after enqueue, before anyone has read them.
   - Messages that were read are later labelled "abandoned": 128 this session, and 1,376 of the 1,990 "abandoned" rows in the last month.
   - Every metric, banner and abandonment notice built on these fields is unreliable. Filed as **cas-f7b0**.
3. **Readers see the same thing several times.**
   - A Claude Teams supervisor reads each pointer-woken message three times. 37 ids came wake + hook + teammate block, about 21K tokens.
   - Every merge park sends two relays, and the second one reads as a rejection: 14/14 this session, 311/312 since October.
   - New workers inherit the whole broadcast backlog, 13 h old and contradictory, and cannot start their task until they read it.
   - The supervisor receives its own broadcasts back.
4. **Alarms cry wolf.**
   - Six workers got 105 "you have gone quiet" nudges (~19K tokens), more than all the supervisor's direct messages to them. One worker got 67, 23 of them in a single prompt, while it was sleeping on CI.
   - 6 of 8 checkable `worker_stalled` escalations fired while a tool call was running.
   - A shutdown the supervisor itself requested sent 13 Critical alarms, about 2.9K chars each, mostly spinner glyphs.
5. **Supervisors can't coordinate with each other.**
   - Nothing hands off epics when a session ends.
   - Integration notices go to dead owners.
   - Two supervisors' conflicting integrations triggered no notice to either.
   - The 3.51 cloud peer mailbox has never carried a live send from this machine: `peer_message_outbox` has 0 rows.

**Highest-leverage fixes, in order**
1. cas-d88c: operator lane sweep
2. cas-f7b0: honest receipts
3. cas-1f0c: one body per message
4. cas-7156: stop the nudge storms
5. cas-6853: broadcast backlog and echo
6. cas-5c2c: supervisor-to-supervisor handoff and conflict notices

The first three are small and local. Together they change what every agent and every dashboard believes about delivery.

Filed tasks: 19 new tasks, plus evidence notes on 4 existing ones (cas-5c89, cas-00d5, cas-8063, cas-6eec). The full list is in §7.

---

## 1. Architecture as built

| Channel | Path | Key code |
|---|---|---|
| supervisor ↔ worker | `coordination action=message` → `message_send` validates summary, the 1200-char cap (2500 for escalations) and the target → `prompt_queue` row → daemon delivery loop → Claude Teams inbox file plus an optional `CAS wake` line, or full body typed into the PTY (Codex/Grok/OpenCode) → turn-start or tool-boundary hook surfaces it as `[incoming messages]` | `agent_search_system/message.rs:662-2552`; `traffic_limits.rs:53-71`; `daemon/runtime/delivery.rs:163-176, 439-449`; `hooks/.../factory_inbox.rs:180-401` |
| worker ↔ worker | Same handler. The peer must be in the same session. Writes two rows: the peer's, plus a "Peer worker message copy" for the supervisor. Limited to 5 per 60 s. Native `SendMessage` bypasses all of this. | `message.rs:923-981, 2208-2237`; `prompt_queue_store.rs:3710-3772`; `pre_tool.rs:650-655, 7264-7400` |
| Director / lifecycle → supervisor | `supervisor_queue` row (INSERT OR IGNORE on `transition_key`) → `prompt_queue` row with source `lifecycle:` (inbox only) or `lifecycle-wake:` (may wake). Only close_rejected, awaiting_merge, blocked, attention and verification classes wake the supervisor. | `supervisor_push.rs:38-156, 822-865`; `supervisor_queue_store.rs:100-103, 304-330`; `queue_and_events.rs:4659-4774` |
| Director → worker nudges | Direct director `Prompt`, `durable_retry=false`. Never enters `prompt_queue`, so queue metrics can't see it. | `director/prompts.rs:1816-1836`; `director/events.rs:1925-1960` |
| Supervisor ↔ supervisor, same machine | A name match against the local agent store stays local. The match ignores agent status. | `peer_message.rs:115-128`; `message.rs:1456-1506` |
| Supervisor ↔ supervisor, cross-machine (3.51) | `peer_message_send` → `peer_message_outbox` (m268): one synchronous try, then 5/10/20/30 s backoff and a 15-min TTL, then an abandon notice to the sender. Inbound poller runs every 15 s. Messages are addressed by **agent id**. | `cloud/peer_mailbox.rs:284-317, 377-460, 508-606`; `peer_mailbox/outbox.rs:5-219`; `peer_mailbox/runtime.rs:105` |
| Reminders | `factory_remind` → `reminders` → daemon tick every 2 s (external probes every 60 s) → `prompt_queue` row with no origin stamp, plus a `reminder_fired` row in `supervisor_queue` that nothing consumes | `factory_remind.rs:28-446`; `queue_and_events.rs:8941-9559`; `ci_watch.rs:24-27, 314-418` |
| Operator / Commander | Supervisor-only `target=operator\|commander:<label>` with a `kind`. Writes `record_operator_turn` → `operator_delivery_outbox` (always `unenrolled`), plus `operator_cloud_outbox` only while a feed binding is active. The Commander mirror copies final assistant text every 5 s. | `message.rs:1146-1438`; `prompt_queue_store/operator_delivery.rs:3-14, 184-288, 420`; `operator_cloud.rs:225-251`; `commander_mirror.rs:14, 90-362` |
| Violet / Slack | The daemon claims Slack activity from Cloud every 15 s and sends a typed `SlackActivity` wake to the supervisor (at most one per channel per 60 s). Only Violet may post. | `violet_activity.rs:46-57, 384-399, 551-704, 1239-1330, 1596-1611`; `slack_transport.rs:8-80` |
| Transport guards | Wake gate: 45 s of silence for an active recipient, 2 s for an idle one; parked after 3 declines; lifecycle rows re-nudged every 60 s, at most 20 times. Delivery watchdog: 600/1800 s, capped at 720 s. The 900 s retry age is `PROMPT_RETRY_MAX_AGE_SECS`. | `queue_and_events.rs:1125-1192, 1366-1428, 2386-2396`; `prompt_queue_store.rs:43, 2714`; `config/settings.rs:894-924` |

---

## 2. Measurements

Every query ran against the snapshot. The SQL files are in `queries/`. Abbreviations: `S = sqlite3 snap.db`, `W = factory_session='cassy-swift-gazelle-77'`.

### 2.1 Volume, latency and receipts by channel

This is `queries/lat.sql`. It classifies rows by source and target and computes transport and seen latency.

| from → to | n | seen | transported | acked | gate-declined | avg transport s | avg seen s |
|---|---|---|---|---|---|---|---|
| worker/self-reminder → named agent (230 to zen-viper-42 incl. 31 peer copies; 62 self-reminders; ~18 worker→worker) | 310 | 309 | 274 | 9 | 16 | 0.1 | 0.2 |
| supervisor → worker | 183 | 183 | 80 | 1 | **102** | 0.1 | 0.5 |
| worker → "supervisor" literal | 181 | 180 | 110 | 50 | 0 | 0.2 | 0.3 |
| supervisor → operator | 105 | **0** | 3 | 0 | 0 | 0.3 | – |
| director → worker | 35 | 35 | 26 | 3 | 10 | 1.0 | 0.7 |
| task-assignment → worker | 15 | 15 | 9 | 0 | 6 | 0.1 | 0.9 |
| supervisor → all_workers | 12 | 12 | 11 | 0 | 0 | 0.1 | 0.0 |

Reading the table:
- Transport is fast.
- 56% of supervisor→worker messages were deferred by "pane has not been silent long enough".
- The "seen" latency is an artifact. For gated rows the receipt source is `transport_claimed`, written about 0.1 s after enqueue (`queries/gate.sql`). For example, 4185905 was "seen" at +0.05 s, but the worker answered 9 min later (4185932).
- Explicit acks are rare (66 of 870 rows). That is by design: task start counts as acceptance.

### 2.2 Final stage distribution in this session

```sql
select highest_stage, count(*) from prompt_queue where W group by 1;
select last_pending_reason, count(*) from prompt_queue where W group by 1;
```

| Stage | Count |
|---|---|
| assumed_seen | 265 |
| **abandoned** | **228** |
| delivered | 197 |
| gated | 84 |
| confirmed | 66 |
| none (empty) | 26 |
| selected | 2 |
| partially_delivered | 1 |
| suppressed | 1 |

The reasons behind the 228 abandoned rows:
- `operator` target, never seen: 100. These are caused by the sweep and filed as cas-d88c.
- Worker targets, *seen*, never transported: 128. The average age at sweep was 40,455 s. These are false and filed as cas-f7b0.

```sql
select target='operator', transport_delivered_at is not null, acked_at is not null,
  exists(select 1 from prompt_queue_recipient_seen s where s.prompt_id=p.id), count(*),
  avg((julianday(processed_at)-julianday(created_at))*86400)
from prompt_queue p where W and last_pending_reason='abandoned_unknown_target' group by 1,2,3,4;
```

Cross-check against the daemon's own log: every one of the 128 rows plus 101 rows still "gated" has an `events.supervisor_injected … (ok)` entry.

```sql
with inj as (select json_extract(metadata,'$.prompt_id') pid, json_extract(metadata,'$.status') st
  from events where event_type='supervisor_injected' and created_at>='2026-10-13T20:48' group by 1,2)
select inj.st, p.highest_stage, p.last_pending_reason, count(*)
from inj join prompt_queue p on p.id=inj.pid group by 1,2,3;
```

### 2.3 The same pattern across sessions

This is `queries/xs.sql`.

| Session | Rows | Gated | Abandoned | …of which seen | → operator | Operator abandoned | Lifecycle | Peer copies | Broadcasts |
|---|---|---|---|---|---|---|---|---|---|
| swift-gazelle-77 | 870 | 134 | 228 | 128 | 105 | 100 | 149 | 31 | 12 |
| agile-raven-93 | 102 | 17 | 37 | 17 | 23 | 20 | 22 | 0 | 0 |
| fierce-octopus-14 | 1566 | 462 | 743 | 741 | 2 | 2 | 414 | 12 | 4 |
| rapid-parrot-36 | 496 | 108 | 219 | 219 | 0 | 0 | 145 | 4 | 4 |
| zealous-panther-88 | 305 | 72 | 269 | 204 | 66 | 65 | 57 | 0 | 0 |
| keen-tiger-11 | 275 | 9 | 97 | 0 | 97 | 92 | 15 | 0 | 0 |

Month-wide, from 2026-09-15:
- 1,990 rows were `abandoned_unknown_target`, and 1,376 of them had a seen receipt. Of those 1,376, 1,122 were "stale 86400s+ quarantined" and 254 were "target no longer belongs to factory session".
- 7 abandoned with "No such file or directory (os error 2) … 16 failed attempts / ~900s".

### 2.4 Operator channel

```sql
select factory_session, count(*), sum(relay_receipt is not null), sum(attempts) from operator_delivery_outbox group by 1;
select count(*) from operator_cloud_outbox;
```

- `operator_delivery_outbox`: 202 rows across 5 sessions. 0 relay receipts and 0 attempts.
- `operator_cloud_outbox`: **0 rows ever**.
- Of the 105 operator rows this session, 86 are mirrored `status` and 19 are `answer` (origin daemon). Only 3 were delivered: the in_reply_to answers sent while the Commander was live.

### 2.5 Lifecycle relays and the supervisor queue

```sql
select json_extract(payload,'$.transition'), count(*), count(distinct json_extract(payload,'$.task_id'))
from supervisor_queue where created_at>='2026-10-13T20:48' and event_type='task_lifecycle' group by 1;
```

| Transition | Count |
|---|---|
| task_started | 41 |
| task_closed | 37 |
| task_close_rejected | 14 |
| task_awaiting_merge | 14 |
| task_blocked | 9 |
| task_ready | 1 |

Other event types:
- reminder_fired: 62 (none processed or prompted)
- worker_died: 12
- worker_idle: 11, exactly one per worker
- worker_stalled: 10
- supervisor_stalled: 4
- workers_died_together: 1

Pairing: all 14 close_rejected rows share an `occurrence_id` with an awaiting_merge row for the same task, with reason `MERGE REQUIRED`. Since October: 311 close_rejected against 312 awaiting_merge.

```sql
select count(*) from supervisor_queue a join supervisor_queue b
 on json_extract(a.payload,'$.occurrence_id')=json_extract(b.payload,'$.occurrence_id')
 and json_extract(a.payload,'$.task_id')=json_extract(b.payload,'$.task_id')
where a.created_at>='2026-10-13T20:48' and json_extract(a.payload,'$.transition')='task_close_rejected'
  and json_extract(b.payload,'$.transition')='task_awaiting_merge';   -- 14
```

Dead letters:
- All-time, 2,514 `reminder_fired` rows have `processed_at` and `prompt_delivered_at` both NULL.
- All 116 task_lifecycle rows this session were never marked processed.

### 2.6 What the supervisor receives

This is `queries/inb.sql`: rows addressed to the supervisor, by category.

| Category | n | chars | avg chars |
|---|---|---|---|
| Worker messages | 218 | 187,028 | 858 |
| Lifecycle (may wake) | 52 | 50,712 | 975 |
| Lifecycle (inbox only) | 71 | 45,859 | 646 |
| Peer-worker copies | 31 | 18,972 | 612 |
| Worker attention (wake) | 26 | 8,548 | 329 |
| Reminders | 13 | 4,876 | 375 |
| Director (spawn) | 19 | 4,550 | 239 |
| Commander | 3 | 110 | 37 |

The queue holds 433 rows and 320K chars. The transcript shows about 534K chars (~133K tokens) of agent communication, because of repeats; §3 has the detail.

### 2.7 Broadcast backlog

```sql
select recipient, count(*) from prompt_queue_recipient_seen s join prompt_queue p on p.id=s.prompt_id
where p.target='all_workers' and p.factory_session='cassy-swift-gazelle-77' group by 1;
```

All 18 recipients have 12 of 12 receipts. That includes crisp-sparrow-96, registered 11:49Z, about 13 h after the first broadcast, and also `zen-viper-42` and `supervisor`, i.e. the sender itself.

### 2.8 Reminders

```sql
select trigger_type, trigger_event, status, cross_session, count(*),
  avg((julianday(fired_at)-julianday(created_at))*86400)
from reminders where created_at>='2026-10-13T20:48' group by 1,2,3,4;
```

- Time reminders: 62 fired (average delay 577 s) and 22 cancelled.
- branch_contained_in: 1 fired (#2904, 1,562 s after creation, when the PR landed) and 3 cancelled.
- All 13 reminder rows addressed to the supervisor record `wake_not_applicable: row carries no server-stamped sender`. They still worked for this Claude Teams supervisor, but only because the Teams inbox write starts a turn.

---

## 3. Transcripts: clarity, noise and token cost

Method: Python extractors over the model-visible text (`transcript-scripts/`); tokens are estimated as chars/4.

**Supervisor (zen-viper-42)**
- About 534K chars (~133K tokens) of communication.
- Including hook overhead (ambient recall ~68K tokens, supervisor-reminder hook ~13K tokens), about 870K chars (~218K tokens).
- **Repeats**:
  - 47 ids were delivered more than once: 37 three times (wake, hook body, teammate block), 7 as a queued wake plus hook plus teammate, and 3 other combinations. The repeats cost about 85K chars (~21K tokens).
  - All 45 wake lines claim "(body not repeated here)".
- **Semantic echoes**:
  - 25 of 30 `task_closed` relays were followed within 15 minutes by the worker's own "closed" message.
  - All 14 awaiting_merge relays were followed by 1–3 worker merge-request messages.
  - Eight workers each sent a short re-send right after a 1,200-cap rejection.
  - young-newt-54 said why: "plain status messages don't wake an idle supervisor. I re-sent it as a formal merge request".
- **Send receipts**: 185 sends returned about 540 chars each (~24K tokens). Of those, 179 were the same text: "enqueued… not yet confirmed delivered… check message_status". `message_status` was used once.
- **Noise**:
  - worker_died bodies are about 2.9K chars of pane tail ("Hullaballooing…✻✶…").
  - supervisor_stalled once said "actionable-idle for **0m**", while the supervisor was correctly waiting for CI.
  - A liveness relay told a Claude supervisor to call `mcp__cs__task`, which is the Codex prefix.

**Workers (6 Claude transcripts)**
- Inbound communication is 2.7–8.3% of each worker's context.
- About 47 off-task broadcasts reached them (~8K tokens), and 105 "gone quiet" nudges (~19K tokens).
- young-newt-54 alone got 67 nudges, 23 in one prompt at 00:02:19, during `sleep 290`.
- subtle-gopher-35 worked continuously from 11:19 to 11:42. Its brief, its status check and 5 nudges all arrived together at 11:42:20, after it had already finished.
- Return-contract compliance: 52% of all worker→supervisor messages, 71% of delivery messages.
- Weakest: fierce-cheetah-73, 8 of 22.

**Clarity**
- Good:
  - Supervisor briefs are specific and bounded (4185886).
  - Contract-form status messages can be checked in one read (4185226).
- Bad:
  - The merge-request envelope always looks failed ("Evidence not supplied", "Door: not declared").
  - Broadcast policy flip-flopped (merge, then don't merge), and the stale versions were replayed to new workers.

---

## 4. Supervisor observations: verdicts

| # | Lead | Verdict | Evidence |
|---|---|---|---|
| 1 | Wake gate defers to busy workers with no feedback, while `worker_stalled` fires | **Confirmed** | 4185905: deferred to the inbox at +0.05 s (`wake_gate_declines=1`, "pane has not been silent long enough"). `worker_stalled` 16496 came at 11:34, while the worker was running tool calls continuously. 102 of 183 supervisor→worker messages were gated. Filed as cas-d5ab. |
| 2 | Stalled, quiet and idle alarms are noisy | **Confirmed for stalled and quiet. Refuted for idle repeats; the real idle bug is missed alarms.** | 6 of 8 stall escalations fired during in-flight calls, and there were 105 worker nudges (cas-7156). `worker_idle` fired exactly once per worker (11 of 11), because its key never changes (`lifecycle.rs:121`; retention exempt at `supervisor_queue_store.rs:563-567`). Later idle episodes are never reported (cas-0028). |
| 3 | Every relay arrives twice | **Confirmed, and worse: three times** | 37 ids came wake + hook + teammate (cas-1f0c). `factory_inbox.rs:180-214, 327-368`. |
| 4 | Workers repeat their reports | **Confirmed in substance; no byte-identical repeats** | 0 exact duplicate groups in `prompt_queue`. Semantic echoes after close and merge relays, the 4185932/4185934 pair, and re-sends after cap rejections. Cause: plain status cannot wake the supervisor (cas-48e5). |
| 5 | Merge-request envelope says "Evidence not supplied" and "Door: not declared" | **Confirmed** | `review_body.rs:58-83, 140-150`. The worker's body never fills these fields (`message.rs:1858-1888`). Filed as cas-8b0e. |
| 6 | MERGE REQUIRED nudge prescribes a per-task PR and blocks chat | **Confirmed** | `prompts.rs:1105-1110, 1276-1293, 1313`; `pty.rs:17, 35, 292`. Filed as cas-03ff. |
| 7 | in_reply_to works only for messages from the target | **Confirmed** | `message.rs:1516-1565`. Error seen on 4185839. Filed as cas-009c. |
| 8 | Body limits produce the error only after the message is written | **Confirmed** | Caps are in `settings.rs:894-904` and checked at `message.rs:719`, but the served schema doesn't state them. 9 worker rejections and 2 supervisor rejections. Filed as cas-3349. |
| 9 | shutdown_workers accepts but does nothing | **Plausible; confirmed by code, not reproduced** | Returns success before the daemon acts (`factory_ops.rs:3362-3379`). Daemon failures go only to the TUI and log (`queue_and_events.rs:8742-8756`). Note added to cas-00d5. |
| 10 | 12 worker_died plus 1 workers_died_together, each needing an ack | **Confirmed** | 12 + 1 rows in `supervisor_queue`; ack burst at 11:12:57–11:13:02. Deliberate per GH #197 (`epic_workers.rs:1520-1530`). Filed as cas-b06f. |
| 11 | September UNDELIVERED relays shown forever | **Confirmed** | `prompt_queue_store.rs:6373-6455`: no age-out, and retention deletes only processed rows. Filed as cas-5871. |
| 12 | GH #1186 cross-machine abandonment | **Partly fixed; live behaviour unverified** | Unknown names now fail fast (`message.rs:1499-1506`). The cloud path has an honest outbox and abandon notice (`outbox.rs:5-219`). The 900 s local abandon remains (`prompt_queue_store.rs:43`). **`peer_message_outbox` has 0 rows**, so no live cross-machine send is recorded on this machine. Filed as cas-5c2c. |
| 13 | No cross-supervisor conflict notice or handoff | **Confirmed by code** | SessionEnd has no epic handoff (`handlers_session.rs:1371-1380`). The overlap module only reports. `notify_owner` ignores liveness and remote owners (`rolling_integration.rs:1326-1461`). Filed as cas-5c2c. |
| 14 | cas-8063: peer mail during a restart targets the old id | **Confirmed by code** | Messages are addressed by agent id (`peer_mailbox.rs:511, 601`), and claims are made only for current ids (`:394-398`). Note added to cas-8063. |
| 15 | Commander status needs target=operator plus kind; terminal text isn't mirrored | **Partly refuted** | The mirror exists and produced 86 status and 19 answer rows this session (`commander_mirror.rs`). But it runs only after verified operator activity and only for Active supervisors, and its rows are then swept as abandoned (cas-d88c). |
| 16 | cas-5c89: the cloud shows operator turns but not supervisor replies | **Confirmed, with root cause** | 0 cloud-outbox rows ever, 0 of 202 relay receipts. The cloud row needs an active feed binding (`operator_cloud.rs:225-251`). The drain has no production caller (`operator_delivery.rs:420`). Note added to cas-5c89, plus cas-d88c. |
| 17 | Event reminders need cross_session, and the filter fields aren't discoverable | **Confirmed** | Error text at `factory_remind.rs:37-41, 64-70`. The served schema omits both, and its TTL text is wrong for external reminders (`tool_schema.rs:105-113`). Filed as cas-3349. |

Also found in my own spawn (not one of the supervisor's leads):
- This worker's spawn message 4185939 arrived twice: once in `[incoming messages]` and again as a teammate turn.
- Task start was refused until 11 stale broadcasts had been read.
- The supervisor's brief 4185941 arrived only after task start.

---

## 5. Good / Bad / Needs improvement

### Good
- **The staged delivery model is explicit.** There are 10 stages and 16 pending reasons, each with legal transitions (`prompt_queue_store.rs:845-1077`). Rows record wake attempts with readable reasons, so every gated row says *why*.
- **Transport is fast.** 0.1–0.2 s from enqueue to transport on every live channel (§2.1).
- **Unknown targets fail fast with a named error.** This fixes the silent half of GH #1186 (`message.rs:1499-1506`). The cloud peer outbox has honest outcomes and abandon notices (`outbox.rs:124-211`).
- **Typed envelopes decide what may wake an idle supervisor.** Blocker, merge, verification and lifecycle classes, from server-stamped senders only (`queue_and_events.rs:4659-4774, 4880-4893`). This blocks spoofed wakes.
- **Lifecycle relays are durable, keyed and replayable** (`supervisor_queue_store.rs:100-103`). The supervisor's own transitions are not echoed back to it (`supervisor_push.rs:850-865`).
- **Reminders work.** Time reminders were reliable, and the external `branch_contained_in` reminder fired when the PR landed (#2904).
- **Messages are size-bounded.** Caps exist, supervisor overflow spills to artifacts (`message.rs:757-781`), and a burst limit applies to peer-worker traffic.
- **The worker return contract** produces messages that can be checked in one read, when it is followed.
- **Peer-worker traffic is visible to the supervisor** through the copy lane.

### Bad (defects, each with a filed task)
| Problem | Evidence | Task |
|---|---|---|
| Operator replies are swept as abandoned at about 930 s | 100 of 105 this session. "operator" is missing from `prompt_poison_sweep_targets` (`queue_and_events.rs:271-286`). | cas-d88c |
| The operator outbox never relays; the cloud outbox is empty | 0 of 202 relay receipts; 0 cloud-outbox rows | cas-5c89 (note) |
| False seen receipts and false abandonment | 128 this session; 1,376 of 1,990 this month | cas-f7b0 |
| Triple reading of every pointer-woken message | 37 ids, about 21K tokens | cas-1f0c |
| Every merge park also sends a close_rejected relay | 14/14; 311/312 since October | cas-3a82 |
| New workers get the stale broadcast backlog, gating task start, and the sender gets its own broadcasts | 12 broadcasts × 18 recipients, including the sender | cas-6853 |
| "Gone quiet" nudge storms and false stall escalations | 105 nudges; 67 to one worker; 6 of 8 escalations false | cas-7156 (+ cas-6eec note) |
| Idle and stalled alarms fire only once per worker per session | 11 idle relays for 11 workers | cas-0028 |
| A requested shutdown sends a Critical relay storm | 12 + 1 relays at ~2.9K each | cas-b06f |
| Worker SendMessage bypasses every coordination rule | `pre_tool.rs:650-655, 7264-7400` | cas-dd09 |
| Reminders cannot PTY-wake a pane | 13 of 13 marked `wake_not_applicable` | cas-835e |
| Dead-letter supervisor_queue rows and a banner that never clears | 2,514 reminder_fired rows; September banner rows | cas-5871 |
| Two sessions on one clone kill each other's Violet watches | `violet_activity.rs:384-399` | cas-639c |

### Needs improvement (design gaps)
| Gap | Evidence | Task |
|---|---|---|
| The sender gets no outcome when the wake gate defers, and the stall detector ignores the gate's evidence that the recipient is busy | 4185905 / 16496 | cas-d5ab |
| The merge-request envelope reads as a failure | 14 of 14 envelopes | cas-8b0e |
| The MERGE REQUIRED nudge ignores integration-branch bundling and blocks chat | `prompts.rs:1276-1293` | cas-03ff |
| in_reply_to threading is one-directional | 4185839 | cas-009c |
| The served schema hides the caps and external-reminder rules; the reminder validator and parser disagree | `tool_schema.rs:105-113` | cas-3349 |
| No supervisor-to-supervisor handoff or conflict notice; integration notices go to dead owners; over-cap peer bodies point at a local path | `handlers_session.rs:1371`; `rolling_integration.rs:1326-1461` | cas-5c2c (+ cas-8063 note) |
| Workers echo lifecycle relays, and plain status has no typed wake | 25 of 30 close echoes | cas-48e5 |
| Send receipts are long and identical: 179 × about 540 chars | §3 | in cas-d5ab |

---

## 6. Suggestions, ranked by impact and effort

Impact: H, M or L, judged by how many agents or decisions it touches and its token cost. Effort: S (≤1 day, one module), M (a few modules), L (cross-crate or cloud).

| Rank | Suggestion | Impact | Effort | Task |
|---|---|---|---|---|
| 1 | Give the operator lane its own sweep exemption and owner. Fix the cloud binding and drain so supervisor replies reach Commander and Cloud. | H | S (sweep) + M (binding) | cas-d88c, cas-5c89 |
| 2 | Make receipts honest: no seen receipt at claim time, no abandonment of read rows, a separate `recipient_ended_unread` reason. Everything else is measured from these fields. | H | M | cas-f7b0 |
| 3 | One body per message on Teams recipients, with truthful wake text | H | S–M | cas-1f0c |
| 4 | Debounce and coalesce director nudges; count in-flight tool calls and gate declines as activity | H | M | cas-7156, cas-d5ab |
| 5 | Broadcasts: recipient registration cutoff, supersede keys, never echo to the sender, never gate task start | H | S | cas-6853 |
| 6 | Drop the close_rejected twin on park | M | S | cas-3a82 |
| 7 | Requested shutdown sends one receipt; trim pane tails; fold into workers_died_together | M | S | cas-b06f, cas-00d5 |
| 8 | Supervisor ↔ supervisor: handoff at SessionEnd, live-owner routing, conflict notices, a live two-machine receipt in the release gate | H | L | cas-5c2c, cas-8063 |
| 9 | A typed `status/ready` wake class plus a one-message-per-event worker rule, to stop the echo loop | M | S (docs) + M (class) | cas-48e5 |
| 10 | Merge-request envelope fed from the contract's `proof:` and `ci:` lines | M | S | cas-8b0e |
| 11 | Integration-aware MERGE REQUIRED; quiet supervisor_stalled while a matching reminder is pending | M | S | cas-03ff |
| 12 | Episode-keyed idle and stalled relays | M | S | cas-0028 |
| 13 | Schema states the caps and reminder filter shapes; validator matches parser | M | S | cas-3349 |
| 14 | Two-way in_reply_to threading | L–M | S | cas-009c |
| 15 | Route SendMessage through `message_send` | M (latent) | S | cas-dd09 |
| 16 | Daemon-stamp reminders so they can wake PTY panes | L (Claude) / H (Codex supervisor) | S | cas-835e |
| 17 | Dead-letter cleanup and banner age-out | L | S | cas-5871 |
| 18 | Per-session Violet watches and owner-routed claims | L | S | cas-639c |

**How to make it great.** Make three properties hold everywhere:
1. **One truth per message.** Make a row's stage the single source of what happened. Each reader then sees one body, and dashboards, banners and notices all agree with what the agent actually read.
2. **Every alarm is an episode.** Each alert is keyed to a real episode, debounced, aware of activity, and coalesced when the reader is busy. A supervisor-initiated action produces one receipt, not alarms.
3. **Every party has a live address.** Operator, peer supervisor and dead owner each have a delivery lane, a liveness check and a named failure. Identity follows the name across restarts.

Measure the result with the queries in §2 as release SLOs:
- false-abandon rate = 0;
- operator relay receipts = 100%;
- duplicate body reads = 0;
- nudges per worker-hour ≤ 1.

---

## 7. Filed tasks

**New tasks**
- **P1**
  - cas-d88c: operator rows swept as abandoned
  - cas-f7b0: receipt honesty
  - cas-6853: broadcast backlog and echo
  - cas-1f0c: triple read
  - cas-7156: nudge storms and false stalls
  - cas-b06f: shutdown relay storm
  - cas-5c2c: supervisor-to-supervisor handoff and conflicts
- **P2**
  - cas-3a82: close_rejected twin
  - cas-0028: once-per-session idle and stalled alarms
  - cas-8b0e: merge-request envelope
  - cas-03ff: MERGE REQUIRED nudge
  - cas-009c: in_reply_to threading
  - cas-3349: schema limits and reminder rules
  - cas-d5ab: gate feedback and stall evidence
  - cas-dd09: SendMessage bypass
  - cas-48e5: skills, one message per event, typed status
- **P3**
  - cas-5871: dead letters and banner
  - cas-639c: Violet watches
  - cas-835e: reminder wake stamp

**Evidence notes on existing tasks**: cas-5c89, cas-00d5, cas-8063, cas-6eec.

## 8. Limits of this review
- Cross-machine peer messaging could not be measured live: the outbox has 0 rows on this machine. The far machine's database was not read.
- shutdown_workers (lead 9) is confirmed from code only.
- The SendMessage bypass is a code finding, not observed in data.
- Codex and Grok worker transcripts were not analysed: there is no Claude transcript directory for steady-swan-22, warm-fox-56 or golden-gazelle-73.
- Transcript classifiers are regex-based. Treat their counts as close estimates; the DB counts are exact.
