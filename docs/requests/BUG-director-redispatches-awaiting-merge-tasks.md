---
from: Penguinz factory session Penguinz-eager-cheetah-11
date: 2026-07-29
priority: P2
cas_task: none (process bug observed across cas-2949, cas-00d4, cas-fcf0)
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
