---
name: cas-supervisor-checklist
description: Use at the start of a factory-supervisor session to load context, inspect EPICs, and confirm worker availability.
metadata:
  managed_by: cas
---

# Supervisor Checklist

## Session Start

0. **Preflight.** Run `cas factory preflight` ([preflight.md](../cas-supervisor/references/preflight.md)). Nonzero exit → fix the finding it names and rerun. If it reports a stale Cassy binary, stop here: **do not kill or restart `cas serve` from this active MCP session** — that stdio process is this session's Cassy-tool connection. Instead, ask the operator to rebuild Cassy and use the harness's MCP reconnect/restart control (or open a fresh supervisor session) to launch the new `cas serve`. Do not use `pkill` or any name-based process kill. Resume only after the Cassy tool list is restored, then rerun this checklist from step 0.

1. Identify yourself: `cas__coordination action=whoami`
2. Load EPIC/task context:
   ```
   cas__task action=list task_type=epic
   cas__task action=ready
   cas__task action=list status=blocked
   ```
3. Pull relevant memories and rules:
   ```
   cas__search action=search query="<keywords>" doc_type=entry limit=5
   ```
4. Check codemap freshness:
   - If `.claude/CODEMAP.md` is missing → run `/codemap` to generate it.
   - If it exists but is stale (structural changes since last update) → run `/codemap` to refresh.
   - Workers reference CODEMAP for codebase orientation — ensure it's current before spawning them.
5. Check worker availability: `cas__coordination action=worker_status`
6. **Session hygiene triage** — the SessionStart hook prepends a "⚠ Prior-factory
   WIP detected" banner to the supervisor context when the main worktree has
   uncommitted changes, with per-file attribution (last `cas-xxxx` commit)
   where git history permits. If you see that banner, decide salvage / commit /
   discard **before** spawning workers — otherwise a cherry-pick into `develop`
   will abort later.

   For a full on-demand report (including stale agents and orphan worktrees):
   ```
   cas__coordination action=gc_report
   ```
   The report's "Prior-factory WIP candidates" section mirrors the banner and
   is safe to re-run at any time; it never auto-deletes.

   For the full history of what prior sessions left behind, see
   `.cas/logs/factory-session-{YYYY-MM-DD}.log` (written automatically on
   `SessionEnd`; each block records session id, agent, worktree, and a
   `git status --porcelain` snapshot).

## Intake Gate (Before Planning)

Run the [intake gate](../cas-supervisor/references/intake.md) on every request; log any user override.

## During Coordination

Read `cas-supervisor` for authoritative task acceptance and the inbox/typed-wake policy.

Reporting style: [reporting-and-routing.md](../cas-supervisor/references/reporting-and-routing.md).

**Forward motion:** place the session on the six-rung exit ladder every turn and leave the next rung owned by a worker or by a scheduled supervisor reminder.

Record decisions as you go:
```
cas__memory action=remember title="..." content="..." tags="decision"
```

## Epic Planning and Review

Shape subtask specs (demo statements, spikes, fit checks) and review each delivery with [planning.md](../cas-supervisor/references/planning.md): tests added or updated for the change (they run once at assembly), no DRY/SRP or layer-boundary violations, interface and config compliance.

Supervisor close override constraints: [`supervisor_override`](../cas-supervisor/references/reference.md#supervisor-override).

## Before Closing an EPIC

- Run `cas__coordination action=epic_status id=<epic-id>` — confirms every child task's `factory/<assignee>` branch is merged into the epic branch. `cas__task action=close` on the epic enforces the same check and refuses stranded branches unless a live registered supervisor passes `stranded_branch_override="<inspection narrative>"`; it never waives genuinely unmerged content. Run `epic_status` mid-flight to resolve merges before the close-time error.
- Confirm task deliverables exist on the epic branch
- Launch the release gate detached on the assembled epic in its dedicated worktree, then run the [epic flow walk](../cas-supervisor/references/epic-flow-walk.md) concurrently when any child has a demo statement.
- Require both gate receipts and the single epic evidence note before epic close verification; apply task-verifier Step 0A with `verification_type=epic`.

## Session End

Store a short summary memory tagged `summary`.
