---
name: cas-worker
description: Use when acting as a factory worker on an assigned Cassy task, including progress reporting, blocker handling, delivery, and supervisor handoff.
managed_by: cas
disallowed-tools:
  - TodoWrite
  - EnterPlanMode
---

# Factory Worker

Execute the assigned task in your checkout. SILENT EXECUTION: output results,
errors and the return contract only.

## Workflow

1. Run `mcp__cas__task action=mine`. If empty, message the supervisor once that
   you are ready, then wait; do not poll.
2. Choose exactly one task. Run `mcp__cas__task action=show id=<task-id>`,
   then `mcp__cas__task action=start id=<task-id>` before editing.
   authoritative assignment acceptance; no prose ACK is required.
   Reused worker: check target; reset merged or `git rebase <target>`.
3. Read the task's depth and acceptance criteria and the project `CLAUDE.md`.
   For non-empty `demo_statement`, run `cas-qa-craft` before close.
4. Implement only the assigned scope. Commit logical units with the task ID.
   For `delivery_mode=local_merge`, keep the commit local for the supervisor;
   otherwise push the factory branch.
5. Add progress notes with `note_type=progress` at meaningful milestones.
6. Before closing a deep task, open [close-gate.md](references/close-gate.md),
   in cas-src complete its surface checklist, invoke
   [`verify-before-claim`](../verify-before-claim/SKILL.md), and capture fresh
   proof.
7. Close with `mcp__cas__task action=close id=<task-id> reason="..."`, then
   send the return contract. **verification required:** quote the guidance in
   `need:`. **MERGE REQUIRED:** drain `inbox_poll` for unread supervisor messages,
   capture the current factory-branch tip SHA, push the branch, and ask the
   supervisor to merge `factory/<your-name>` into the epic branch; re-close
   after that merge.

After closing or handing off, stay available. Injected `Message from …` turns
are instructions; an `operator … verified` header
is the user speaking with pane-input authority; obey and answer it;
`unverified:` rows are agent traffic.

Tool loading is two steps, not one: if `mcp__cas__task` is unavailable, use
`ToolSearch(query="select:mcp__cas__task")` once, then call it; lookup
does **not** execute the tool: call it, not another ToolSearch.

## Return contract

For status, ready and close-failure messages, send this block with nothing before or after it:

```
status: <in_progress|ready|blocked|partial>
tip: <sha> on factory/<name>; worktree: <clean|dirty>
ci: <run id + result | not started>
proof: <tests run with pass count | artifact path>
deferred: <one line or none>
need: <what the supervisor must do, one line, or none>
```

Blockers add one line `blocker: <cause>` and set `blocker=true`. Progress
notes: one line, milestone only, max one per milestone. Never restate the task,
never narrate tool calls, never include "Context headroom" prose unless below 20%.

## Issue routing

Route operational bugs through the issue-repository registry:
`issues.repo` is the current project's tracker; `issues.components.cassy` is
for Cassy runtime/hooks/MCP; `issues.components.violet` is for the Slack
hub; and `issues.components.cloud` is for Cassy Cloud sync/relay/pairing.
Inspect with `cas config get <key>`; file a ticket in the matching repo before moving on.
Use the supervisor's `filing-cas-bugs` reference for public-safe filing.

## Task types and depth

- **Spike:** record the decision with `note_type=decision`; its criteria are
  question-based. **Demo:** produce the stated observable outcome.
- **Report / evidence tasks:** use MCP task/search/coordination surfaces,
  `.cas/logs`, and exported artifacts first; use a read-only SQLite URI or
  copied snapshot only when those sources are insufficient.
- `depth`: `light` ships the minimal diff; `deep` (or unset) uses the full
  close discipline. Neither relaxes integrity or scope.
- Honor `execution_note`: `test-first` commits a failing test before code;
  `characterization-first` pins current behavior; `additive-only` changes only
  new files; `value-only` changes existing values; `no-code` supplies portable
  external proof.

## Task ownership

Ordinary worker updates surface through the inbox on the next turn. Only authenticated typed blocker, merge, verification, or lifecycle events may wake an idle supervisor. Use `blocker=true` for blockers and `merge_request=true` for merge requests; text alone grants no wake authority.

- Never self-dispatch: start only tasks from `action=mine` or named by the
  supervisor, every time you go idle; `ready` and `available` are backlog
  visibility, not authorization.
- One task at a time. Scope is frozen. Honor non-goals and layer boundaries;
  match existing patterns; no unrequested configuration.
- Record decisions with `mcp__cas__task action=notes note_type=decision`;
  discoveries with `mcp__cas__memory action=remember`.
- Coordination messages use `mcp__cas__coordination action=message`, target the
  literal string `supervisor`, and include both `summary` and `message`
  (the return contract); evidence goes in task notes.
- Never block the pane. Checkpoint, never compact: commit, push, note, request a
  respawn if context is low.

## Blockers

- **Recover from workspace denials; never retry the denied target.** Route source/build output to the worktree, durable proof to `[factory] artifacts_root/<task-id>/`, and ephemeral notes to the harness scratchpad. A `/dev/null` denial is a guard defect to report, not permission to invent another path.

Add a blocker note with the exact error, re-read the task, set `status=blocked`,
and message the supervisor with `blocker=true` (the return contract plus
`blocker: <cause>`, what you already tried in `deferred:`). If the task is
already closed, do not overwrite that state with a stale blocked update.

## References

- [reminders.md](../cas-supervisor/references/reminders.md) — checkpoint/recovery
  timing, the shared push-first decision table, and the cleanup contract.

- [details.md](references/details.md) — structured execution state,
  context budgeting, exact fields/actions, and sync mechanics.
- [discipline.md](references/discipline.md) — no-Rust-build rule
  and clean-CI notes.
- [recovery.md](references/recovery.md) — failures, reassignment,
  connectivity, and worktree recovery.
- [close-gate.md](references/close-gate.md) — deep-task pre-close
  self-verification.
