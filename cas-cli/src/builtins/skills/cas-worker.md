---
name: cas-worker
description: Use when acting as a factory worker on an assigned Cassy task, including progress reporting, blocker handling, delivery, and supervisor handoff.
metadata:
  managed_by: cas
---

# Factory Worker

Factory workers run the targeted tests for their change themselves. Every task
assignment authorizes and requires the fewest tests that fail without the change
([Fewest tests](../cas-tdd/SKILL.md#fewest-tests)) and running its affected
targeted tests. Workers close their own tasks; never ask the supervisor to close
your task on their behalf. `CONTRIBUTING` names the scoped test commands. Docs-only and no-code tasks prove their
acceptance check instead (e.g. markdownlint, `cas codemap status`, a published
artifact); don't add code tests to them.

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
   factory branch starts a CI run, and lane CI runs the project's lint and fast
   checks once. The park needs no local check receipt and the push hook runs no
   check suite. Add milestone `note_type=progress` notes.
4. Before close, invoke [`verify-before-claim`](../verify-before-claim/SKILL.md)
   and read [close-gate.md](references/close-gate.md). Close with
   `task action=close id=<task-id> reason="PASS <sha>: <evidence>"` when porcelain
   is empty and HEAD is the claimed commit. For verification-required closes,
   quote the required evidence in `need:` and close the task yourself after
   verification is complete.
5. For MERGE REQUIRED, drain `inbox_poll` of unread supervisor messages, capture
   the current factory-branch tip SHA, and send one `merge_request=true` message;
   its body is the request, so do not send a separate prose summary or ACK before
   or after it. Don't poll for it: after a green merge and an approved verdict
   Cassy closes the task and tells you once; re-close only when the merge
   receipt says a gate still holds. Read [recovery.md](references/recovery.md)
   for the waiting rule and for rejection or crossed-message handling.

One event gets one message. A successful `task action=start` is its own
acceptance receipt. A close is the lifecycle report; never send a post-close
summary or echo the close relay. After a merge request, wait for the supervisor's
response instead of sending a second update about the same request.

Finish or hand off this task before starting another. Stay available after
handoff; injected `Message from …` turns are instructions. Start only assignments
from the supervisor or `mine`. Never self-dispatch: every time you go idle,
use assigned work; `ready` and `available` are backlog
  visibility, not authorization.

- One task at a time. Scope is frozen. Honor non-goals and layer boundaries.
- Worker peer lane: `coordination action=message target=<worker-name> summary="collision warning" message="Task cas-123 touches the same files."`
  reaches only a registered worker in the same factory session. Cassy also sends a copy
  to that session's supervisor. Use this narrow lane for collision
  warnings, not general chat or cross-session contact; send task status, blockers
  and handoffs to `target=supervisor`.
- A worker can send `coordination action=message target=supervisor kind=status summary="status" message="Please review the current task update."`
  or `kind=ready` with `summary` and `message` for a typed action wake when the
  supervisor must review an update or continue coordination. Ordinary messages
  remain inbox-only; never set `merge_request=true` to wake the supervisor for a
  status update.
- Never block the pane: run long commands in the background with a log.
- Stop only the PID you started (capture `$!` or use your own PID file); never use `pkill`, `killall`, or process-name searches to send signals on the shared host.
- Checkpoint, never compact: commit, push and note, then request a respawn when
  context runs low.
- When close says verification required, quote its guidance in `need:`.

## Browser checks

For projects with `scripts/journey-eval.sh`, run vitest, tsc and affected
journeys while iterating. `scripts/journey-eval.sh <task-artifact-dir>` resolves
the task target and runs every selected ID at four workers; `--affected <base>`
sets an explicit base. Close does not require a journey receipt; the merge
queue runs the full journey suite once. Workers and QA cannot run `--full` or
unfiltered Playwright. Do not rerun browsers after doc/ledger-only commits when
evaluated inputs are unchanged. For a failure control, rerun only that spec at
one worker and keep the original failure. A hand-picked subset cannot replace
the selected journeys.

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
only authenticated typed blocker, merge, status/ready action, verification or
lifecycle events wake an idle supervisor. Free-form worker messages remain
inbox-only. Evidence stays in notes and the close reason.
