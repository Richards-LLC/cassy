# BUG: task close merge guard tracks a stale pre-rebase factory SHA

- **Date:** 2026-07-10
- **Reporter:** factory worker relay (downstream project session), via supervisor
- **Area:** factory / task-close merge guard
- **Severity:** Medium — blocks legitimate task close after a normal rebase
  until manually patched; a workaround exists (see below) but requires
  supervisor-side git surgery and wastes multiple retry round-trips each time
  it recurs.
- **Related:** BUG-awaitingmerge-anchor-squash-merge-2026-07-09.md (same
  family — a stored pre-integration commit identity being ancestry-checked
  against a branch tip that has since moved past it for a legitimate reason)

## Summary

The task-close "MERGE REQUIRED" guard can keep rejecting a task even after
its work is genuinely merged into the parent (epic) branch, when the
worker's commit was **rebased** onto a newer parent tip between the first
merge attempt and the final one. The guard appears to check ancestry against
a commit identity captured at an earlier point (the pre-rebase SHA), not the
worker's current branch head, so it reports "1 commit not on parent" even
though `git merge-base --is-ancestor <current-SHA> <parent-branch>` proves
the opposite.

## Reproduction

1. Worker commits task work on `factory/W1` as commit `A`, based on parent
   branch `epic/E1` at some tip.
2. Supervisor merges `A` into `epic/E1` (say as merge commit `M1`). Worker
   attempts a **different, later** task on the same branch.
3. Before that second task's close, the supervisor asks the worker to rebase
   `factory/W1` onto the now-advanced `epic/E1` tip (unrelated commits landed
   from other workers). The rebase is a fast-forward-style linear rebase with
   no conflicts; it rewrites `A` to a new SHA `A'` with identical content.
   Worker force-pushes `factory/W1` at `A'`.
4. Supervisor merges `A'` into `epic/E1` as merge commit `M2`.
5. Both worker (`git merge-base --is-ancestor A' epic/E1`) and supervisor
   independently confirm `M2`'s parent chain contains `A'` — i.e. the task's
   code is unambiguously integrated, with zero commits ahead of the parent.
6. Worker calls task-close. **Rejected**: `⚠️ MERGE REQUIRED — factory/W1 has
   1 commit(s) not on epic/E1`.
7. Retried twice more after the supervisor re-confirmed the merge (once
   quoting the exact merge commit SHA) — **identical rejection both times**,
   byte-for-byte the same guard text.
8. The rejection only cleared after the supervisor performed a manual
   **no-content `ours` merge** on the epic branch that explicitly records the
   original pre-rebase SHA `A` as an ancestor of `epic/E1` (i.e. patched the
   history so *both* `A` and `A'` independently satisfy ancestry). Task close
   then succeeded on the very next attempt with no other change.

## Expected Behavior

The merge guard should evaluate ancestry against the **worker's current
branch head** (or the SHA the task's most recent close attempt actually
references), not a commit identity captured earlier in the task's lifecycle.
A legitimate rebase-and-force-push that preserves (or supersedes) the
original change's content, followed by a real merge of the new head into the
parent, should satisfy the guard without requiring the old, now-superseded
SHA to also be independently reachable.

## Actual Behavior

The guard continued to report the task's branch as unmerged after a
confirmed, verifiable merge of the current (post-rebase) head, and did so
identically on three consecutive attempts spanning two independent
supervisor confirmations. It only cleared once the *original pre-rebase SHA*
was artificially made an ancestor of the parent branch via an unrelated
no-content merge — strongly suggesting the guard is keyed off a stored
identifier for the "original" task commit rather than re-resolving the
worker's live branch head at check time.

## Workaround (applied, not a fix)

Supervisor created a no-content `ours` merge on the parent (epic) branch
that records the pre-rebase SHA as an ancestor, alongside the real
post-rebase SHA already merged in. This is a git-history patch, not a code
fix — it works around the guard's stale reference rather than correcting it,
and would need to be repeated for any future task that rebases mid-flight.

## Impact

- Worker retry cycles wasted: three `task action=close` attempts, two extra
  supervisor-relay round-trips, before the guard cleared.
- Requires manual, non-obvious git surgery (`ours` merge) on the parent
  branch by the supervisor to unblock a task whose code was never actually
  unmerged.
- Likely to recur for any task whose worker branch is rebased onto a moving
  parent/epic tip after an earlier partial merge — a routine supervisor
  instruction in multi-task-per-branch factory sessions.

## Second Reproduction (meta) — merge guard checks an unrelated branch/repo pair

A second, independent instance of the same guard-uses-wrong-reference family
surfaced while relaying **this very report**:

