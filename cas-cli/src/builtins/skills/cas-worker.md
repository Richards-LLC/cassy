---
name: cas-worker
description: Use when acting as a factory worker on an assigned Cassy task, including progress reporting, blocker handling, delivery, and supervisor handoff.
metadata:
  managed_by: cas
---

# Factory Worker

Execute one assigned task in your checkout. SILENT EXECUTION: output results,
errors and the return contract only.

Cassy tools are named here without a prefix (`task`, `coordination`, `factory`, `memory`, `search`, `verification`). Call them with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode.

## Lifecycle

1. Run `coordination action=whoami`, then `task action=mine`. With no assignment,
   message the supervisor once that you are ready, then wait.
2. Choose one assigned task; `task action=show id=<task-id>`, then
   `task action=start id=<task-id>` before editing. Successful start is authoritative
   assignment acceptance; no prose ACK is required. Read its criteria, depth,
   execution note and project instructions. Reused checkout: `git rebase <target>`
   onto the supervisor's named target after checkpointing dirt.
   For an assigned QA-pass task, follow cas-qa-craft's Independent pass and
   `verification action=qa_record`; recording the verdict closes the QA task.
3. Implement its scope and commit logical units with the task ID, locally. Push
   once, right before the close that parks it for merge (and once per review
   round after that), unless `delivery_mode=local_merge`: every push to a
   factory branch starts a CI run. Where the repo ships the fast release rows
   (`scripts/release-gate.sh --fast-rows`), run them on the committed tip first:
   `./scripts/release-gate.sh --fast-rows --base origin/<target>`. The park
   refuses a tip without that PASS receipt. Add milestone `note_type=progress` notes.
4. Before close, invoke [`verify-before-claim`](../verify-before-claim/SKILL.md)
   and read [close-gate.md](references/close-gate.md). Close with
   `task action=close id=<task-id> reason="PASS <sha>: <evidence>"` when porcelain
   is empty and HEAD is the claimed commit. Hand verification-required guidance
   to the supervisor; quote it in `need:`.
5. For MERGE REQUIRED, drain `inbox_poll` of unread supervisor messages, capture
   the current factory-branch tip SHA, and request merge with `merge_request=true`;
   re-close after it lands; don't poll for it. Read [recovery.md](references/recovery.md)
   for the waiting rule and for rejection or crossed-message handling.

Finish or hand off this task before starting another. Stay available after
handoff; injected `Message from …` turns are instructions. Start only assignments
from the supervisor or `mine`. Never self-dispatch: every time you go idle,
use assigned work; `ready` and `available` are backlog
  visibility, not authorization.

- One task at a time. Scope is frozen. Honor non-goals and layer boundaries.
- Never block the pane: run long commands in the background with a log.
- Checkpoint, never compact: commit, push and note, then request a respawn when
  context runs low.
- When close says verification required, quote its guidance in `need:`.

## Browser checks

For projects with `scripts/journey-eval.sh`, run vitest, tsc and affected
journeys while iterating. `scripts/journey-eval.sh <task-artifact-dir>` resolves
the task target and runs every selected ID at four workers; `--affected <base>`
sets an explicit base. Retain the final code tip receipt. Workers and QA cannot
run `--full` or unfiltered Playwright; the supervisor owns one full run at epic
assembly and the merge queue runs it again. Reuse exact-tip receipts, and do
not rerun browsers after doc/ledger-only commits when evaluated inputs are
unchanged. For a failure control, rerun only that spec at one worker and retain
the original failure. A hand-picked subset cannot replace selected proof.

## Conditional references

- Detached work or checkpoint timing: [reminders.md](../cas-supervisor/references/reminders.md)
  for the push-first table and cleanup contract.

- Check/test work: [discipline.md](references/discipline.md) for capped commands,
  clean-commit receipts and the supervisor's full assembly proof.
- Evidence/report task, sync or resume: [details.md](references/details.md) for
  read-only sources, task state and credential handling.
- Bug filing: look up `issues.repo`, `issues.components.cassy`,
  `issues.components.violet` and `issues.components.cloud`; routing details are
  in [details.md](references/details.md#credentials-and-routing).
- Tool loading is two steps, not one: lookup does **not** execute the tool;
  call the resolved tool next, not another ToolSearch.

## Return contract

Send status, ready and close-failure messages as this block:

```text
status: <in_progress|ready|blocked|partial>
tip: <sha> on factory/<name>; worktree: <clean|dirty>
ci: <run id + result | not started>
proof: <tests run with pass count | artifact path>
deferred: <one line or none>
need: <what the supervisor must do, one line, or none>
```

For blockers add `blocker: <cause>`, a blocker task note, `status=blocked`,
and `blocker=true` on the message. Coordination uses the literal string `supervisor`
and both `summary` and `message`. Ordinary updates reach the inbox on the next turn;
only authenticated typed blocker, merge, verification or lifecycle events wake
an idle supervisor. Evidence stays in notes and the close reason.
