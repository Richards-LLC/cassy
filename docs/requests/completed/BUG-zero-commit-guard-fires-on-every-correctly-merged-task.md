---
from: gabber-studio (pippenz @ /home/pippenz/Petrastella/gabber-studio)
date: 2026-07-28
priority: P2
---

# ZERO-COMMIT guard rejects the close of every task that was merged before closing

## Resolution (Cassy v2.33.0)

The reported merge-before-close path is fixed when Cassy captured the task's
commit anchor before the merge:

- `cas-cli/src/hooks/handlers/handlers_events/attribution.rs:207-230` resolves
  the created commit SHA during the `PostToolUse` commit hook, and
  `:313-369` records it on the worker's one active task.
- `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:4477-4516` accepts
  a zero-ahead worker branch when that recorded anchor is reachable from the
  epic branch. The regression test at `:11079-11144`,
  `cas127f_post_merge_ancestor_anchor_proceeds` covers the report's exact
  ancestry shape: the task commit is on the epic, the synchronized worker
  branch is zero commits ahead, and close proceeds.

For an existing repository, upgrade to Cassy v2.33.0 and run `cas update`.
Claude users should ensure the generated/global `PostToolUse` hook is active.
Codex users must also review the installed hook with `/hooks`, then add and
commit `.codex/hooks.json` before spawning worker worktrees so each worktree
receives the commit-attribution hook.

There is one residual case: already-merged work with no captured task anchor
still produces the ZERO-COMMIT rejection. Branch-tip ancestry alone cannot
safely distinguish that state from a task that produced no commit, so Cassy
does not guess. The task-attributed commit-receipt fix is tracked as
`cas-26bb`. Until it lands, a supervisor should audit the task commit's
reachability from both the local and origin target branch before using the
documented override.

The three gabber-studio occurrences in this report predate v2.33.0's anchor
path. The report is therefore resolved as triaged: the anchored reproduction
is fixed, upgrade/install guidance is documented above, and the no-anchor
residual has an owned follow-up.

## Summary

The ZERO-COMMIT close guard counts commits present on the worker's factory branch but **not** on the epic branch, and rejects the close when that count is 0. After a successful `git merge --no-ff factory/<worker>` into the epic branch, that count is **necessarily 0** — the merge is exactly what makes it 0.

So the guard fires on the correct, intended workflow: supervisor merges, then the worker re-closes. It fired **three times in a single session** on three different tasks and two different workers, always with the same shape.

Every occurrence required a supervisor `bypass_code_review=true` override. The guard produced no true positives in that session.

## Concrete failure mode

Observed 2026-07-28 on epic `cas-3e6b`, three times:

| Task | Worker | Commit | Epic tip at close |
|---|---|---|---|
| `cas-c941` | dash-frontend-truth | — | — |
| `cas-8a6b` | dash-frontend-truth | `55af1e5b8` | `12c7f7755` |
| `cas-331b` | dash-frontend-truth | `3700cf4c0` | `e19d943ba` |

Precondition at the moment of each rejected close (verified with git, not inferred):

```
git merge-base --is-ancestor <commit> <epic>          -> true  (local)
git merge-base --is-ancestor <commit> origin/<epic>   -> true  (origin)
git rev-list --count <epic>..factory/<worker>         -> 0
```

The work was merged, pushed, and reachable from both the local and origin epic branch. The guard read that state as "this task has no commits" and blocked the close.

## Why it happens

The check appears to be, in effect:

```
unmerged = rev-list --count <epic>..factory/<worker>
if unmerged == 0: reject("ZERO-COMMIT")
```

That predicate answers "does this branch have work not yet on the epic?" It is being used to answer a different question: "did this task produce any work?" Those coincide only *before* the merge. The documented factory workflow is merge-then-close, so the guard is evaluated precisely when its predicate is guaranteed false.

This is the same class of defect the gabber-studio epic itself was about: **a check whose predicate does not express the condition it claims to test.**

## Proposed fix (options, in preference order)

