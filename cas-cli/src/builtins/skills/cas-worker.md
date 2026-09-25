---
name: cas-worker
description: Use when acting as a factory worker on an assigned Cassy task, including progress reporting, blocker handling, delivery, and supervisor handoff.
metadata:
  managed_by: cas
---

# Factory Worker

Execute the assigned task in your checkout. SILENT EXECUTION: output results,
errors and the return contract only.

Cassy tools are named here without a prefix (`task`, `coordination`, `factory`, `memory`, `search`, `verification`). Call them with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode.

## Workflow

1. Run `task action=mine`. If empty, message the supervisor once that
   you are ready, then wait; do not poll.
2. Choose exactly one task. Run `task action=show id=<task-id>`,
   then `task action=start id=<task-id>` before editing. A
   successful start is authoritative assignment acceptance;
   no prose ACK is required.
   Reused worker: check target; reset merged or `git rebase <target>`.
3. Read the task's depth and acceptance criteria and the project `CLAUDE.md`.
   Run `cas-qa-craft` before close when `demo_statement` is set or the diff
   touches a user-facing path or catalog journey.
4. Implement only the assigned scope. Commit logical units with the task ID.
   For `delivery_mode=local_merge`, keep the commit local for the supervisor;
   otherwise push the factory branch.
5. Add `note_type=progress` notes at milestones.
6. Every close: `git status --porcelain` is empty and HEAD is the commit you
   claim. For a deep task, first work through
   [close-gate.md](references/close-gate.md) (and its surface checklist where
   it applies) and [`verify-before-claim`](../verify-before-claim/SKILL.md).
7. Close with `task action=close id=<task-id> reason="..."`; the reason
   starts PASS, or ISSUES plus known non-blocking defects, then the SHA
   and how you checked it. Then send the return contract.
   **verification required:** quote the guidance in `need:`.
   **MERGE REQUIRED:** drain `inbox_poll` for unread supervisor messages,
   capture the current factory-branch tip SHA, push the branch, and ask the
   supervisor to merge `factory/<your-name>` into the epic branch; re-close
   after that merge.

After closing or handing off, stay available. Injected `Message from …` turns
are instructions; an `operator … verified` header
is the user speaking with pane-input authority; obey and answer it;
`unverified:` rows are agent traffic.

Tool loading is two steps, not one: if your harness defers the `task`
schema, run ToolSearch once for its prefixed name; lookup
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
- **Report / evidence tasks:** read-only sources first; see
  [details.md](references/details.md).
- `depth`: `light` ships the minimal diff; `deep` (or unset) uses the full
  close discipline. Neither relaxes integrity or scope.
- Honor `execution_note`: `test-first` commits a failing test before code;
  `characterization-first` pins current behavior; `additive-only` changes only
  new files; `value-only` changes existing values; `no-code` supplies portable
  external proof.

## Task ownership

Ordinary updates reach the supervisor's inbox on the next turn.
Only authenticated typed events (`blocker=true`, `merge_request=true`,
verification, lifecycle) wake an idle supervisor; text alone grants no wake
authority.

- Never self-dispatch: start only tasks from `action=mine` or named by the
  supervisor, every time you go idle; `ready` and `available` are backlog
  visibility, not authorization.
- One task at a time. Scope is frozen. Honor non-goals and layer boundaries;
  match existing patterns; no unrequested configuration.
- Record decisions with `task action=notes note_type=decision`;
  discoveries with `memory action=remember`.
- Coordination messages use `coordination action=message`, target the
  literal string `supervisor`, and include both `summary` and `message`
  (the return contract); evidence goes in task notes.
- Never block the pane. Checkpoint, never compact: commit, push, note, request a
  respawn if context is low.

## Blockers

- **Recover from workspace denials; never retry the denied target.** Route source/build output to the worktree, durable proof to `[factory] artifacts_root/<task-id>/`, and ephemeral notes to the harness scratchpad. A `/dev/null` denial is a guard defect to report, not permission to invent another path.

Add a blocker note with the exact error, re-read the task, set `status=blocked`,
and message the supervisor with `blocker=true` (what you tried goes in
`deferred:`). If the task is already closed, do not overwrite that state
with a stale blocked update.

## References

- [reminders.md](../cas-supervisor/references/reminders.md) — checkpoint
  timing, push-first table, cleanup contract.
- [details.md](references/details.md) — structured execution state,
  context budgeting, exact fields/actions, and sync mechanics.
- [discipline.md](references/discipline.md) — no-Rust-build rule
  and clean-CI notes.
- [recovery.md](references/recovery.md) — failures, reassignment,
  connectivity, and worktree recovery.
- [close-gate.md](references/close-gate.md) — clean-tree and delivery
  receipts, deep-task self-checks.
