---
metadata:
  managed_by: cas
---

# Close Gate — Delivery

Read at delivery. Review owns coding and test-quality judgments; this branch
collects evidence and delivers the assigned task.

## Clean-tree receipt

Run `git status --porcelain` and `git rev-parse HEAD`. Empty porcelain and HEAD
equal to the delivered SHA establish what is on disk. A rejected write may
still have changed disk: inspect any divergence, commit your authorized work,
and record its disposition before close. Escalate unexplained writes instead
of closing over them. This receipt applies at every depth.

For `depth=light`, deliver the minimal working outcome with honest evidence.
For deep or unset depth, map every criterion to executed proof or a named proof
owner using `verify-before-claim`; trace affected consumers and registrations.
Read [discipline.md](discipline.md) before check/test work. Rust assembly belongs
to the supervisor; name unexecuted obligations. Run affected non-Rust checks and
record their exit status and actual test count. A zero-test success is no proof.

## Merge and freshness

Close requires the delivered work on the named target. On MERGE REQUIRED:

1. Drain `coordination action=inbox_poll` to `No unread messages`; act on
   unread supervisor messages and re-read the task before corrective edits.
2. Capture the current tip: `git rev-parse factory/<name>` (use your per-task
   branch if active). Check `git merge-base --is-ancestor <delivered-tip> <target-tip>`.
   If already merged or closed, re-close or stop rather than amend stale state.
3. For `delivery_mode=local_merge`, retain the local commit and request supervisor
   merge with its branch and SHA. Push only with explicit supervisor authorization.
   Otherwise push the delivered branch, then send `merge_request=true` with the
   target and SHA; re-close after confirmed merge.
4. For a protected target, follow its PR flow. Ask for an epic merge rather than
   `gh pr create --base epic/...`; the supervisor owns the merge venue.

If another assigned task follows a handed-off delivery, start its per-task branch
from the epic tip (`factory/<name>-<task-id>`), preserving the parked branch.
No simultaneous task ownership. For rejection details, read [recovery.md](recovery.md).
Never set `status=closed` or invent a verification record to bypass close.

## Delivery receipts

- `commit_receipt=<sha>` names this task's immutable, non-empty delivered commit.
  Close validates ancestry against the target; use it for squash/cherry-pick
  identity drift. Forward a rejected receipt with the exact guard error.
- `completion_receipt=<json>` requests transactional close after the current
  source tip is merged. Fields: `task_id`, `worker_agent_id`, `repo_selector`,
  `source_branch`, `commit_sha`, `merge_base_sha`, `target_branch`, `target_sha`,
  `proof_reference`, `scope_summary`, optional `artifact_path`. The artifact must
  exist under the configured task artifacts directory. Identity and Git are
  revalidated; verification remains supervisor-owned.

## Conditional evidence

- Demo, configured user-facing path or catalog journey: use `cas-qa-craft`
  before parking; cite its commit-bound bundle or real-build ledger. Terminal
  paths also need `cas-cli-craft`'s `terminal-qa: PASS` receipt. Docs/test/CI-only
  deliveries do not trigger visual QA.
- Risk: read task `risk` and `proof_targets`. Name the supervisor's
  `ASSEMBLY_PROOF` for full Rust execution. Non-Rust platform risk needs a
  `platform_proof` note with the actual platform, command and passing result.
- Spike: record the decision and evidence; `search_manifest` can record commands
  and hit counts for conclusions based on searches.
- Epic: every child's delivered work must reach the parent before epic close.

## cas-src surface checklist

Record applicable evidence and the reason for each inapplicable surface:

| Changed surface | Delivery evidence |
| --- | --- |
| Builtin skill/agent | One canonical body registered in Claude, Codex and Grok catalogs; regenerate reference history. |
| MCP/CLI | Caller/dispatch registration, CLI parity and docs. |
| Hook/gate | Relevant `config_gen` and `.codex/hooks.json` projection. |
| Migration | Bootstrap/reconciliation expectations and `doctor_snapshot`. |
| Behavior/state | Affected contracts/consumers, including reverse transitions. |
| Public web/terminal | Commit-bound QA receipts for the configured journey. |
| User-visible change | Release-note impact and the project's publication rubric. |

Close with `PASS <sha>` or `ISSUES <sha>` plus known non-blocking defects,
criterion evidence, safety facts and deferred proof ownership. Read actual tool
rejections as state; forward verification-required guidance once to the supervisor.