1. **Ask the right question.** Satisfy the guard when commits attributable to the task are *reachable from the epic branch*, not when they are absent from it. Attribution can come from the task id in the commit message/trailer, or from the merge commit recorded at merge time.
2. **Record the merge, then trust it.** Have the supervisor merge path persist the merged SHA on the task (e.g. `merged_at` / `merged_sha`). The close guard checks that field first and only falls back to the rev-list count when it is absent.
3. **Invert the fallback.** If `rev-list --count <epic>..factory/<worker>` is 0, do not reject immediately — check whether the branch tip is an ancestor of the epic. If it is, the work is merged: pass. If it is not, the branch is genuinely empty: reject.
4. **Cheapest mitigation, if none of the above land.** Make the rejection message say "0 unmerged commits — if you already merged, this is expected; supervisor should close with bypass_code_review=true". Right now the wording reads as data loss, and a worker's reasonable first reaction is to suspect its own work vanished.

Option 3 is a small, local change and would have eliminated all three occurrences.

## What we did to recover

Supervisor verified the guard's precondition independently with `git merge-base --is-ancestor` against **both** the local and origin epic branch, then closed with `bypass_code_review=true`, recording the verification in the close reason.

Notably, the workers diagnosed this correctly themselves each time — including citing the guard's own remediation path — rather than assuming their work was lost or attempting to force a close. The failure is recoverable but it costs a full supervisor round trip every time, and it trains people to reach for the bypass, which is corrosive: a bypass that is routine stops being a safeguard.

## Related observations from the same session

Three other Cassy bookkeeping signals disagreed with git during the same epic. Git was correct in every case:

- **`epic_status` reported false stranded commits.** It claimed two child tasks carried stranded factory commits; `git log <epic>..factory/*` was empty for both — they were fully merged.
- **A task closed CLEAN with its code unmerged.** `cas-3f2e` closed with no MERGE REQUIRED prompt at all while its fix existed only on the factory branch. This is the dangerous inverse: an `awaiting_merge` task stays visible and keeps prompting, but a *closed* task is invisible and nothing will ever surface it. It was caught only because the worker checked ancestry on its own initiative after the clean close.
- **The verification jail refused `task action=start`** on a new task, citing "unverified task: cas-8a6b", when cas-8a6b was already closed.

Taken together: Cassy's merge bookkeeping is unreliable in **both** directions — it invents strandedness for merged work, and it clean-closes unmerged work. Suggest treating git ancestry as the single source of truth for merge state, and deriving all three of these signals from it.

## Sharper framing: one hardcoded notion of "merged", failing in both directions

A worker on the same epic put this better than the original write-up did, and it is the clearest statement of the underlying defect:

> This is the inverse of the cas-3f2e problem: there, a clean close hid genuinely **unmerged** code; here, a blocked close hid genuinely **shipped** code. Both are the guard having one hardcoded notion of "merged".

The guard assumes exactly one destination — the epic branch — and treats reachability from it as the definition of done. Both failure directions follow from that single assumption:

- Work merged to the epic but closed afterwards -> rev-list is 0 -> **rejected though complete** (the three occurrences above).
- Work merged somewhere else entirely, e.g. a hotfix routed straight to `staging` -> not reachable from a now-stale epic -> **rejected though shipped and deployed**.
- Work merged nowhere at all -> can still slip through a clean close (cas-3f2e).

A fourth rejection occurred on this epic when a P0 hotfix was deliberately routed to `staging` rather than the epic branch. That one was a TRUE positive on a real condition — the factory branch did contain commits the epic lacked — and it is recorded here for completeness, NOT as a fifth instance of the zero-commit bug. Conflating them would overstate the report. It was resolved by fast-forwarding the epic to staging, not by an override.

The fix implied by all of this: the guard should ask "is this task's work reachable from any branch this task was legitimately targeted at", with the merge target recorded at merge time, rather than assuming the epic branch is the only possible destination.

## Reproducer (synthetic)

1. Create an epic and one child task; spawn an isolated worker.
2. Worker commits and pushes to `factory/<worker>`.
3. Worker attempts close -> `MERGE REQUIRED` (correct).
4. Supervisor: `git merge --no-ff factory/<worker>` into the epic branch, push.
5. Worker attempts close again -> **ZERO-COMMIT rejection**, despite the work being merged and reachable.

Step 5 is the bug. Step 4 is what causes it.

## Severity

P2. Not data loss and fully recoverable, but it fires on the happy path, costs a supervisor round trip every time, and normalises `bypass_code_review` — which is the one override that should stay rare enough to be noticed.
