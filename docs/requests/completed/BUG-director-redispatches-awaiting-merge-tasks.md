---
from: Penguinz factory session Penguinz-eager-cheetah-11
date: 2026-07-29
priority: P2
cas_task: cas-ef0a3
status: completed
---

# Director re-dispatches AwaitingMerge tasks to their own workers

## Problem

When a worker's `task close` is rejected by the MERGE REQUIRED gate, the task
parks as `AwaitingMerge` with the worker's lease released. The director's
dispatcher then treats that task as assignable and re-dispatches it — to the
same worker that just finished it — with a fresh "start this task" style
dispatch.

Observed **three for three** in one factory session (Penguinz-eager-cheetah-11,
2026-07-29), all for worker silent-newt-20:

- `cas-2949` — parked AwaitingMerge 14:32 UTC; stale rescue/start dispatch
  arrived after completion; worker had to refuse and re-litigate.
- `cas-00d4` — same pattern after parking at 14:48 UTC.
- `cas-fcf0` — same pattern after parking at 14:55 UTC.

Each occurrence cost a supervisor round-trip to disprove ("I am NOT
re-starting X, it is finished"). The worker was careful; the real risk is a
less-careful worker taking the bait and **redoing destructive work** — in the
cas-fcf0 case, re-forcing a real CUDA OOM on a live desktop GPU; in other
factories, re-running migrations or host mutations that were already applied.

## Expected behavior

The dispatcher should never treat `AwaitingMerge` as assignable/idle work.
An AwaitingMerge task has exactly one owner action outstanding — the
supervisor's merge — and zero worker actions. Dispatch decisions appear to key
off task assignment/lease state rather than task status.

## Suggested fix

Skip tasks in `awaiting_merge` (and arguably `blocked`) in the director's
dispatch/idle-worker-nudge paths; if a nudge is desired, target the
**supervisor** ("merge pending for factory/<worker>") instead of the worker.

## Repro sketch

1. Factory session with merge-required close gate enabled.
2. Worker completes a task; close returns MERGE REQUIRED; task parks
   AwaitingMerge.
3. Observe the director's next dispatch cycle re-offering/starting the same
   task at the same worker before the supervisor merges.

## Resolution and current-path trace

Verified against v2.36.0 on 2026-07-29. The earlier cas-b16d and cas-2ca9
fixes covered most paths, but two delivery-time status races remained because
`DirectorData::in_progress_tasks` is a visibility bucket, not an assignability
bucket: it intentionally contains `InProgress`, `PendingSupervisorReview`, and
`AwaitingMerge` (`crates/cas-factory/src/director.rs:289-324`).

- Assignment event detection — safe. `events.rs:824-869` emits
  `TaskAssigned` only for `Open | InProgress` and de-duplicates each
  `(task, assignee)` pair.
- Assignment delivery — fixed here. The fresh-snapshot recheck in
  `prompts.rs:494-541` used to accept the entire `in_progress_tasks` bucket,
  allowing an event detected before close to survive after the task parked.
  It now requires actual status `Open | InProgress`; the worker-directed
  start prompt at `prompts.rs:980-1002` is therefore unreachable for
  `AwaitingMerge`.
- Assigned-Open stall rescue — safe at detection. `events.rs:1003-1058`
  explicitly selects only `Open` tasks.
- Active-task stall rescue — safe at detection. `current_task` is populated
  only from `InProgress` tasks (`crates/cas-factory/src/director.rs:255-263`),
  and `events.rs:1065-1205` uses that field.
- Stall rescue delivery — fixed here. The delivery recheck in
  `prompts.rs:436-474` also used to accept the visibility bucket wholesale,
  so a queued worker-directed rescue nudge (`prompts.rs:1281-1341`) could
  survive an `InProgress` to `AwaitingMerge` transition. It now requires
  `Open | InProgress`.
- Idle-worker ready nudge — safe. `dispatchable_ready_count` at
  `prompts.rs:46-60` requires `Open`, unassigned, and dependency-ungated.
  The assignment suggestion at `prompts.rs:1238-1278` targets the supervisor,
  never the worker.
- Newly-registered-worker ready nudge — safe. It uses the same
  `dispatchable_ready_count` at `prompts.rs:1412-1428` and targets the
  supervisor.
- Awaiting-merge idle nudge — safe and intentionally supervisor-directed.
  `crates/cas-factory/src/director.rs:495-532` reconstructs the released-lease
  park as `active_lease`; `prompts.rs:1169-1235` selects the MERGE REQUIRED
  wording and targets the supervisor. The regression at
  `prompts.rs:4409-4452` proves an `AwaitingMerge` park is not worded as
  assignable even when real ready work also exists.
- Orphan/dead-worker rescue — safe. `orphan_recovery.rs:28-37` protects
  `AwaitingMerge`, candidate discovery at `:64-83` includes only
  `InProgress | Blocked`, and the mutation rechecks protection at `:151-185`.
- Shutdown/failed-spawn rescue — safe.
  `epic_workers.rs:276-328` reopens only `Open | InProgress | Blocked`, while
  the surgical preassignment cleanup at `:335-367` explicitly skips
  `AwaitingMerge`.

Regressions at `prompts.rs:1818-1879` reproduce both delivery races, prove the
parked task is dropped while a genuinely `Open` assignment still survives,
and ensure no worker-directed stalled/rescue nudge survives the park.
