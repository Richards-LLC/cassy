# BUG: close path for a zero-diff spike in supervisor-review mode is a two-stage trap

**Observed:** 2026-07-31, Penguinz factory session `Penguinz-proud-crane-49`, task cas-208b, cas 2.30.0. Project runs `[code_review] owner = "supervisor"`. Worker ran in **shared mode** (no worktree) in a main checkout carrying ~64 files of pre-existing prior-factory WIP. Task was a characterization-only spike: description mandated no commits; worker's fresh proof recorded `in_scope_git_diff=clean`.

## Symptom 1 — approval/close loop

1. Worker close → task queued `pending_supervisor_review` (expected in this mode).
2. Supervisor reviews, records `verification action=add status=approved` (ver-fd59de6ef422), messages worker to close.
3. Worker close again → **re-queued to `pending_supervisor_review`** despite the approved verification on record. Worker cannot ever close.
4. Supervisor must perform the close themselves.

Either the worker's close should consume the existing approved verification, or the documented flow should say plainly: in supervisor-review mode the supervisor closes. The current shape invites a retry loop.

## Symptom 2 — CODE_REVIEW_REQUIRED on a task with zero task-produced changes

The supervisor's close was then rejected with `CODE_REVIEW_REQUIRED: this task has reviewable code changes`. The task produced **no** changes; the reviewable-diff detection evidently picked up the pre-existing dirty state of the shared main checkout and attributed it to the task. Supervisor had to use `bypass_code_review=true`.

## Suggested fixes

- Reviewable-change detection for shared-mode workers should be scoped to changes attributable to the task (e.g. diff since task start / commits referencing the task), not "checkout is dirty".
- A task whose close reason + proof declare zero commits and whose lease produced no commits should not trip the code-review gate at all.
- In supervisor-review mode, let an approved verification satisfy the review queue so exactly one party (documented) performs the final close.

## Minor

Close output said "verification skipped — assignee unknown" even though verification ver-fd59de6ef422 existed for the task — the close path did not find it.
