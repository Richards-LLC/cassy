---
name: cas-supervisor-checklist
description: Use at the start of a factory-supervisor session to load context, inspect EPICs, and confirm worker availability.
metadata:
  managed_by: cas
---

# Supervisor Checklist

## Session Start

0. **Preflight:** call `system action=preflight` (without MCP, `cas --json factory preflight`) and act on `factory_blocked` and the finding codes per [preflight.md](../cas-supervisor/references/preflight.md). After an MCP reconnect, rerun from step 0.
1. `coordination action=whoami`, then `task action=list task_type=epic`, `task action=ready` and `task action=list status=blocked`.
2. `search action=search query="<keywords>" doc_type=entry limit=5`.
3. `factory action=worker_status`.
4. If the SessionStart banner reports prior-factory WIP, decide salvage, commit or discard before spawning (history: `.cas/logs/factory-session-{YYYY-MM-DD}.log`).
5. Before human-facing work, confirm the target has a `DESIGN.md` ([DESIGN.md first](../design-spec/references/design-md-first.md)).

## Every request

Run the [intake gate](../cas-supervisor/references/intake.md); log any user override.

## During Coordination

- Shape and review subtasks with [planning.md](../cas-supervisor/references/planning.md); for a revised delivery, use [correction proof](../cas-supervisor/references/correction-proof.md).
- Close overrides: [`supervisor_override`](../cas-supervisor/references/reference.md#supervisor-override).

## Before Closing an EPIC

Require **Phase outcome met (plan metric receipt)**, then run the [Phase 4 closeout](../cas-supervisor/references/workflow.md#phase-4-complete) once. Epic close verification needs the release `batch_receipt` and uses `verification_type=epic`.
