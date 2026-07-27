---
from: ozer factory — session ozer-warm-owl-12, supervisor lively-dolphin-39
date: 2026-07-27
priority: P1
cas_task: (none)
---

# A conflicting `worktree_merge` leaves the shared checkout mid-merge, breaking every subsequent merge

Most dangerous of today's tooling defects, because the damage lands on an *unrelated* operation and the error message points at the wrong thing.

## Symptoms

`coordination action=worktree_merge` on a branch that conflicts fails with a generic error:

```
MCP error -32603: Failed to merge worktree: Git error: Failed to execute git command:
```

It does not say what conflicted, and — critically — **it does not clean up**. The main repository checkout is left with `MERGE_HEAD` present and a partially-staged conflict.

The next `worktree_merge`, for a completely different worker and a completely different task, then fails with:

```
MCP error -32603: Failed to merge worktree: Git error: Failed to execute git command: error: you need to resolve your current index first
```

That message describes the *symptom of the previous failure*, not the branch being merged. A supervisor who did not witness the first failure has no path from that error to its cause.

## Concrete evidence

1. `worktree_merge factory/kind-lynx-53` → generic git error. Cause (found manually) was a content conflict in `apps/frontend/stores/sleepRhythmStore.ts`.
2. `worktree_merge factory/ready-fox-78` — unrelated branch, unrelated task — → `you need to resolve your current index first`.
3. `git status` in the main checkout showed the repo sitting on the epic branch with `.git/MERGE_HEAD` present and `M  apps/frontend/components/sleep-rhythm/SleepConfigScreen.vue` staged from the abandoned merge.
4. `git merge --abort` restored a clean state; the ready-fox merge then succeeded immediately.

Between (1) and (4), every merge in the factory was blocked, and the merge queue was actively filling behind it.

## Why this is worse than it looks

The shared checkout is the factory's single merge point. One conflicting branch therefore halts **all** integration until a human notices and manually aborts — and nothing in the tooling surfaces that state. `epic_status` reported normally throughout. The only tell was the error on an unrelated operation.

It also leaves a dirty working tree in the checkout, which interacts badly with the separate `.husky/_/` dirty-check defect (see `BUG-worktree-merge-blocked-by-cas-own-husky-artifact.md`): a supervisor habituated to passing `force: true` past "uncommitted changes" could force a merge on top of an abandoned conflict state.

## Workaround applied

`git merge --abort` in the main checkout, then re-run the merge. Requires the supervisor to (a) know the previous merge conflicted, (b) know it left state behind, and (c) know to look in the main checkout rather than the worker's worktree. None of that is discoverable from the error.

## Proposed fix

- a) **Abort on failure (leaned).** If the merge cannot complete, `git merge --abort` before returning the error. A failed merge should leave no trace. This alone removes the cascade.
- b) **Report the conflict.** Return the conflicting paths in the error rather than a bare "Failed to execute git command". `git merge-tree --write-tree <target> <source>` gives them without touching the working tree, and can be run *before* attempting the merge at all.
- c) Pre-flight: check mergeability with `merge-tree` and refuse cleanly with the conflict list, so the working tree is never entered in the failing case.
- d) Detect and report a pre-existing merge-in-progress on entry, instead of surfacing it as an opaque git failure on an unrelated branch.

(a) and (b) together are small and would have turned a factory-wide stall into a one-line "these paths conflict, rebase and resolve".
