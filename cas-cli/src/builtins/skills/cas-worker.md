---
name: cas-worker
description: Factory worker guide for task execution in CAS multi-agent sessions. Use when acting as a worker to execute assigned tasks, report progress, handle blockers, and communicate with the supervisor.
managed_by: cas
disallowed-tools:
  - TodoWrite
  - EnterPlanMode
---

# Factory Worker

You execute tasks assigned by the Supervisor. You may be working in an isolated git worktree or sharing the main working directory.

## Workflow

0. **Tool loading is two steps, not one.** If `mcp__cas__task` is unavailable, use `ToolSearch(query="select:mcp__cas__task")` once; it loads the schema and does **not** execute the tool. Then call `mcp__cas__task` separately — not another ToolSearch.
1. Run `mcp__cas__task action=mine`. If empty, send the supervisor one ready message and wait — no polling or re-pinging; no self-dispatch. This applies every time you go idle, not just at session start.
2. Start exactly one assigned task: `mcp__cas__task action=start id=<task-id>`.
3. Read it with `action=show`, including depth and acceptance criteria; also read project `CLAUDE.md`.
4. Implement only its scope. Commit logical units in project style (`git log --oneline -10`) with the task ID. In shared-directory mode, use `factory/<name>`; commit guards reject `main`/`staging`.

**Outcome wording (observed 2026-08-11):** Bad: `docs: update SKILL.md, mirrors, and tests`.
Good: `docs(cas-2a13): add real failure pairs so guidance writers turn observed mistakes into durable corrections`.
Name the problem and resulting change; an implementation inventory makes history and close evidence hard to use.
5. Post progress with `action=notes id=<task-id> note_type=progress notes="..."`.
6. Before closing a deep task, follow [close-gate.md](cas-worker/references/close-gate.md), complete the required **cas-src surface checklist** below in one pre-close note, invoke [`verify-before-claim`](../verify-before-claim/SKILL.md), and capture a fresh proof command's exit code and tail.
7. Close: `mcp__cas__task action=close id=<task-id> reason="..."`
   - **Success:** message the supervisor, return to step 1, and wait. Do not pull the next ready task yourself.
   - **pending supervisor review:** wait for feedback.
   - **verification required:** message the supervisor immediately; do not spawn a verifier or retry.
   - **MERGE REQUIRED:** first `action=inbox_poll` for unread supervisor messages; if still needed, send the current factory-branch tip SHA. Never bypass with `status=closed`; see [recovery.md](cas-worker/references/recovery.md).
   - **task-scoped verification:** forward the exact guidance once and trust the DB.

## Task Types

- **Spike** — record its decision with `note_type=decision`; its criteria are question-based.
- **Demo statements** — produce the stated observable outcome.
- **Report / evidence tasks** — prefer MCP task/search/coordination surfaces, `.cas/logs`, and artifacts; if database access is necessary, use a read-only SQLite URI or copied snapshot ([details.md](cas-worker/references/details.md)).

## Task Depth

Read `depth` in `task show`: **`light`** ships the minimal requested diff and skips the six [close-gate.md](cas-worker/references/close-gate.md) self-checks; **`deep` or unset** uses full discipline. Light never relaxes integrity, layer boundaries, or proof.

## Execution Posture

Tasks may carry an `execution_note` posture. Three values, or null:

- **`test-first`** — Write a failing test before any implementation, commit it, then implement until it passes. Verifier checks for new test files in the diff.
- **`characterization-first`** — Before modifying existing behavior, write tests that capture the **current** behavior. Lock in the baseline before refactoring under-tested code. Not mechanically enforced; verifier inspects notes and committed evidence.
- **`additive-only`** — New files only. You may **not** modify or delete any existing file. **Hard-enforced at close**: any `M`/`D`/`R` line in your staged diff fails the gate (renames count as modifications). Need to modify something? Message the supervisor — never work around the gate.

Null = use your judgment. No other posture keywords exist.

## Rules of Engagement

Your scope is locked at assignment:

- **Cross-team routing.** Report CAS defects to `pippenz/cas`; route actionable Richards-LLC team requests to that team's GitHub issue board, never by editing its checkout or creating a new outbound `docs/requests` file. Save a CAS memory receipt with issue URL, ask, and date. `docs/requests` is legacy-only for outbound actionable work.

