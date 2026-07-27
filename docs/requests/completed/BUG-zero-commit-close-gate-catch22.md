---
from: ozer factory — session ozer-warm-owl-12, supervisor lively-dolphin-39
date: 2026-07-27
priority: P1
cas_task: cas-52ec, cas-6325 (both already filed; this adds field evidence at volume)
---

# ZERO-COMMIT close gate makes the supervisor merge the thing that blocks the close

Highest-cost defect of the session by a wide margin. Seven supervisor-side closes, dozens of wasted worker turns, and multiple workers idling while holding finished work.

## Symptoms

The documented worker flow is: implement → push → `task close`. The close gate rejects with `MERGE REQUIRED`, parks the task in `awaiting_merge`, and releases the lease. The supervisor then merges `factory/<worker>` into the epic. The worker retries the close — and it is rejected **again**, now because their branch has zero commits ahead of the epic.

So the supervisor merge, which the first rejection demanded, creates the exact state the second rejection refuses. There is no sequence of worker actions that closes the task.

## Concrete evidence

Seven tasks required a supervisor escape-hatch close in one session, each after verifying `git log <epic>..factory/<worker>` returned empty:

| Task | Worker | Merge commit |
|---|---|---|
| cas-5a7a | zealous-marten-56 | e5e1defe |
| cas-9923 | quick-sparrow-15 | e3e70262 |
| cas-93f1 | zealous-marten-56 | d3dfa7cc |
| cas-7ffc | quick-puma-7 | 1d8e9d42 |
| cas-9ec6 | tender-hound-11 | 21a3d325 |
| cas-09e6 | quick-sparrow-15 | a8c42cc7 |
| cas-8f6a | zealous-marten-56 | 9f337cb0 |

Workers looped: each rejection produced a fresh "please re-check reachability and merge if still needed" message to the supervisor for an already-merged branch. `quick-sparrow-15` and `tender-hound-11` each sent three such messages for the same task.

## Workaround applied

Supervisor verifies `git log epic..factory/X` is empty, then closes with `bypass_code_review: true` and a reason documenting the catch-22. Roughly two minutes and one model turn per task, plus the worker turns spent looping. Late in the session I issued a standing rule to all workers: "if a close is rejected with MERGE REQUIRED after you have pushed, message me once and move on" — which suppresses the loop but does not fix it.

## Likely root cause

Two independent gate conditions evaluated against the same branch state, with opposite requirements:
- `MERGE REQUIRED` fires when the worker branch has commits not on the epic.
- `ZERO-COMMIT` fires when the worker branch has no commits ahead of the epic.

Satisfying the first necessarily produces the second. Nothing appears to record that the branch's commits are *already reachable from the epic*, as opposed to never having existed.

## Proposed fix

- a) **Reachability, not ahead-ness (leaned).** Replace the ZERO-COMMIT check with "are this task's commits reachable from the epic branch?" `git merge-base --is-ancestor <worker-tip> <epic>` answers it directly and is true both before and after the supervisor merge.
- b) Record the merge. When the supervisor merges a worker branch, stamp the task with the resulting epic commit; the close gate then accepts a task whose recorded merge commit is on the epic, regardless of current branch topology.
- c) Make `awaiting_merge` → `closed` a supervisor-side transition by design rather than an escape hatch, so the worker never retries. This is what actually happened seven times; the tooling should model it instead of treating it as an exception.

Option (a) is smallest and fixes the root asymmetry. (b) is more robust if branches are ever re-used across tasks, which they are here — `factory/<worker>` is long-lived and carries several tasks in sequence.

---

## Resolution

**Resolved 2026-07-27 — shipped in cas v2.29.0 (`origin/main`).** Task: cas-7efe

Root cause: five close gates resolved `parent_branch` independently and four fell back to a hardcoded `"main"`, so `commit_is_merged_into_parent(anchor, "main")` returned false for work merged into an epic branch. Unified onto one resolver in `7acb9b6` (merge `01348e7`).
