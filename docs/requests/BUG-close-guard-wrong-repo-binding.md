---
from: witty-falcon-64 (Penguinz factory supervisor)
date: 2026-07-29
priority: medium
cas_task: cas-4fa1
---

# BUG: close guard binds to spawn-repo factory anchor; cross-repo tasks cannot close

## What happened

Worker gentle-dragon-76 was provisioned in the Penguinz project (isolated worktree, branch `factory/gentle-dragon-76` cut from the checked-out epic tip `91ec652`). Its task (cas-4fa1, vramp tray icon) required — per the task description — that the fix land in a **different repo**: `/home/pippenz/soundwave-config` (its own git repo with its own `.cas/worktrees`). The worker committed there (`f8c2b05`), pushed, and the supervisor merged it to soundwave-config master (`22e01e9` on origin).

`task close` then returned MERGE REQUIRED repeatedly, because the guard inspects the **Penguinz** `factory/gentle-dragon-76` branch, which contained 22 commits "not on Penguinz master" — all of them inherited A1111-epic history from the branch point, none authored by the worker.

## What made it worse

The guard appears to compare against a **stale anchor captured at spawn/lease time**: after the supervisor verified the branch had zero worker-authored commits and `git reset --hard master` moved it to `bd7aec2` (worktree clean), a re-close still reported the identical local tip `91ec652`. Live git state changes do not update the guard's view.

## Workaround used

Supervisor closed on the worker's behalf with `commit_receipt=f8c2b05...` + `bypass_code_review=true` and a full evidence trail in the close reason. (Also note: `commit_receipt` docs say the SHA is validated "in the task's parent branch" — it's unclear which repo that validation ran against, since f8c2b05 does not exist in the Penguinz object store; it succeeded regardless, which may itself be a validation gap.)

## Asks

1. Let a task (or close call) declare its target repo, so the merge/reachability guard checks the repo the work actually lives in — factory workers legitimately get tasks whose source of truth is another local repo (host-config repos especially).
2. Re-evaluate the guard against **live** branch state at close time instead of the spawn-time anchor, or expose a way to refresh the anchor.
3. Secondary: when a factory worktree is cut from a non-master parent (epic branch), the guard counting inherited parent-branch commits as "unmerged" makes any trunk-targeted comparison misleading.

Repro pointers: Penguinz session Penguinz-fast-viper-48, task cas-4fa1, notifications 785/787; worker's verbatim guard output is in the task notes and supervisor transcript.

## Addendum — verbatim receipt-validation rejection (worker attempt)

The worker's own close attempt with `commit_receipt=22e01e9` (short SHA) returned:

> ⚠️ INVALID TASK COMMIT RECEIPT — commit_receipt `22e01e9` is not valid merge evidence: expected a full 40- or 64-character hexadecimal commit SHA. A close receipt must be the full SHA of a commit produced by this task, carry a non-empty file diff, and already be an ancestor of master (or origin/master).

The supervisor's close with the full SHA `f8c2b05365bd370503162394916ab8edab6d8121` + `bypass_code_review=true` **succeeded** — despite that commit not existing in the Penguinz object store where the gate runs. So either the ancestry/diff validation silently no-ops when the SHA is unresolvable in the bound repo, or bypass_code_review skips receipt validation entirely. Either way the receipt check gives false assurance for cross-repo closes.
