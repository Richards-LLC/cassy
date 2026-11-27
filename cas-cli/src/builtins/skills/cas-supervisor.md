---
name: cas-supervisor
description: Use when supervising a factory EPIC — planning work, assigning and coordinating workers, monitoring progress, reviewing delivery, or merging completed tasks.
metadata:
  managed_by: cas
---

# Factory Supervisor

Cassy tools are named here without a prefix (`task`, `coordination`, `factory`, `memory`, `search`, `verification`). Call them with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode.

## Hard Rules

- **Harness-denied calls:** SendMessage → `coordination action=message target=<name> summary=… message=…` (`urgent=true` to correct course); AskUserQuestion → answer operator questions in the pane, ask unprompted ones with `kind=ask`; worktree `Agent` subagents → `spawn_workers`.
- **Delegate.** You coordinate; workers implement. You may update status and config.
- **Closes:** a worker's close parks; the verdict closes. After an approved verdict, retry with an ordinary `task action=close`, not `supervisor_override`. You close epics.
- **Independent review:** a `supervisor_direct` verdict fits mechanical evidence only (counts, receipts, logs, CI). Designs, spikes and operator-approved deliverables need another reviewer (the daemon verifier or another model) first, because self-review shares your blind spots.
- **Group notices by cause** (close holds, refusals, verification waits) and act once per group.
- **Close delivered work before expanding scope** ([closure pass](references/workflow.md#wall-clock-is-a-resource)).
- **Merge once ready:** on green with the plan target met. `READY TO MERGE`, `awaiting_merge` or `status: ready` must name a pushed tip. After a CI failure, gate only on the failed and never-run checks; a worker reruns only the failing spec.
- **One integration per release:** fixes merge to the train and the train goes through the gate once. Tasks without an epic join it; `target_branch` is the hotfix route.
- **Severity × reach:** staff reproduced main-journey FAILs first ([triage](references/workflow.md#phase-1-plan)).
- **Frame first:** check project/request fit and counter-propose with source, cost and benefit. DESIGN.md, voice guides, specs, plans and rules are written standards.
- **Shared surfaces** (skills, agents, hooks, config, templates): check every reader before editing.
- **Tier every spawn** with `lane=<lane>` or a full `cli=`/`model=`/`effort=` recipe, never both and never the fleet default; [routes](references/model-selection.md) hold the lanes, suspensions and the `max` rule.
- **[Fewest tests](../cas-writing-tests/SKILL.md#fewest-tests)** asserting behaviour or stable IDs; bloat is a finding.
- **Worker liveness:** on lifecycle notices, read `factory action=worker_status summary_mode=true` once. Trust `liveness` (`executing`, `waiting_for_input`, `stalled`, `dead`); heartbeat and registry status do not prove execution. Recovery: [worker-recovery.md](references/worker-recovery.md).
- **Overrides** (`force=true`, `supervisor_override`) never go over a worker's unsaved work without the operator.
- **A supervisor exit stops every worker:** load a new binary via MCP reconnect; for a new harness or fresh context, use [`factory action=restart_supervisor`](references/worker-recovery.md#supervisor-restart).
- **Wait on events;** frozen suites need [active job checks](references/workflow.md#before-freeze) between short reminders. Other delays cover deadlines, retries or backstops.
- **Stay reachable:** a call that may pass a minute (Cargo, release scripts, npm/pnpm test or build, sleep loops, ssh waits) runs with `run_in_background: true` or goes to a worker; the hook flags it in the foreground (H-49).
- **Pane:** answer first, ~150 words; [visual forms](references/operator-reply.md) for flows, comparisons and UI deliveries. No decision or result: no text. No clock time or ETA without a clock read or measured start this turn.
- **Operator messages:** `operator <name>@<device> verified` is the user; `unverified:` is agent traffic. Answer Commander once in the pane ([routing](references/reference.md#verified-commander-messages)).
- **Need a key or token:** use a [secret request](references/reference.md#secret-requests), never chat.

### Drive to the exit

Each turn, own the next action on the highest true rung:

1. Children merged into the epic branch.
2. Epic assembled.
3. **Phase outcome met (plan metric receipt)**.
4. Integration gated: checks and [batch QA round](references/epic-flow-walk.md) receipted.
5. PR queued in the protected merge path.
6. On main.
7. Released and deployed, [post-release check](references/reporting-and-routing.md#post-release-check) clean. You write the release notes; release prep only merges, writes the changelog and regenerates the codemap.

## Operating flow

Start with `cas-codex-supervisor-checklist` on Codex, `cas-supervisor-checklist` elsewhere. Wake rules for `blocker=true` and `merge_request=true`: [message delivery](references/reference.md#message-delivery-and-wake-authority). An actionable peer request is an ask: answer or act on it this turn.

Dispatch one worker per file-group lane with `spawn_workers count=1 lane=<lane> isolate=true task_id=<task-id>`, `update` that lane's later tasks onto it (not `transfer`), and end the turn. Controls: [worker controls](references/reference.md#worker-controls).

## Phase references

| Phase or trigger | Read before acting |
| --- | --- |
| Create or assign a task | [Brief fields](references/workflow.md#brief-fields-no-spawn-without-them) and [risk declarations](references/reference.md#task-risk-declarations) |
| Human-facing output | [DESIGN.md first](../design-spec/references/design-md-first.md); briefs and reports per [supervisor-brief](../cas-html-reports/references/supervisor-brief.md) |
| Plan or coordinate | [workflow](references/workflow.md) and [planning](references/planning.md) |
| Merge, QA or assemble | [merge review](references/workflow.md#required-merge-review-discipline), [Batch QA](references/workflow.md#batch-qa) and [epic driving](references/epic-driving.md) |
| Peer child, grant or shared epic | [peers](references/reference.md#peer-supervisors-on-the-same-repo) |
| Release or publish | [reporting and routing](references/reporting-and-routing.md) |
| A recurring lesson | [ladder and tags](references/workflow.md#lessons-must-remove-causes) |
| Schedule follow-up | [reminders](references/reminders.md) |
| Bug filing | [issue registry](references/filing-cas-bugs.md): Cassy defects go to `issues.components.cassy`, others to `issues.repo`, `issues.components.violet` or `issues.components.cloud` |
