# BUG: awaiting_merge has no lifecycle path for an amendment-required review verdict

**Observed:** 2026-07-31, Penguinz factory session `Penguinz-proud-crane-49`, task cas-7088, cas 2.30.0.

## Symptom

Flow that deadlocks:

1. Worker's `task close` returns MERGE REQUIRED → task parked `awaiting_merge`, lease released.
2. Supervisor merges the factory branch into the epic (epic_status: 0 unmerged) — merge is fully complete.
3. Supervisor's post-merge multi-persona review returns **AMENDMENT REQUIRED** (recorded as a decision note).
4. Task must now become actionable again for the amendment, but it is still `awaiting_merge`:
   - `task start` is rejected: "Cannot start a task that is awaiting merge. The worker work is already complete... restart is only permitted after a genuine merge conflict."
   - Re-close is wrong: the amendment hasn't been implemented.

Original assignee being gone (new session, new worker) makes this worse: the replacement worker is assigned, receives the amendment brief, and cannot start.

## Workaround used

Supervisor forced `task update status=open` with an audit note, after which the new worker's `start` succeeded. This works but is an undocumented manual override of the state machine.

## Suggested fix

`awaiting_merge` needs a sanctioned exit for the amendment case — either:
- supervisor action (e.g. `task reopen`/`update`) that is documented as the amendment path and logs the transition, or
- the review-verdict recording itself (amendment-required) transitions the task `awaiting_merge → open` while preserving assignee, or
- `task start` permitted on `awaiting_merge` when the parked branch shows 0 unmerged commits AND the latest decision note is an amendment verdict.

Any of these beats "only a merge conflict unlocks start", which assumes review can never fail after merge.
