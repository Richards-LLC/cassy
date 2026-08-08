> **Disposition (2026-08-07, cas-ab75):** RESOLVED — filed as [#55](https://github.com/pippenz/cas/issues/55) (closed completed). Fix verified on `main`: `d0d95f29` "fix(cas-aee6): sanctioned request_changes exit from awaiting_merge" (epic cas-7e66); `task action=request_changes` is now the documented exit from `awaiting_merge` with the assignee preserved. Archived.

> Migrated to GitHub Issues: [#55](https://github.com/pippenz/cas/issues/55)

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

---

## Recurrence 2026-08-02 — declined-merge variant, and the documented workaround is now blocked

**Observed:** Woodworking factory session `Woodworking-jolly-octopus-76`, tasks cas-6fac and cas-bdcb, cas 2.38.2.

Two things are new since the original report.

### 1. A second trigger: the supervisor DECLINES the merge

The original report covers merged-then-amendment-required. This variant never merges at all. Worker delivers, `close` returns MERGE REQUIRED, task parks `awaiting_merge` — and on review the supervisor **rejects the work and does not merge it**. Here the deliverable (cat silhouettes) was structurally valid, analytically correct, and did not depict a cat, so the branch was correctly not taken.

The task must become actionable again for a redraw, but:

- `task start` — rejected, task is `awaiting_merge` (worker attempted; CAS suggested `start` as the remedy in its own MERGE REQUIRED text, which is misleading when the merge was refused rather than pending).
- `task reopen` — rejected: *"Task is already awaiting_merge (only closed or blocked tasks can be reopened)"*. The error then suggests `task update status=open`, which is also refused — see below.

This is arguably the more common case than amendment-after-merge: any review that rejects work outright lands here.

### 2. The recommended workaround is now blocked by the proof lock

The original report's workaround was "supervisor forced `task update status=open`". That path no longer works:

```
task update id=cas-6fac status=open
→ MCP error -32602: DELIVERY PROOF SCOPE LOCKED: task cas-6fac has an active
  exact verification/delivery proof boundary. Refusing review-relevant update
  fields [status]. Append progress with notes only.
```

So the state machine now refuses both the sanctioned transitions AND the documented manual override. The only path left was:

```
task reset id=cas-6fac force=true
```

which works but is semantically wrong for this case: `reset` is documented for reviving tasks orphaned by a dead session. It force-releases the lease, **clears the assignee**, and logs a forced-reset audit note. The worker was alive, correct, and about to redo the work — it then had to be re-assigned. The audit trail records "orphaned task recovered" when what happened was "supervisor rejected the deliverable".

### Suggested fix, extending the original

Add an explicit supervisor verdict that owns this transition, e.g. `task reject` / `task request_changes`, which:

- transitions `awaiting_merge → open` (or `in_progress`) **preserving the assignee**,
- records the rejection reason as a first-class decision note rather than a forced-reset audit line,
- is exempt from the delivery-proof lock, since recording a failed review is exactly when the proof boundary should yield.

Whatever the mechanism, the proof lock should not be able to block the recording of a negative review outcome. As it stands, a supervisor who declines work has no supported action at all.