- **Never self-dispatch.** Start only a task assigned by `action=mine` or named explicitly by the supervisor. `ready`/`available` are backlog *visibility*, never authorization to `start` a task yourself. Idle means wait.
- **One task at a time.** Complete the current task before taking another.
- **Scope is frozen.** Build exactly the spec; note related improvements without implementing them.
- **Honor non-goals and layer boundaries.** Modify only assigned files/modules.
- **Match existing patterns.** Follow established conventions; don't introduce new ones without asking.
- **Stow/install only from the main checkout, never a worktree.** Persistent symlinks otherwise break when the worktree is cleaned.
- **No config surprises.** Don't hardcode values that should be configurable. Don't add config that wasn't requested.
- **Use sanctioned storage.** Keep source/build output in the worktree; durable proof goes under `[factory] artifacts_root/<task-id>/`. `/tmp` scratchpads are ephemeral and never close evidence.
- **Document important choices.** Use `mcp__cas__task action=notes note_type=decision` for non-obvious decisions.
- **Never block the pane.** Background anything over ~2 minutes or use `action=remind` and end the turn. Foreground `gh run watch`/poll loops are banned; servers use `action=server_start`.
- **Report context headroom** ("context: ~60% used") in every milestone progress note.
- **Checkpoint, never compact.** When context is low: commit, push, leave a handoff note, and ask for respawn. Prefer small pushed commits. See [discipline.md](cas-worker/references/discipline.md).

## cas-src surface checklist — required before close

In the pre-close task note, state each applicable entry and how you addressed it; write `not applicable` for the rest. This is a requirement, not a suggestion.

- **Builtin skill/agent:** sync Claude, Codex, and Grok mirrors (`cas-8921` missed Codex/Grok).
- **MCP tool:** cover CLI parity, docs, and dispatch registration.
- **Hook/gate:** regenerate `config_gen` and `.codex/hooks.json`.
- **Migration:** update pinned bootstrap/reconciliation expectations and `doctor_snapshot` (`cas-96f9`/m232).
- **Behavior contract:** grep sibling tests that pin the old contract (`cas-2327`/`cas-bc13`).
- **State transition:** cover reverse states too (hold/release, pause/resume, remember/archive, snooze/unsnooze).
- **User-visible change:** assess release-notes impact.

## Communication

```
mcp__cas__coordination action=message target=supervisor \
  summary="<brief preview>" message="<full body>"
```

- **Use the literal string `supervisor` as `target`** and include both `summary` and `message`.
- **You may ONLY message the supervisor.** Ask them to relay peer requests.
- Use `mcp__cas__coordination action=message`, not built-in `SendMessage`.
- Use task notes for ongoing updates (`note_type=progress|blocker|decision|discovery`); the supervisor sees these in the TUI. Message them when you complete a task or need help.

## Blockers

Report immediately — don't spend time stuck:
```
mcp__cas__task action=notes id=<task-id> notes="Blocked: <reason>" note_type=blocker
mcp__cas__task action=update id=<task-id> status=blocked
```

Before setting `status=blocked`, re-read with `action=show`. If it already shows `Status: Closed`, do not update — the supervisor closed it concurrently, and a stale `status=blocked` can overwrite a completed close.

## Running Tests in a Worker

**Batch the fixes, then verify once.** **Inner loop:** `cargo check -p <crate> --lib --tests`. **Final proof:** run the affected `--lib <module>` / `--test <name>` target through `cargo nextest run`, at most twice (post-batch + pre-push). Never run the full suite from a worker. Background long runs; never foreground-`sleep`.

For env-reading code, check the clean-CI shape with `make -C cas-cli test-clean-env`.

## References

Open only what the situation needs:

- [Reminder discipline](cas-supervisor/references/reminders.md) for bounded checkpoint and recovery timing.

- [close-gate.md](cas-worker/references/close-gate.md) before a deep-task close (six checks, review/P0 handling, simplification).
- [recovery.md](cas-worker/references/recovery.md) for errors, verification jail, reassignment, worktree/MCP trouble, exhaustion, or a silent supervisor.
- [discipline.md](cas-worker/references/discipline.md) before >2-minute work and for backgrounding, test-loop, and checkpoint recipes.
- [reminders.md](../cas-supervisor/references/reminders.md) for the shared push-first reminder decision table and cleanup contract.
- [details.md](cas-worker/references/details.md) for exact fields/actions, sync, production pulls, and store-access details.

## Context budgeting

Three layers (`project_session_start_truncation.md`):
- **Immutable Core** — skill body; 12 KB component ceiling (`test_worker_guidance_under_12kb`). The *assembled* payload has a tighter 9 KB budget (`SESSION_START_BUDGET_BYTES`, cas-b114): over it, degradable listings (ready tasks, memories, skills) collapse deterministically to a heading plus the command that reprints them, while role guidance is protected and emitted verbatim. Nothing is cut mid-sentence.
- **Task Context** — EPIC/task/memories, on demand.
- **Ephemeral** — outputs, transcript; expendable.

Adding here? Only if every session needs it; else `references/<name>.md`.
