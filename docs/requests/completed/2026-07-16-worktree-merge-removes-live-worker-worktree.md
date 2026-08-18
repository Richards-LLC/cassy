# Bug: coordination worktree_merge removes a live worker's worktree mid-session

**Date:** 2026-07-16
**Reporter:** loyal-merlin-56 (Ozer factory supervisor, session 1e692247)
**Severity:** P2 — breaks active workers, recoverable manually
**Cassy version:** 2.27.0 (9b52e17-dirty 2026-07-16)

## What happened

During epic cas-c446 (Ozer), supervisor drained the merge queue with:

```
mcp__cas__coordination action=worktree_merge id=hv-food branch=epic/... force=true
```

The merge succeeded (commit e550ef30), but the tool also **removed the worker's
worktree and local branch** while worker `hv-food` was still alive with two more
assigned open tasks (cas-57ba8, cas-de1e5):

- `/home/pippenz/Petrastella/ozer/.cas/worktrees/hv-food` → ENOENT; worker's shell
  failed on cwd mid-task and it had to manually `git worktree add` to recover.
- Local branch `factory/hv-food` stopped resolving (`git rev-parse` fatal); only
  `origin/factory/hv-food` survived because the worker had pushed.
- Same for `hv-reco`/`st-layout` merges (95eccf89, 848bcf6f).

## Expected

`worktree_merge` should either (a) leave the worktree + branch intact when the
assignee has other open/in_progress tasks or a fresh heartbeat, or (b) require an
explicit `cleanup=true` flag for consume-on-merge semantics. Merging a worker's
completed commits is a mid-epic operation; the worktree is the worker's home for
the rest of the lane.

## Repro

1. Spawn isolated worker, assign 2+ tasks.
2. Worker commits + pushes task 1; supervisor runs `worktree_merge id=<worker> branch=epic/... force=true`.
3. Worker's next shell command in its worktree → ENOENT.

## Notes

- `force=true` was passed (worktree dirty with only untracked `.husky/_/`); if
  force implies consume, that coupling is surprising — `force` is documented as
  "dirty worktree override", not "remove worktree after merge".
- Recovery that worked: `git worktree add .cas/worktrees/<name> <origin-branch>`.


## Completion

- **completed:** 2026-07-21
- **epic:** cas-887b — Factory reliability: open docs/requests bugs → main
- **completed_by:** cas-369f
- **status:** Fixed on epic tip; report archived from `docs/requests/`.
