---
from: Ozer supervisor (pippenz @ /home/pippenz/Petrastella/ozer)
date: 2026-07-14
priority: P2
cas_task: cas-d1a0
status: fixed
---

# BUG: `coordination worktree_list` returns "No worktrees found" while Cassy-created worktrees from sibling sessions exist

## Summary

While integrating epic `cas-ea3e`, `git worktree list` showed three worktrees:

```
/home/pippenz/Petrastella/ozer                           b3022aa6 [staging]
/home/pippenz/Petrastella/ozer/.cas/worktrees/hv-food-qa a90f98a6 [factory/hv-food-qa]
/tmp/ozer-epic-ea3e-hv                                   ec0087ce [epic/…-cas-ea3e]
```

Two were created by Cassy factory tooling in *other* sessions (a director session and its worker). Yet `mcp__cas__coordination action=worktree_list` in my session returned **"No worktrees found."** — so `worktree_show` / `worktree_merge` / `worktree_cleanup` were unusable for exactly the cleanup the factory pattern says the supervisor should run. I had to fall back to raw `git worktree remove` / `branch -D` / `push --delete`.

## Environment

- `cas 2.27.0 (dd8bcbd-dirty 2026-07-11)`, factory mode, supervisor `fast-kestrel-14`, session `07275a32-c0d5-4695-abbb-5c04663df721`, project `/home/pippenz/Petrastella/ozer`
- Worktrees created by a concurrent director session (same project, same `.cas` root)

## Expected

The worktree registry should be project-scoped (keyed under `.cas/` for the repo), not session-scoped — any supervisor in the project should see and be able to manage worktrees created by sibling/predecessor sessions. Alternatively, `worktree_list` could reconcile against `git worktree list` and report unregistered Cassy-pattern worktrees (`.cas/worktrees/*`, epic worktrees) as "untracked".

## Impact

Cross-session handoffs (director spawns workers, supervisor integrates — exactly the flow the director requested of me) lose all worktree bookkeeping: no merge-state tracking, no policy checks before cleanup, and orphaned worktrees accumulate invisibly unless someone happens to run raw git commands.

## Resolution (cas-d1a0)

**Root cause:** System B (`spawn_workers isolate=true`) never writes `WorktreeStore` rows. `worktree_list` already reconciled live git worktrees, but only under the hardcoded path `<cas_root>/worktrees`. That missed:

1. Customized `worktrees.base_path` (same path spawn/merge use)
2. Cassy-pattern worktrees outside that tree (e.g. director epic worktrees under `/tmp/…` with `epic/*` branches)

**Fix:** Project-scoped list is store rows (`.cas/cas.db`) **plus** a git reconcile of Cassy-pattern worktrees:

- Paths under configured factory base, `<cas_root>/worktrees`, or `<repo>/.claude/worktrees`
- Branches matching `factory/*`, `epic/*`, or `cas/*`
- Unregistered entries labeled `[factory]` or `[untracked]` with path shown
- Unrelated user worktrees are not listed

**Proof:** `cargo test --test worktree_surface_test` — 13 passed (including cas-d1a0 cases for custom base_path, unregistered epic outside `.cas/`, sibling factory without store row, and ignore non-Cassy worktrees).
