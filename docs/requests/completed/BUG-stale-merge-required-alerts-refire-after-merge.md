---
from: ozer factory — session ozer-warm-owl-12, supervisor lively-dolphin-39
date: 2026-07-27
priority: P2
cas_task: (none)
---

# MERGE REQUIRED / WorkerIdle alerts re-fire for branches already merged and tasks already closed

Distinct from the ZERO-COMMIT catch-22 (see `BUG-zero-commit-close-gate-catch22.md`), though triggered by it. That bug is about gate logic; this one is about alerts having no freshness check before they reach a human.

## Symptoms

The director emits `⚠️ MERGE REQUIRED — supervisor action needed` with a full five-step remediation script, for branches that are already fully merged into the epic and, in several cases, for tasks the supervisor has already closed. The alert asserts current state ("Worker X is idle while task Y is awaiting_merge") without re-checking it at send time.

Each alert is indistinguishable from a real one, so it costs a full supervisor turn to disprove — `epic_status` plus a `git log epic..factory/X` per branch.

## Concrete evidence

- cas-5a7a: MERGE REQUIRED received **after** the branch merged as `e5e1defe`. Received again after a second merge. `epic_status` at the time printed `✓ All child factory branches are merged into the parent epic branch` with `Unmerged 0` on every row — while the alert claimed otherwise.
- cas-9923: alert arrived after merge `e3e70262` and after supervisor close.
- cas-9ec6 and cas-09e6: both alerted after being merged (`21a3d325`, `a8c42cc7`) *and* closed.
- cas-8f6a: alert arrived after merge `9f337cb0` and supervisor close, instructing a merge of a branch with zero unmerged commits.

At least six stale alerts in one session, each carrying an authoritative-sounding instruction to perform a merge that was already done.

## Workaround applied

Treat every MERGE REQUIRED as unverified. Run `task list status=awaiting_merge` and `git log <epic>..factory/<worker>` before acting. Twice I nearly re-merged an already-merged branch on the alert's instruction.

## Likely root cause

The alert is generated from a task-state snapshot at the moment the close was rejected and delivered later, with no re-read of task status or branch topology before send. Because the ZERO-COMMIT bug leaves tasks parked in `awaiting_merge` indefinitely, the stale trigger persists and re-fires.

## Proposed fix

- a) **Re-validate at send time (leaned).** Before emitting, confirm the task is still `awaiting_merge` and the branch still has unmerged commits. Drop the alert otherwise. Cheap, and removes the entire class.
- b) Include the evidence in the alert — unmerged commit count and the epic SHA it was computed against — so a supervisor can dismiss it in one glance instead of running two verification commands.
- c) Suppress repeats: do not re-emit for the same (task, branch) pair without an intervening state change.

Worth noting the alert's own step 1 tells the supervisor to run `epic_status` to confirm — and in every case here `epic_status` contradicted the alert. The check the alert recommends is one the alert could have run itself.

---

## Resolution

**Resolved 2026-07-27 — shipped in cas v2.29.0 (`origin/main`).** Task: cas-6883

MERGE REQUIRED and WorkerIdle alerts now re-validate against live git state at send time rather than firing from stale queued state (`b25a2b1`).
