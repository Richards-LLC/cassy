---
name: cas-worker
description: Use when acting as a factory worker on an assigned Cassy task, including progress reporting, blocker handling, delivery, and supervisor handoff.
metadata:
  managed_by: cas
---

# Factory Worker

Tests assert behavior or stable machine identifiers such as error codes and rule IDs. Never require human-readable wording, word lists, canonical sentences or phrase presence; changing prose alone must not fail a test. Exact caller payloads and machine protocol bytes remain data-integrity checks. Fixed safety text needs a named consumer that requires those bytes; a writing preference is insufficient.

Cassy tools are named here without a prefix (`task`, `coordination`, `factory`, `memory`, `search`, `verification`). Call them with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode.

The launch contract carries start, one task at a time, push, close and the 80% checkpoint; this skill adds what it does not.

## Lifecycle

1. `task action=start` before editing; success accepts the assignment.
   QA-pass tasks follow cas-qa-craft; `verification action=qa_record` closes them.
2. Only when task start's close checks list a Ledger or `Adversarial probe` line,
   scaffold proof with `cas qa ledger init --task <id>` and validate with
   `cas qa ledger check <path> --task <id>`. Follow [close-gate.md](references/close-gate.md) and
   [verify-before-claim](../verify-before-claim/SKILL.md), then close with
   `task action=close reason="PASS <sha>: <proof>"`.
3. A park reads `PARKED … awaiting merge verdict; nothing more to do`; the
   verdict wakes you once. VERIFICATION REQUIRED and MERGE REQUIRED refusals
   follow [recovery.md](references/recovery.md).

One event gets one message. A close is the lifecycle report, so send no
post-close summary; after a merge request, wait for the supervisor's response.

- Never self-dispatch: `ready` and `available` are backlog visibility, not authorization.
- One task at a time. Scope is frozen. Honor non-goals and layer boundaries; cite the plan section and its metric receipt at close when the brief links one.
- Worker peer lane: local collision warnings within the same factory session; remote same-epic contracts
  ([rules](references/details.md#worker-peer-lane)); Cassy sends a copy to the supervisor.
- The supervisor writes release notes; workers never draft them.
- `kind=status` requests an action wake; FYIs use `kind=receipt` or `wake=false`.
- Never block the pane: run long commands in the background with a log.
  Signal only PIDs you started; never `pkill`, `killall` or name matching.
- Checkpoint: `kind=respawn` queues your recycle with the same recipe and task bindings
  ([Interrupted assignment or checkout](references/recovery.md#interrupted-assignment-or-checkout)).

## Phase references

| Phase or trigger | Read before acting |
| --- | --- |
| Test design/review | [Fewest tests](../cas-writing-tests/SKILL.md#fewest-tests) |
| Before checks/tests | [discipline.md](references/discipline.md) |
| Browser changes/journeys | [Browser checks](references/discipline.md#browser-checks) |
| Delivery | [close-gate.md](references/close-gate.md); [Batch QA](references/details.md#batch-qa) when `qa.mode=batch` |
| Capacity refusal (not a blocker) | [Capacity waits](references/discipline.md#capacity-waits) |
| Review or other blocker | [Blockers and review waits](references/recovery.md#blockers-and-review-waits), before blocking the task or notifying the supervisor |
| Queued, reused or returned branch | [recovery](references/recovery.md#queued-successor-and-branch-reuse); checkpoint before an allowed `git rebase <target>`; a checkpoint push alone is not a park |
| Detach or checkpoint | [reminders.md](../cas-supervisor/references/reminders.md) |
| Human-facing output | [DESIGN.md first](../design-spec/references/design-md-first.md) |
| Evidence, sync or resume | [details.md](references/details.md) |
| Bug filing | [Routing](references/details.md#credentials-and-routing): Cassy defects go to `issues.components.cassy`, others to `issues.repo`, `issues.components.violet` or `issues.components.cloud` |

## Return contract

Status and blocker messages use this block; fix close refusals silently:

```text
status: <in_progress|blocked|partial>
tip: <sha> on factory/<name>; worktree: <clean|dirty>
pushed: <sha | not pushed>
proof: <tests run with pass count | artifact path>
blocker: <cause or none>
need: <what the supervisor must do, one line, or none>
```

Coordination uses the literal string `supervisor` and both `summary` and `message`.
The send result says whether it woke the recipient.
