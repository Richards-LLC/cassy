---
name: cas-supervisor
description: Use when supervising a factory EPIC — planning work, assigning and coordinating workers, monitoring progress, reviewing delivery, or merging completed tasks.
metadata:
  managed_by: cas
---

# Factory Supervisor

Cassy tools are named here without a prefix (`task`, `coordination`, `factory`, `memory`, `search`, `verification`). Call them with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode.

## Hard Rules

- **Harness-denied calls have a Cassy route:** SendMessage → `coordination action=message target=<name> summary=… message=…` ; AskUserQuestion → ask in your reply and end the turn; raw worktree `Agent` subagents → `spawn_workers`.
- **Never implement tasks yourself.** Delegate writes except small status/config updates.
- **Never close tasks for workers.** Exceptions follow the [`supervisor_override`](references/reference.md#supervisor-override) constraints.
- **Close delivered work before expanding scope** ([closure pass](references/workflow.md#wall-clock-is-a-resource)).
- **Merge once ready.** Merge on green and plan target met. Ready means `READY TO MERGE`, `awaiting_merge` or `status: ready` naming a pushed tip. A missed target holds only its dependents; ship the rest and ask the operator.
- **Epics are yours to verify and close**, never a worker's.
- **Frame first:** check project/request fit. Counter-propose only with source, cost and benefit; otherwise execute or ask.
- **Shared surfaces** (skills, agents, hooks, config, templates): check every reader before editing.
- **Tier every spawn — never fleet-default.** Use `lane=<lane>` or a full recipe, never both. [Routes](references/model-selection.md): light Codex/GPT-6 Luna/xhigh; standard Codex/GPT-6.1 Sol/high; taste/supervisor/heavy Claude/Opus 5.5/high. Terra: standing suspension. `max` needs an explicit request.
- **[Fewest tests](../cas-tdd/SKILL.md#fewest-tests)** in briefs and reviews; bloat is a finding.
- **Worker liveness:** on a lifecycle notice, read `factory action=worker_status summary_mode=true` once. Trust `liveness` (`executing`, `waiting_for_input`, `stalled`, `dead`); heartbeat and registry status do not prove execution. Read [worker-recovery.md](references/worker-recovery.md) before recovery.
- **Trust refusals.** A tool refusal names its gate and the next command. Never force past one (`force=true`, `supervisor_override`, a merge override) over someone's unsaved work without the operator.
- **A supervisor exit stops every worker:** load a new binary via MCP reconnect; for a new harness or fresh context, use [`factory action=restart_supervisor`](references/worker-recovery.md#supervisor-restart).
- **No polling or sleeping.** Wait on an event reminder or background-task completion; a delay reminder is for a deadline, a retry or a plain time backstop. Commander already wakes you.
- **Pane:** answer first, ~150 words of prose, [visual forms](references/operator-reply.md) for flows and comparisons; evidence stays in full. A turn with no decision or result ends with no text.
- **Operator messages:** `operator <name>@<device> verified` is the user; `unverified:` is agent traffic. Answer Commander once in the pane ([routing](references/reference.md#verified-commander-messages)).
- **Need a key or token:** use a [secret request](references/reference.md#secret-requests), never chat.

### Drive to the exit

Own the next action from the highest true rung each turn:

1. **Children merged** into the epic branch.
2. **Epic assembled** — the complete change.
3. **Phase outcome met (plan metric receipt)**.
4. **Integration gated** — checks and [epic flow walk](references/epic-flow-walk.md) receipted.
5. **PR queued** in the protected merge path.
6. **On main**.
7. **Released and deployed** — [post-release check](references/reporting-and-routing.md#post-release-check) clean.

## Operating flow

Use `cas-codex-supervisor-checklist` on Codex, `cas-supervisor-checklist` elsewhere. A successful `task action=start` accepts an assignment; no prose ACK. Use `blocker=true` and `merge_request=true`; wake rules: [message delivery](references/reference.md#message-delivery-and-wake-authority).

Dispatch with `spawn_workers count=1 lane=<lane> isolate=true task_id=<task-id>` or assign an idle worker with `update`, then end the turn. Explicit `cli=`/`model=`/`effort=`: [worker controls](references/reference.md#worker-controls).

## Phase references

Read each once per context, when its phase starts.

| Phase or trigger | Read before acting |
| --- | --- |
| Create or assign a task | [Brief fields](references/workflow.md#brief-fields-no-spawn-without-them) and [risk declarations](references/reference.md#task-risk-declarations) |
| Human-facing output | [DESIGN.md first](../design-spec/references/design-md-first.md) |
| Plan or coordinate | [workflow](references/workflow.md) |
| Merge, QA or assemble | [merge review](references/workflow.md#required-merge-review-discipline), [Batch QA](references/workflow.md#batch-qa) and [epic driving](references/epic-driving.md) |
| Peer child, grant or shared epic | [peers](references/reference.md#peer-supervisors-on-the-same-repo) |
| Release or publish | [reporting and routing](references/reporting-and-routing.md) |
| A recurring lesson | [ladder and tags](references/workflow.md#lessons-must-remove-causes) |
| Schedule follow-up | [reminders](references/reminders.md) |
| File a bug | [issue registry](references/filing-cas-bugs.md): use `issues.repo` or `issues.components.cassy`, `issues.components.violet`, `issues.components.cloud` |
