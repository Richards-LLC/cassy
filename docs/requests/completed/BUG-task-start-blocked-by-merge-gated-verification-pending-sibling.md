# BUG: `task start` is blocked while a *sibling* task is merge-gated "verification-pending"

Resolution: Already fixed by commit `2491fb97` (`cas-6a99`), which skips `pending_worktree_merge` tasks in `check_pending_verification` and added `test_task_start_not_blocked_by_merge_gated_sibling_cas_6a99`.

**Reported:** 2026-06-26 (gabber-studio, factory mode)
**Severity:** Medium — forces workers to do tracked work with no formal `start`, leaving Cassy task state inconsistent in any supervisor-deferred-merge workflow.

## Scenario
A single factory worker was assigned two related tasks to ship together on **one branch** (supervisor owns the PR + merge):
- `cas-cb85` (credit-pack PostHog events)
- `cas-9fd3` (shared checkout shell)

The worker:
1. Completed + committed `cas-cb85` (type-check exit 0).
2. Tried to `task close cas-cb85` → **correctly** merge-gated ("115 commits ahead of main" — close requires the branch to be merged; expected, this part is fine).
3. Tried to `task start cas-9fd3` → **BLOCKED by Cassy** because `cas-cb85` is "unverified / verification-pending".

There is **no declared dependency** between the two tasks — they're just both assigned to the same worker and bundled on the same branch.

## Why this is a problem
- In a **supervisor-deferred-merge** workflow (the norm here: staging-first, supervisor reviews + merges), a task legitimately sits in `verification-pending` for a while because it *cannot* be verified until its branch merges — which the worker doesn't control.
- Coupling the *start* of an independent/bundled task to the *verification* state of a previous one means the worker can't formally `start cas-9fd3`. It proceeds anyway (work still gets done on the branch), but Cassy now shows inconsistent state (work happening on a task that was never `start`ed), and the supervisor must hand-reconcile both tasks at merge.
- This compounds the already-known merge-gated-close gap (closed ≠ merged): a worker can be stuck unable to *close* task A and unable to *start* task B simultaneously.

## Expected
Starting task B should NOT be blocked by task A merely being `verification-pending`, unless B explicitly `blocks`-depends on A. A `verification-pending` (awaiting-merge) task is not "in flight" in a way that should gate unrelated work.

## Suggested fixes (any one)
- Don't gate `task start` on a sibling's `verification-pending` state when there's no explicit dependency edge.
- Treat "merge-gated / awaiting-merge" as a distinct, non-blocking state (separate from "actively-being-verified").
- Provide a supervisor/worker override to start the next task with an audit note (mirrors the existing dual-gate close fallback).

## Operational fallback used
Worker proceeded with `cas-9fd3` on the same branch **without** a formal `start`; supervisor will reconcile both tasks' Cassy state at merge (close both once the bundled PR lands on staging). Related: the verification-jail merge-gap and the dual-gate-close fallback already documented internally.
