---
name: cas-supervisor-checklist
description: Use at the start of a factory-supervisor session to load context, inspect EPICs, and confirm worker availability.
metadata:
  managed_by: cas
---

# Supervisor Checklist

## Session Start

0. **Preflight.** From a connected CAS MCP session, call `system action=preflight`; otherwise run `cas --json factory preflight`. Follow [preflight.md](../cas-supervisor/references/preflight.md), using `factory_blocked` and the finding codes rather than `binary.state` alone. A healthy MCP connection with `factory_blocked=false` and `binary.source_evidence_missing` can continue startup while gathering source comparison evidence; it does not require a rebuild or reconnect.

   If the findings establish a deployment mismatch, resolve the intended deployment before replacing it: **do not kill or restart `cas serve` from this active MCP session** — that stdio process is this session's Cassy-tool connection. If replacement is needed, ask the operator to install the intended clean build and use the harness's MCP reconnect/restart control, which keeps every worker running. A plain supervisor exit stops every worker in this factory; use `factory action=restart_supervisor` for a new harness or fresh context ([supervisor restart](../cas-supervisor/references/worker-recovery.md#supervisor-restart)). Do not use `pkill` or any name-based process kill. After replacement, confirm the Cassy tool list is restored, then rerun this checklist from step 0.

1. Identify yourself: `coordination action=whoami`
2. Load EPIC/task context:

   ```text
   task action=list task_type=epic
   task action=ready
   task action=list status=blocked
   ```

3. Pull relevant memories and rules:

   ```text
   search action=search query="<keywords>" doc_type=entry limit=5
   ```

4. Check codemap freshness:
   - If `.claude/CODEMAP.md` is missing → run `/codemap` to generate it.
   - If it exists but is stale (structural changes since last update) → run `/codemap` to refresh.
   - Workers reference CODEMAP for codebase orientation — ensure it's current before spawning them.
   - Before human-facing work, confirm each target project has a `DESIGN.md`
     ([DESIGN.md first](../design-spec/references/design-md-first.md)); plan a
     `design-spec` task for a missing one.
5. Check worker availability: `factory action=worker_status`
6. **Session hygiene triage** — the SessionStart hook prepends a "⚠ Prior-factory
   WIP detected" banner to the supervisor context when the main worktree has
   uncommitted changes, with per-file attribution (last `cas-xxxx` commit)
   where git history permits. If you see that banner, decide salvage / commit /
   discard **before** spawning workers — otherwise a cherry-pick into `develop`
   will abort later.

   For a full on-demand report (including stale agents and orphan worktrees):

   ```text
   factory action=gc_report
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

**Closure first:** run the [closure pass](../cas-supervisor/references/workflow.md#wall-clock-is-a-resource) before new noncritical dispatch. For a revised delivery, use [correction proof](../cas-supervisor/references/correction-proof.md) to decide what must rerun and share one immutable candidate.

Record decisions as you go:

```text
memory action=remember title="..." content="..." tags="decision"
```

## Epic Planning and Review

Shape subtask specs (demo statements, spikes, fit checks) and review each delivery with [planning.md](../cas-supervisor/references/planning.md): the [fewest tests](../cas-tdd/SKILL.md#fewest-tests) that fail without the change, bloat being a finding (they run once at the final gate), no DRY/SRP or layer-boundary violations, interface and config compliance.

Supervisor close override constraints: [`supervisor_override`](../cas-supervisor/references/reference.md#supervisor-override).

## Before Closing an EPIC

- Run `factory action=epic_status id=<epic-id>` — confirms every child task's `factory/<assignee>` branch is merged into the epic branch. `task action=close` on the epic enforces the same check and refuses stranded branches unless a live registered supervisor passes `stranded_branch_override="<inspection narrative>"`; it never waives genuinely unmerged content. Run `epic_status` mid-flight to resolve merges before the close-time error. `EPIC CLOSE CHECK INCOMPLETE` is resumable: retry the same close until it completes.
- Confirm task deliverables exist on the epic branch
- Run the closeout once, in order: freeze scope; one full review and the [interim queue precheck](../cas-supervisor/references/workflow.md#interim-queue-precheck) together; one batch fix; re-review only the fix diff; one QA round using the [epic flow walk](../cas-supervisor/references/epic-flow-walk.md) when any child has a demo statement; enqueue.
- Require the single epic evidence note before epic close verification; apply task-verifier Step 0A with `verification_type=epic`.

## Session End

Store a short summary memory tagged `summary`.
