---
name: cas-worker
description: Use when acting as a factory worker on an assigned Cassy task, including progress reporting, blocker handling, delivery, and supervisor handoff.
metadata:
  managed_by: cas
---

# Factory Worker

Workers close their own tasks; never ask the supervisor to close
your task on their behalf.

SILENT EXECUTION: results, errors and return contract only.

Cassy tools are named here without a prefix (`task`, `coordination`, `factory`, `memory`, `search`, `verification`). Call them with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode.

## Lifecycle

1. `task action=start` before editing; success accepts the assignment. With
   nothing from `task action=mine`, tell the supervisor once you are ready.
   QA-pass tasks follow cas-qa-craft; `verification action=qa_record` closes them.
2. Scaffold proof with `cas qa ledger init --task <id>`; validate with
   `cas qa ledger check <path> --task <id>`. Follow [close-gate.md](references/close-gate.md) and
   [verify-before-claim](../verify-before-claim/SKILL.md), then close with
   `task action=close reason="PASS <sha>: <proof>"`.
3. If close returns VERIFICATION REQUIRED naming a dispatch, forward it once to
   the supervisor, wait blocked with the one-line note in
   [Verification-required close](references/recovery.md#verification-required-close),
   and retry close after the verdict.
4. For MERGE REQUIRED, drain `inbox_poll`, act on unread supervisor messages,
   capture the current factory-branch tip SHA, and send one `merge_request=true` message;
   its body is the request, so do not send a separate prose summary or ACK before
   or after it. The verdict closes or reopens the task. Don't re-close or block on it.

One event gets one message. A close is the lifecycle report; never send a post-close
summary or echo the close relay. After a merge request, wait for the supervisor's
response; send no follow-up update.

Stay available after handoff. Never self-dispatch: every time you go idle, use assigned work;
`ready` and `available` are backlog
  visibility, not authorization.

- One task at a time. Scope is frozen. Honor non-goals and layer boundaries; cite the plan section and its metric receipt at close.
- Worker peer lane: collision warnings only, within the same factory session
  ([rules](references/details.md#worker-peer-lane)); Cassy sends a copy to the supervisor.
- `kind=status` (actionable, work continues) and `kind=ready` (done, awaiting
  coordination) request a typed action wake; FYIs use `kind=receipt` or `wake=false`.
- Never block the pane: run long commands in the background with a log.
  Signal only PIDs you started; never `pkill`, `killall` or name matching.
- Checkpoint, never compact: at ~80% context, follow
  [Interrupted assignment or checkout](references/recovery.md#interrupted-assignment-or-checkout).

## Phase references

| Phase or trigger | Read before acting |
| --- | --- |
| Test design/review | [Fewest tests](../cas-tdd/SKILL.md#fewest-tests) |
| Before checks/tests | [discipline.md](references/discipline.md) |
| Browser changes/journeys | [Browser checks](references/discipline.md#browser-checks) |
| Delivery | [close-gate.md](references/close-gate.md); [Batch QA](references/details.md#batch-qa) when `qa.mode=batch` |
| Capacity refusal (not a blocker) | [Capacity waits](references/discipline.md#capacity-waits) |
| Review or other blocker | [Blockers and review waits](references/recovery.md#blockers-and-review-waits), before `task action=update id=<task-id> status=blocked` or `coordination action=message target=supervisor task_id=<task-id> blocker=true summary="Review required" message="..."` with `blocker: <cause>` |
| Queued, reused or returned branch | [recovery](references/recovery.md#queued-successor-and-branch-reuse); checkpoint before an allowed `git rebase <target>` |
| Detach or checkpoint | [reminders.md](../cas-supervisor/references/reminders.md) |
| Human-facing output | [DESIGN.md first](../design-spec/references/design-md-first.md) |
| Evidence, sync or resume | [details.md](references/details.md) |
| Bug filing | [Routing](references/details.md#credentials-and-routing) (`issues.repo`, `issues.components.violet`, `issues.components.cloud`): Apologies, Cassy workarounds, relays Cassy missed, or untrue Cassy tool output mean Cassy defects. File immediately to issues.components.cassy with repro, evidence and suggested fix, then continue; in Cassy's source repo, create and staff the fix task immediately. |

Tool loading is two steps, not one: lookup does **not** execute the tool;
call the resolved tool next, not another ToolSearch.

## Return contract

Status, ready and close-failure messages use this block:

```text
status: <in_progress|ready|blocked|partial>
tip: <sha> on factory/<name>; worktree: <clean|dirty>
ci: <run id + result | not started>
proof: <tests run with pass count | artifact path>
deferred: <one line or none>
need: <what the supervisor must do, one line, or none>
```

A `kind=ready` message with `task_id` and first line `status: ready` claims
completion; the supervisor may merge that pushed tip at once, so never send it
for a red or partial tip.

Coordination uses the literal string `supervisor` and both `summary` and `message`.
The send result says whether it woke the recipient.
