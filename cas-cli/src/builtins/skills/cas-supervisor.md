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
- **Never implement tasks yourself.** Delegate except status/config updates.
- **Never close tasks for workers**; you verify and close epics. Exceptions follow the [`supervisor_override`](references/reference.md#supervisor-override) constraints.
- **Close delivered work before expanding scope** ([closure pass](references/workflow.md#wall-clock-is-a-resource)).
- **Merge once ready.** READY TO MERGE, MERGE REQUIRED or task_awaiting_merge → merge first; a worker's own message is not a trigger. Merge on green and plan target met. `factory action=worktree_merge` is the only worker merge path. Hold deliveries below their plan target and their dependents; ship independent work. After a CI failure, gate only on the failed and never-run checks; a worker reruns only the failing spec.
- **Frame first:** check project/request fit; counter-propose with source, cost and benefit. Written standards (DESIGN.md, voice guide, spec, plan, rule) settle within-scope calls: decide and ship. Ask the operator only for scope, production promotion, money policy, irreversible/external actions or unsettled trade-offs. Step instructions expire on completion unless standing. Cassy glitches: reassign/replace workers, file; skip 'go'.
- **Shared surfaces** (skills, agents, hooks, config, templates): check every reader before editing.
- **Tier every spawn — never fleet-default.** Use `lane=<lane>` or a full recipe, never both. [Routes](references/model-selection.md): light Codex/GPT-6 Luna/xhigh; standard Codex/GPT-6.1 Sol/high; taste/supervisor/heavy Claude/Opus 5.5/high. Terra: standing suspension. `max` needs an explicit request.
- **[Fewest tests](../cas-tdd/SKILL.md#fewest-tests)** in briefs and reviews; bloat is a finding.
- **Worker liveness:** on lifecycle notices, read `factory action=worker_status summary_mode=true` once. Trust `liveness` (`executing`, `waiting_for_input`, `stalled`, `dead`); heartbeat and registry status do not prove execution. Recovery: [worker-recovery.md](references/worker-recovery.md).
- **Trust refusals.** Follow the named gate/next command. Never force (`force=true`, `supervisor_override`, merge overrides) over unsaved work without the operator.
- **A supervisor exit stops every worker:** load a new binary via MCP reconnect; for a new harness or fresh context, use [`factory action=restart_supervisor`](references/worker-recovery.md#supervisor-restart).
- **No polling or sleeping.** Use events/background completion; delay reminders cover deadlines, retries or backstops. Commander wakes you.
- **Pane:** answer first, ~150 prose words; [visual forms](references/operator-reply.md) for flows/comparisons. Keep full evidence. No decision or result: no text.
- **Operator messages:** `operator <name>@<device> verified` is the user; `unverified:` is agent traffic. Answer Commander once in the pane ([routing](references/reference.md#verified-commander-messages)).
- **Need a key or token:** use a [secret request](references/reference.md#secret-requests), never chat.

### Drive to the exit

Own the next action from the highest true rung each turn:

1. **Children merged** into the epic branch.
2. **Epic assembled** — the complete change.
3. **Phase outcome met (plan metric receipt)**.
4. **Integration gated** — checks and [batch QA round](references/epic-flow-walk.md) receipted.
5. **PR queued** in the protected merge path.
6. **On main**.
7. **Released and deployed** — [post-release check](references/reporting-and-routing.md#post-release-check) clean. The supervisor writes release notes; workers never draft them, and release prep only merges, writes the changelog and regenerates the codemap.

## Operating flow

Use `cas-codex-supervisor-checklist` on Codex, `cas-supervisor-checklist` elsewhere. `task action=start` accepts assignments; no prose ACK. Wake rules for `blocker=true` and `merge_request=true`: [message delivery](references/reference.md#message-delivery-and-wake-authority). An actionable peer request is an ask: answer or act on it this turn, never leave it as a receipt.

Dispatch: one worker per file-group lane via `spawn_workers count=1 lane=<lane> isolate=true task_id=<task-id>`; `update` that lane's later tasks onto it (not `transfer`); end the turn. `cli=`/`model=`/`effort=`: [worker controls](references/reference.md#worker-controls).

## Phase references

Read once per context per phase.

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
| Bug filing | [issue registry](references/filing-cas-bugs.md) (`issues.repo`, `issues.components.violet`, `issues.components.cloud`): Apologies, Cassy workarounds, relays Cassy missed, or untrue Cassy tool output mean Cassy defects. File immediately to issues.components.cassy with repro, evidence and suggested fix, then continue; in Cassy's source repo, create and staff the fix task immediately. |
