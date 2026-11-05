---
name: cas-supervisor-checklist
description: Use at the start of a factory-supervisor session to load context, inspect EPICs, and confirm worker availability.
metadata:
  managed_by: cas
---

# Supervisor Checklist

## Session Start

0. **Preflight:** call `system action=preflight` (without MCP, `cas --json factory preflight`) and act on `factory_blocked` and the finding codes per [preflight.md](../cas-supervisor/references/preflight.md); `binary.source_evidence_missing` on healthy MCP needs no rebuild. To replace a deployment, **do not kill or restart `cas serve` from this active MCP session**: ask the operator to install the intended build and use the harness's MCP reconnect/restart control. Do not use `pkill`. A plain supervisor exit stops every worker ([supervisor restart](../cas-supervisor/references/worker-recovery.md#supervisor-restart)). Once the Cassy tool list is restored, rerun this checklist from step 0.
1. `coordination action=whoami`, then `task action=list task_type=epic`, `task action=ready` and `task action=list status=blocked`.
2. `search action=search query="<keywords>" doc_type=entry limit=5`.
3. `factory action=worker_status`.
4. If the SessionStart banner reports prior-factory WIP, decide salvage, commit or discard before spawning (history: `.cas/logs/factory-session-{YYYY-MM-DD}.log`).
5. Before human-facing work, confirm the target has a `DESIGN.md` ([DESIGN.md first](../design-spec/references/design-md-first.md)).

## Every request

Run the [intake gate](../cas-supervisor/references/intake.md); log any user override.

## During Coordination

- Leave the next exit rung owned by a worker or an event reminder each turn.
- Run the [closure pass](../cas-supervisor/references/workflow.md#wall-clock-is-a-resource) before new noncritical dispatch; for a revised delivery, use [correction proof](../cas-supervisor/references/correction-proof.md).
- Shape and review subtasks with [planning.md](../cas-supervisor/references/planning.md), asking for the [fewest tests](../cas-tdd/SKILL.md#fewest-tests) that fail without the change.
- Close overrides: [`supervisor_override`](../cas-supervisor/references/reference.md#supervisor-override).

## Before Closing an EPIC

Require **Phase outcome met (plan metric receipt)**, then close out once, in order: freeze scope; one full review; one batch fix; re-review only the fix diff; one QA round with the [epic flow walk](../cas-supervisor/references/epic-flow-walk.md) when a child has a demo statement; enqueue. Epic close verification uses `verification_type=epic`.
