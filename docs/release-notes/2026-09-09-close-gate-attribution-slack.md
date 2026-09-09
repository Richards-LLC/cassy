# Slack draft — close gates judge only the task's own commits (main merge, PR #781)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production — User — Was: finishing a task could be blocked by changes another task had already shipped on the same branch. Now: Cassy judges a task by its own commits, so earlier work, a branch refresh, or a hotfix recorded after the fact no longer stand in the way.

Reply:
• Own work only — Was: a task limited to value edits or docs could be refused because an earlier, already-merged task on the same branch had added a file, or because the branch had been refreshed onto the trunk. Now: only the commits that belong to the task are judged.
• Complete receipts — Was: the diff receipt for a two-commit delivery could show only the second commit. Now: the receipt covers the whole delivery.
• Recording a hotfix — Was: a task created after its hotfix had already shipped could not be closed, even by a supervisor with a reason. Now: a registered supervisor can accept the merge commit with a logged reason.
• Clearing a constraint — Was: removing a task's edit constraint after approval forced a full re-verification even when nothing changed. Now: if the reviewed delivery is unchanged, the constraint clears without a new cycle.

## Dev thread

Top-level:
Live on production — Dev — Was: posture, no-code, receipt-stat, and epoch gates each diffed `target..branch` or scanned branch history their own way. Now: one shared first-parent delivery-range seam bounded by merge-base and work window feeds all four, with an audited supervisor exception (PR #781, closes #767).

Reply:
• Attribution seam — Was: `check_branch_violations`, the no-code scan, and `get_task_attributable_diff_stat` disagreed on what belonged to a task. Now: `task_attribution::task_delivery_ranges` selects owned or in-window unmerged first-parent commits, excludes other `cas-<hex>` ids, and expands a receipt through unnamed predecessors; legacy merged-anchor fallback retained.
• Supervisor exception — Was: `supervisor_override` was env-derived and never reached the epoch check. Now: it requires live registered supervisor authority, skips posture gates, and lets `validate_task_commit_receipt` accept a pre-lease merge commit with the reason logged; unmerged receipts stay rejected.
• Posture clear — Was: `execution_note=""` after approval hit the proof-scope lock. Now: allowed when the latest dispatch is resolved, approved, exact, and `evaluate_repository_proof` reports Unchanged.
• Foreign-id heuristic — Was: any `cas-*` word marked a commit foreign (`cas-cli` included). Now: `cas-` plus 4–8 hex chars only.

Proof: lib task lifecycle + mcp_tools_test 830/830; fixture tests for all four #767 cases plus crate-name attribution, authority, and reverse-state coverage.

## POSTED
Posted 2026-09-09 14:48Z via the MechaCassy hub to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788965305.173549 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788965305173549 (reply ts 1788965310.772229)
- Dev top-level ts 1788965312.267239 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788965312267239 (reply ts 1788965319.098619)
- GitHub #767 fix comment: https://github.com/Richards-LLC/cassy/issues/767#issuecomment-5603836740
