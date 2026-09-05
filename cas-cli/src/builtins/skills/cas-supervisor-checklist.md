---
name: cas-supervisor-checklist
description: Use at the start of a factory-supervisor session to load context, inspect EPICs, and confirm worker availability.
managed_by: cas
---

# Supervisor Checklist

## Session Start

0. **Binary freshness check (cas-d0f9).** Before anything else — confirm the running `cas serve` binary matches HEAD of this repo. A stale binary may impose legacy global verification blocks instead of the current exact-task close gate. See [preflight.md](cas-supervisor/references/preflight.md) for the full command; the 10-second version:

   ```
   # cas --version format: cas 2.27.0 (9b52e17-dirty 2026-07-16)
   # Fields after '(': short hash, optional -dirty (build tree had local mods), then build date.
   # Do NOT use awk '{print $NF}' — that grabs the date token, not the hash.
   cas --version | sed -E 's/.*\(([0-9a-f]+)(-dirty)? .*/\1/'   # → 9b52e17
   git rev-parse --short HEAD                                   # hash of the repo right now
   ```

   If they don't match AND `git log --oneline HEAD --not <running-hash> -- cas-cli/src/mcp cas-cli/src/hooks cas-cli/src/cli/factory` returns anything, the binary must be rebuilt — but **do not kill or restart `cas serve` from this active MCP session**. That stdio process is this session's Cassy-tool connection, so restarting it here disconnects the very tools needed to finish setup.

   Stop at step 0 and ask the operator to run `cargo build --release` and use the harness's MCP reconnect/restart control (or open a fresh supervisor session) to launch the new `cas serve`. Do not use `pkill` or any name-based process kill. Resume only after the Cassy tool list is restored, then rerun this checklist from step 0.

1. Identify yourself: `mcp__cas__coordination action=whoami`
2. Load EPIC/task context:
   ```
   mcp__cas__task action=list task_type=epic
   mcp__cas__task action=ready
   mcp__cas__task action=list status=blocked
   ```
3. Pull relevant memories and rules:
   ```
   mcp__cas__search action=search query="<keywords>" doc_type=entry limit=5
   ```
4. Check codemap freshness:
   - If `.claude/CODEMAP.md` is missing → run `/codemap` to generate it.
   - If it exists but is stale (structural changes since last update) → run `/codemap` to refresh.
   - Workers reference CODEMAP for codebase orientation — ensure it's current before spawning them.
5. Check worker availability: `mcp__cas__coordination action=worker_status`
6. **Session hygiene triage** — the SessionStart hook prepends a "⚠ Prior-factory
   WIP detected" banner to the supervisor context when the main worktree has
   uncommitted changes, with per-file attribution (last `cas-xxxx` commit)
   where git history permits. If you see that banner, decide salvage / commit /
   discard **before** spawning workers — otherwise a cherry-pick into `develop`
   will abort later.

   For a full on-demand report (including stale agents and orphan worktrees):
   ```
   mcp__cas__coordination action=gc_report
   ```
   The report's "Prior-factory WIP candidates" section mirrors the banner and
   is safe to re-run at any time; it never auto-deletes.

   For the full history of what prior sessions left behind, see
   `.cas/logs/factory-session-{YYYY-MM-DD}.log` (written automatically on
   `SessionEnd`; each block records session id, agent, worktree, and a
   `git status --porcelain` snapshot).

## Intake Gate (Before Planning)

- [ ] "What does done look like?" has a measurable answer
- [ ] No vague terms — "better/faster/cleaner" replaced with testable criteria
- [ ] All assumptions stated and confirmed
- [ ] Scope broken into discrete chunks if sprawling
- [ ] No conflicts with existing architecture or prior decisions
- [ ] User override logged if any challenge was overridden

## During Coordination

**Reporting style:** facts, not narration — assignments, verdicts and merge state, not a
recap of what a worker just said. Brevity never trims evidence: findings, rejection
reasons, measurements and merge receipts stay in full. See the `cas-supervisor` skill.

**Forward motion:** place the session on the six-rung exit ladder every turn and leave the next rung owned by a worker or by a scheduled supervisor reminder.

Record decisions as you go:
```
mcp__cas__memory action=remember title="..." content="..." tags="decision"
```

## Epic Planning Checklist

- Every subtask has a `demo_statement` (if not, it may be a horizontal slice — restructure)
- Investigation tasks use `task_type=spike` with question-based acceptance criteria
- When multiple approaches exist, a spike with a fit check comparison in `design_notes` precedes implementation tasks

## Review Gate (Per Task Completion)

- [ ] Tests exist and pass (including failure paths)
- [ ] No DRY violations or SRP violations
- [ ] No work outside declared layer boundary
- [ ] Output matches declared interface
- [ ] No magic numbers that should be configurable
- [ ] Obvious SOLID violations flagged with specifics

Supervisor close override constraints: [`supervisor_override`](cas-supervisor/references/reference.md#supervisor-override).

## Before Closing an EPIC

- Run `mcp__cas__coordination action=epic_status id=<epic-id>` — confirms every child task's `factory/<assignee>` branch is merged into the epic branch (this check is now also enforced automatically at `mcp__cas__task action=close` for Epic-type tasks and cannot be waived)
- Confirm task deliverables exist on the epic branch
- Run full test suite on epic branch

The `epic_status` action is a defense-in-depth diagnostic: the close-time gate (cas-8f8f) refuses to close an epic with stranded child branches regardless of supervisor overrides, but running `epic_status` mid-flight surfaces the same data so you can resolve merges without chasing a close-time error.

## Session End

Store a short summary memory tagged `summary`.
