---
name: cas-codex-supervisor-checklist
description: Use at the start of a Codex factory-supervisor session to load context, inspect EPICs, and confirm worker availability.
metadata:
  managed_by: cas
---

# Codex Supervisor Checklist

## Codex constraints

- Codex runs Cassy's lifecycle hooks once they are installed and trusted (`cas update` writes and trusts them; spawns refuse while they are untrusted). Hooks only orient and guard. Codex loads no `.md` agents, so call the Cassy tools explicitly with the Codex prefix. Triage from the supervisor core in your developer instructions; after a compaction continue from it. Open a `cas-supervisor` reference only for epic planning, releases, worker recovery or an unfamiliar refusal.
- Coordinate workers; never implement a worker's task yourself or close one outside the Cassy lifecycle.
- Answer verified Commander questions once in your final pane reply, shaped by the [reply contract](../cas-supervisor/references/operator-reply.md); unprompted updates follow the [routing policy](../cas-supervisor/references/reference.md#verified-commander-messages).
- Every spawn passes `lane=` (preferred) or a complete `cli=`, `model=`, and `effort=` recipe ([workflow.md](../cas-supervisor/references/workflow.md)).
- Read and post Slack only through the `violet` skill (`violet.violet_read` / `violet.violet_post`), never the Codex Slack app; an older or untrusted hook install does not enforce this.

## Session Start

0. **Preflight:** call `system action=preflight` (without MCP, `cas --json factory preflight`) and act on `factory_blocked` and the finding codes per [preflight.md](../cas-supervisor/references/preflight.md); `binary.source_evidence_missing` on healthy MCP needs no rebuild. To replace a deployment, **do not kill or restart `cas serve` from this active MCP session**: ask the operator to install the intended build and use the harness's MCP reconnect/restart control. Do not use `pkill`. A plain supervisor exit stops every worker ([supervisor restart](../cas-supervisor/references/worker-recovery.md#supervisor-restart)). Once the Cassy tool list is restored, rerun this checklist from step 0.
1. `coordination action=whoami`, then `task action=list task_type=epic`, `task action=ready` and `task action=list status=blocked`.
2. `search action=search query="<keywords>" doc_type=entry limit=5`.
3. `factory action=worker_status`.
4. Run `factory action=gc_report` yourself, since an untrusted hook shows no WIP banner. Decide salvage, commit or discard for any prior-factory WIP before spawning (history: `.cas/logs/factory-session-{YYYY-MM-DD}.log`).
5. Before human-facing work, confirm the target has a `DESIGN.md` ([DESIGN.md first](../design-spec/references/design-md-first.md)).

## Every request

Run the [intake gate](../cas-supervisor/references/intake.md); log any user override.

## During Coordination

- Leave the next exit rung owned by a worker or an event reminder each turn.
- Run the [closure pass](../cas-supervisor/references/workflow.md#wall-clock-is-a-resource) before new noncritical dispatch; for a revised delivery, use [correction proof](../cas-supervisor/references/correction-proof.md).
- Shape and review subtasks with [planning.md](../cas-supervisor/references/planning.md), asking for the [fewest tests](../cas-tdd/SKILL.md#fewest-tests) that fail without the change.
- Close overrides: [`supervisor_override`](../cas-supervisor/references/reference.md#supervisor-override).

## Before Closing an EPIC

Require **Phase outcome met (plan metric receipt)**, then close out once, in order: freeze scope; one full review with the [interim queue precheck](../cas-supervisor/references/workflow.md#interim-queue-precheck); one batch fix; re-review only the fix diff; the [batch QA round](../cas-supervisor/references/epic-flow-walk.md); enqueue. In qa.mode=batch (the default) the release's batch QA round at the frozen tip is the only walk; per-task QA evidence applies only in qa.mode=per_task. Epic close verification requires the release `batch_receipt` ledger and uses `verification_type=epic`.
