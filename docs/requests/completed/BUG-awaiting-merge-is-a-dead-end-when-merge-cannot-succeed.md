---
from: ozer factory — session ozer-warm-owl-12, supervisor lively-dolphin-39
date: 2026-07-27
priority: P1
cas_task: (none)
---

# `awaiting_merge` is a dead end when the merge genuinely cannot succeed — and it silently parks real work

Related to but distinct from the ZERO-COMMIT catch-22 (`BUG-zero-commit-close-gate-catch22.md`). That one is about a merge that already happened. This one is about a merge that *cannot* happen.

## Symptoms

When a worker's close is rejected with `MERGE REQUIRED`, the task parks in `awaiting_merge` and the lease is released. The state assumes the only remaining step is a mechanical supervisor merge.

If the branch **conflicts**, there is no exit:

- The supervisor cannot merge — `worktree_merge` fails on the conflict.
- The worker cannot fix it — `task start` is refused *because* the task is `awaiting_merge`:
  ```
  Blocked: `task start cas-8cbc` was rejected because the task is already `awaiting_merge`;
  Cassy says worker work is complete and the supervisor must merge the factory branch, then retry close.
  ```
- No other action transitions it.

The task now reads as "done, pending a formality" while containing unfinished work that nobody can touch.

## Concrete evidence

cas-8cbc. The worker pushed, the close was rejected with MERGE REQUIRED, the task parked in `awaiting_merge`. The branch had a real content conflict in `apps/frontend/stores/sleepRhythmStore.ts` — the epic had moved three times underneath it (196a65d4, 3a6d0cbd, f390c5ce).

The original worker was then lost in a fleet restart. A fresh worker was assigned, attempted `task start`, and was refused with the message above. It correctly reported the refusal instead of forcing anything.

Only a supervisor `task reset --force` cleared it.

## Why this matters more than the mechanics

The parked task contained two things the client had explicitly asked for — a toggle rename the CEO requested by name, and the fix for a "I clicked Cancel and it downloaded anyway" bug she reported. The commits were on a branch belonging to a worker that no longer existed.

Nothing flagged any of this. `epic_status` showed the branch as unmerged, which is indistinguishable from "not merged yet". The task status said `awaiting_merge`, which reads as complete. The only reason it surfaced was that a replacement worker tried to start it and hit the refusal.

A state that means "finished" and a state that means "stuck with unfinished work" should not look identical.

## Workaround applied

`task reset --force`, reassign, and hand-write the recovery path (which branch holds the commits, which worker authored them, what the conflict is) into a task note — because none of that survives a fleet restart otherwise.

## Proposed fix

- a) **Let a worker start an `awaiting_merge` task (leaned).** If the merge cannot be performed, the work is not actually complete, and the worker is the right party to resolve it. Starting should be permitted and should transition back to `in_progress`.
- b) Distinguish the cases. `awaiting_merge` should mean "mergeable, queued for supervisor". Introduce a distinct state (or flag) for "merge attempted and conflicted" that is visibly NOT complete and is assignable. `merge-tree` can determine which at close time.
- c) At minimum, name the alternative in the refusal. The error tells the worker the supervisor must merge; it should also say what to do when that merge fails.
- d) Record the branch on the task at close time. When a worker is lost, the commits become orphaned with nothing linking them to the task — recovery today depends on a supervisor remembering.

## Resolution (cas-5054, 2026-07-29)

Resolved by making conflict rework an explicit, narrow exit from `awaiting_merge`.
An assigned worker may restart a parked task only when Cassy has recorded a genuine
merge conflict; cleanly mergeable tasks remain parked with guidance to wait for
the supervisor. Restarting records a conflict-rework decision and atomically
clears the prior factory anchor, parked branch, and conflict flag so the next
close evaluates the resolved work as a fresh close cycle.
