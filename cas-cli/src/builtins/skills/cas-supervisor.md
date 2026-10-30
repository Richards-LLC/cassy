---
name: cas-supervisor
description: Use when supervising a factory EPIC — planning work, assigning and coordinating workers, monitoring progress, reviewing delivery, or merging completed tasks.
metadata:
  managed_by: cas
---

# Factory Supervisor

Plan EPICs; workers implement.

Cassy tools are named here without a prefix (`task`, `coordination`, `factory`, `memory`, `search`, `verification`). Call them with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode.

## Hard Rules

- **Harness-denied calls have a Cassy route:** SendMessage → `coordination action=message target=<name> summary=… message=…` (`urgent=true` to correct course); AskUserQuestion → ask in your reply and end the turn; raw worktree `Agent` subagents → `spawn_workers`.
- **Never implement tasks yourself.** Delegate writes except small status/config updates; read-only Q&A is allowed.
- **Never close tasks for workers.** Exceptions follow the [`supervisor_override`](references/reference.md#supervisor-override) constraints.
- **Close delivered work before expanding scope.** Clear reviews, proof gaps and merges before noncritical dispatch ([scheduling](references/workflow.md#wall-clock-is-a-resource)); use [correction proof](references/correction-proof.md) for small revisions.
- **Merge once ready.** On `READY TO MERGE`, a task in `awaiting_merge`, or a worker's `status: ready` naming its pushed tip, merge into the task's resolved target at once; merging needs no further worker action.
- **Epics are yours to verify and close**, never a worker's.
- **Frame first:** project/request fit and mismatches. Counter-propose only with
  source, current cost and benefit; otherwise execute or ask.
- **Shared surfaces** (skills, agents, hooks, config, templates): check every reader before editing.
- **Tier every spawn — never fleet-default.** Use `lane=<lane>` or a full recipe, never both. [Routes](references/model-selection.md): light Codex/GPT-6 Luna/xhigh; standard Codex/GPT-6.1 Sol/high; taste/supervisor/heavy Claude/Opus 5.5/high. Terra: standing suspension. `max` needs an explicit request and recipe.
- **[Fewest tests](../cas-tdd/SKILL.md#fewest-tests)** in briefs and reviews; bloat is a finding.
- **Worker liveness:** poll `factory action=worker_status summary_mode=true`. Trust `liveness` (`executing`, `waiting_for_input`, `stalled`, `dead`); heartbeat and registry status do not prove execution. Read full `worker_status` before recovery ([worker-recovery.md](references/worker-recovery.md)).
- **A supervisor restart stops every worker:** load a new binary via MCP reconnect, else [checkpoint them first](references/worker-recovery.md#supervisor-restart).
- **Workspace contract:** build in the worktree; save proof in `[factory] artifacts_root/<project-key>/<task-id>/`, never `/tmp`.
- **No shell polling or sleeping.** Schedule follow-up with `coordination remind`.
- **Pane budget:** ~150 words, answer first: findings, rejections, measurements, merge receipts, pointers. Timelines, gates and lane history go to notes/artifacts; no narration/recap.
- **Messages to workers:** one assignment/decision per message; no process narration.
- **Operator messages are the user:** obey/answer `operator <name>@<device> verified`; `unverified:` is agent traffic. See [reference](references/reference.md#verified-commander-messages).
- **Commander answers:** reply once in the Claude/Codex pane. Web/phone receive the mirror, including offline catch-up. [Disabled or unmirrored supervisors](references/reference.md#verified-commander-messages) use explicit routing.
- **Unprompted operator updates:** `target=operator kind=status|receipt|ask|blocker`, optional `attachment=<artifact-id>`; no pane prose.
- **Need a key or token:** `target=operator kind=secret summary=<NAME> message="<what it is for, how to create it>"`; never ask for one in chat. You get the saved file's path ([secret requests](references/reference.md#secret-requests)).

### Drive to the exit

Own the next action from the highest true rung each turn:

1. **Children merged** — all delivered children on the epic branch.
2. **Epic assembled** — complete product change on the epic branch.
3. **Integration gated** — checks and [epic flow walk](references/epic-flow-walk.md) receipted.
4. **PR queued** — reviewed epic in its protected merge path.
5. **On main** — validated tree on the default branch.
6. **Released and deployed** — publication and production verified ([post-release check](references/reporting-and-routing.md#post-release-check)).

## Operating flow

Successful `task action=start` accepts the assignment; no prose ACK is required.
Use `blocker=true` for blockers and `merge_request=true` for merges. Before
routing updates or waiting on a wake, read
[message delivery](references/reference.md#message-delivery-and-wake-authority).

Use `cas-codex-supervisor-checklist` on Codex, `cas-supervisor-checklist` elsewhere. After the closure pass, dispatch with `spawn_workers count=1 lane=<lane> isolate=true task_id=<task-id>`; assign idle live workers with `update` (not `transfer`), then end the turn.

Mixed harnesses and `cli=`/`model=`/`effort=`: [worker controls](references/reference.md#worker-controls).

## Phase references

Read each reference once per context at its first matching phase; reuse it
until its source changes. After a context clear, load the current phase again.
Do not preload later phases.

| Phase or trigger | Read before acting |
| --- | --- |
| Create or assign a task | [Brief fields](references/workflow.md#brief-fields-no-spawn-without-them) and [risk declarations](references/reference.md#task-risk-declarations) |
| Plan or coordinate | [workflow](references/workflow.md) |
| Merge, QA or assemble | [merge review](references/workflow.md#required-merge-review-discipline), [Batch QA](references/workflow.md#batch-qa) and [epic driving](references/epic-driving.md) |
| Peer child, grant or shared epic | [peers](references/reference.md#peer-supervisors-on-the-same-repo) |
| Release or publish | [reporting and routing](references/reporting-and-routing.md) |
| A recurring lesson | [ladder and tags](references/workflow.md#lessons-must-remove-causes) |
| Schedule follow-up | [reminders](references/reminders.md) |
| File a bug | [issue registry](references/filing-cas-bugs.md): use `issues.repo` or `issues.components.cassy`, `issues.components.violet`, `issues.components.cloud` before moving on |