1. The relay task itself (creating this markdown file) lives entirely in a
   separate, unrelated repo (`cas-src`-style docs inbox) and has **zero**
   commits or file changes in the downstream project's own repo (`ozer`-style
   worker repo where `factory/W1` lives).
2. The relay task has no epic association, so its close-time parent-branch
   check apparently fell back to a repo-level default (`staging`) rather than
   recognizing the task has no code changes in that repo at all.
3. `task action=close` for this docs-only task was rejected: `⚠️ MERGE
   REQUIRED — factory/W1 has 9 commit(s) not on staging` — i.e. the guard
   compared the worker's *unrelated* Ozer-repo factory branch (still carrying
   commits from a prior, different epic task, correctly merged into that
   epic's own branch but not yet into `staging`) against `staging`, even
   though the task being closed produced no diff in that repo whatsoever.

### Expected (second repro)

A task with no committed changes in a given repo should not be subject to
that repo's branch-vs-parent merge guard at all — the guard should either be
scoped to repos/paths the task actually touched, or skipped entirely for
tasks with zero relevant commits.

### Actual (second repro)

The guard fired using the worker's currently-registered factory branch and a
hardcoded/default parent (`staging`) regardless of whether the task at hand
touched that repo, producing a "MERGE REQUIRED" rejection for a change that
was never meant to go through that branch/parent pair in the first place.

### Impact (second repro)

- Same class of false-negative close-blocking as the primary bug above, but
  triggered by task/repo scope mismatch rather than a stale rebased SHA.
- Left this task `awaiting_merge` rather than force-clearing it — the
  worker's Ozer factory branch commits are expected to reach `staging`
  naturally once that unrelated epic merges there, at which point ancestry
  will resolve without any history surgery. No workaround was applied here
  by design, to avoid another manual git patch for a check that shouldn't
  have applied to this task at all.

## Notes

- A direct `mcp__cs__system report_cas_bug` call was attempted first and
  failed with **HTTP 401** because GitHub auth was unavailable in that
  session; this markdown file is the manual fallback per the cross-team
  relay convention.
- Downstream project/task identifiers (project name, task IDs, branch names,
  epic name) have been anonymized/generalized above (`W1`, `E1`, `A`/`A'`,
  `M1`/`M2`) — the underlying git objects and exact identifiers are available
  from the reporting session if needed for a repro.
- This is a reporting-only request; no Cassy source was modified to produce
  this file.


## Resolved (cas-5485)

The false reject is the same *family* as cas-2938 (parked `factory_branch_anchor`
is a historical commit-ish). A normal rebase rewrites tip **A → A'**; ancestry
against A never clears after A' is integrated.

**Fix (close_ops `run_factory_branch_merge_gate`):** when a trusted AwaitingMerge
anchor is still ancestry-stranded, accept close via:

1. **Tip-tree reachability** of the parked anchor on parent/origin (clean rewrite
   with identical tip tree), or
2. **Auditable live-tip refresh** — `live_factory_tip_known_fully_merged` requires
   `known_unmerged_factory_commits` → **KnownZero** on the live `factory/<assignee>`
   tip (or vs `origin/<parent>`). Unknown refs/merge-base/rev-list never authorize.
   This is not a blanket zero-ahead bypass of the fail-open legacy counter.
3. **Cherry-equivalent patches of the parked anchor** (cas-5485 P2) —
   `commit_patches_cherry_equivalent_on_parent` (`git cherry` all `-`, at least
   one equivalent commit as positive evidence). Covers: park A → rebase A' →
   integrate A' → later unmerged B on live HEAD. Does not require live
   zero-ahead; evaluates the **task anchor**, so B cannot satisfy A. Fail-closed
   on missing refs / failed cherry / empty / any `+`.

Genuinely unmerged rebased work still Rejects. Serial-task protection (cas-4b3f)
retained for non-rebased anchors; after rebase, cherry-equivalence of A clears
task A while B remains unmerged on the factory branch.

The second repro in this report (docs-only task vs wrong repo/parent) is **out of
scope** for cas-5485 (no code changes for non-epic default parent / zero-diff skip).

Regression coverage (`merge_state_gate_tests`):

- `rebased_awaiting_merge_anchor_proceeds_after_post_rebase_tip_integrated`
- `stale_pre_rebase_anchor_alone_is_stranded_after_rebase_integrate` (precondition)
- `rebased_but_unmerged_work_still_rejects`
- `rebased_integrated_a_with_later_unmerged_b_still_closes_a` (P2)
- `later_unmerged_b_does_not_satisfy_unintegrated_anchor_a` (P2 safety)
- `cherry_equivalent_fails_closed_on_missing_ref`
