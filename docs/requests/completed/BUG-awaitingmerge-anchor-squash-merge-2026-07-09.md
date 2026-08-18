# BUG: AwaitingMerge anchor permanently rejects squash-merged task

- **Date:** 2026-07-09
- **Severity:** High - blocks legitimate factory task close after a squash-equivalent merge and requires audited status-close recovery
- **Related:** cas-4b3f, cas-cf64

## Summary

The `AwaitingMerge` gate can permanently reject a task that has already been
squash-merged to the integration branch. The stored `factory_branch_anchor`
captures the worker task commit `A`, but the pull request is squash-merged as
integration commit `B`. Even after the factory ref is explicitly aligned to the
same `B` and has zero commits ahead, close still rejects because the gate checks
whether anchor `A` is an ancestor of the integration branch.

This makes the live ref convergence irrelevant: the task is effectively stuck in
`AwaitingMerge` even though no worker commits remain unmerged.

## Observed Failure

1. A factory task produces commit `A` on its factory branch.
2. The task's pull request is squash-merged to the integration branch as commit
   `B`.
3. The factory ref is explicitly updated/aligned to the same `B`.
4. `git` reports the factory ref has zero commits ahead of integration.
5. Worker close still rejects the task as `MERGE REQUIRED` / `AwaitingMerge`.
6. Supervisor close also rejects for the same reason.
7. Recovery requires an audited status-close override even though the task's
   branch has converged to the integration result.

The problematic check is not the live branch state. It is the stored
`factory_branch_anchor=A` being ancestry-checked against integration after the
PR has intentionally replaced `A` with squash commit `B`.

## Impact

This regresses the close-guard improvements from cas-4b3f and cas-cf64 for the
common case where task work is integrated through a squash merge. The guard
correctly protects serial tasks from being masked by later branch movement, but
it currently treats a squash-equivalent integration as permanently unmerged.

The result is a false negative in both worker and supervisor close paths:

- The task's work is present on integration as `B`.
- The factory branch has been safely converged to `B`.
- There are no remaining unmerged commits ahead of integration.
- The close gate still rejects because historical commit `A` is not an ancestor.

## Expected Behavior

The merge gate should clear when the task has been integrated through a
squash-equivalent commit or when the live factory ref has safely converged to the
same integration commit with no commits ahead.

The fix must preserve the original safety properties:

- Serial-task anchor protection from cas-4b3f remains intact.
- Later unmerged commits on a shared factory branch still cannot satisfy an
  earlier task's close.
- Genuinely unmerged task commits still reject.
- A squash-equivalent integration or safe live-ref convergence clears the gate.

## Regression Acceptance Criteria

- Reproduce a task commit `A` that enters `AwaitingMerge`.
- Squash-merge that work to integration as commit `B`.
- Align the factory ref to `B` so it has zero commits ahead of integration.
- Worker close succeeds without requiring a manual status-close override.
- Supervisor close follows the same result.
- A serial-task case with later unmerged commits on the factory branch still
  rejects, proving cas-4b3f's anchor protection is preserved.
- A genuinely unmerged task commit still rejects.

## Notes

This is a canonical reporting request only. It documents the regression so the
Cassy fix can be implemented separately. Do not treat the audited status-close
recovery as the desired behavior; it is only the current escape hatch for a
false rejection.


## Resolved (cas-2938)

Implemented in `run_factory_branch_merge_gate` (`cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs`).

When a task is `AwaitingMerge` with a trusted historical `factory_branch_anchor`
that still looks stranded by commit ancestry (the squash A↛B case), the gate
now accepts close if either:

1. **Tip-tree reachability** — the anchor tip's tree object appears on the
   parent branch (or `origin/<parent>`). Clean GitHub squash-merges rewrite the
   SHA but preserve the factory tip tree as the squash commit's tree. This also
   preserves cas-4b3f serial-task protection after squash (task A closes while
   task B's later unmerged commits ride on live factory HEAD).
2. **Live-ref convergence** — the live `factory/<assignee>` tip has zero
   commits not on the parent (or origin). Covers conflict-resolved squashes
   whose tip tree differs from A after the worker force-aligns to the
   integration tip. Cannot mask later unmerged serial work (live stranded > 0).

The historical anchor is **not** deleted or broadly bypassed; secondary signals
fire only after the primary ancestry check fails for a trusted parked anchor.

Regression coverage (`merge_state_gate_tests` in `close_ops.rs`):

- `squash_merged_awaiting_merge_with_live_ref_aligned_to_integration_proceeds`
- `squash_merged_content_equivalent_without_live_ref_align_proceeds`
- `squash_then_serial_second_task_does_not_restrand_first_close`
- `genuinely_unmerged_awaiting_merge_anchor_still_rejects`

Plus existing cas-4b3f / cas-cf64 / unmerged-reject tests remain green.

**Fix commit:** `298a95d` (code + report). Report source: supervisor checkout
`docs/requests/BUG-awaitingmerge-anchor-squash-merge-2026-07-09.md`.
