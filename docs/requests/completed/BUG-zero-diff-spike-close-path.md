> **Disposition (2026-08-07, cas-ab75):** RESOLVED — filed as [#62](https://github.com/pippenz/cas/issues/62) (closed completed). Fix verified on `main`: `a0cf45c5` "fix(cas-e74c): scope close merge guard to the task's own delivery" (symptoms 3–4) and `e18e18cb` "fix(cas-1932): unblock the zero-diff spike close path" (symptoms 1–2 plus the approved-verification lookup miss). Archived.

> Migrated to GitHub Issues: [#62](https://github.com/pippenz/cas/issues/62)

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

## Addendum 2026-07-31 (later same day): Symptom 3 — epic-less task inherits the worker's whole persistent branch

Task cas-297d (zero-commit relay task, no epic association) assigned to a worker whose persistent factory branch carried 34 commits — all belonging to OTHER tasks and already merged into their respective epic branches. Close was rejected with `MERGE REQUIRED: factory/clever-octopus-61 has 34 commit(s) not on master`, and the guard is explicitly unbypassable ("data-state guard, not a review gate"). The demanded remediation (merge the branch to master) would have prematurely landed unreleased epic work on trunk. Workaround used: `task action=reset` to strip the assignee/factory anchor, then supervisor close — which also skipped verification lookup ("orphaned task, no assignee"), losing the audit linkage.

Suggested fix: the merge-state guard should scope to commits attributable to THIS task (e.g. commits since task start / referencing the task id), and an epic-less task should not resolve its merge target to master when the branch's commits are already merged to epic branches. A task with zero task-attributable commits should not trip the guard at all.

## Addendum 2026-07-31 (Symptom 4): guard ignores the branch actually used, keys on the registered worker branch

Task cas-5d90: to avoid Symptom 3, the worker deliberately did its single-commit work on a CLEAN task-local branch (`factory/clever-octopus-61-cas-5d90`, based on the task's epic tip, containing only commit b323c85), which the supervisor merged into the epic and pushed BEFORE the close attempt (`git merge-base --is-ancestor` verified). Close still bounced `MERGE REQUIRED` — the guard evaluated the worker's REGISTERED persistent branch (`factory/clever-octopus-61`, 36 unrelated commits) rather than the branch the commit receipt lives on. So even the disciplined workaround (clean per-task branch, merged first, receipt supplied) cannot satisfy the gate. The guard should resolve merge-state from the commit_receipt's branch/ancestry, not the worker's registered branch name.
