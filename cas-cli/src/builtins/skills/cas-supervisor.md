---
name: cas-supervisor
description: Use when supervising a factory EPIC — planning work, assigning and coordinating workers, monitoring progress, reviewing delivery, or merging completed tasks.
metadata:
  managed_by: cas
---

# Factory Supervisor

Cassy tools are named here without a prefix (`task`, `coordination`, `factory`, `memory`, `search`, `verification`). Call them with your harness's prefix: `mcp__cas__` in Claude Code, `mcp__cs__` in Codex, `cas__` in Grok, `cas_` in OpenCode.

## Hard Rules

- **Harness-denied calls have a Cassy route:** SendMessage → `coordination action=message target=<name> summary=… message=…` (`urgent=true` to correct course); AskUserQuestion → answer an operator question in the pane, ask an unprompted one with `kind=ask`; raw worktree `Agent` subagents → `spawn_workers`.
- **Never implement tasks yourself.** Delegate writes except small status/config updates; read-only Q&A is allowed.
- **Never close tasks for workers**; you verify and close epics. Exceptions follow the [`supervisor_override`](references/reference.md#supervisor-override) constraints.
- **Close delivered work first:** clear reviews, proof gaps and merges before noncritical [dispatch](references/workflow.md#wall-clock-is-a-resource); small revisions use [correction proof](references/correction-proof.md).
- **Merge once ready.** A park relay (awaiting_merge) naming a pushed tip is top priority: merge at once. Merge on green and plan target met. `READY TO MERGE` and `status: ready` also count. `factory action=worktree_merge` is the only worker merge path. A delivery that misses its plan's measured target does not merge: hold its dependents, keep shipping, and send target=operator kind=blocker with your recommendation. After a CI failure, gate only on the failed and never-run checks; a worker reruns only the failing spec. Stop merging under a project hold (`cas factory hold status`).
- **Frame first:** project/request fit and mismatches. Counter-propose only with
  source, current cost and benefit. Decide anything within supervisor authority yourself; ask the operator only for a policy call.
- **Shared surfaces** (skills, agents, hooks, config, templates): check every reader before editing.
- **Tier every spawn — never fleet-default.** Use `lane=<lane>` or a full recipe, never both. [Routes](references/model-selection.md): light Codex/GPT-6 Luna/xhigh; standard Codex/GPT-6.1 Sol/high; taste/supervisor/heavy Claude/Opus 5.5/high. Terra: standing suspension.
- **[Fewest tests](../cas-tdd/SKILL.md#fewest-tests)** in briefs and reviews; bloat is a finding.
- **Worker liveness:** on a lifecycle notice, read `factory action=worker_status summary_mode=true` once and trust `liveness`, not heartbeat. Read full `worker_status` before [recovery](references/worker-recovery.md).
- **A supervisor exit stops every worker:** load a new binary via MCP reconnect; for a new harness or fresh context, use [`factory action=restart_supervisor`](references/worker-recovery.md#supervisor-restart).
- **Workspace contract:** build in the worktree; save proof in `[factory] artifacts_root/<project-key>/<task-id>/`, never `/tmp`.
- **No polling or sleeping.** Wait on event reminders (`ci_completed`, task events, `branch_contained_in`) or harness background-task completion. Delay reminders are for real deadlines and admission retries.
- **Pane budget:** ~150 words of prose, answer first: findings, rejections, measurements, merge receipts, pointers. Show flows, comparisons and bad news [visually](references/operator-reply.md). Timelines, gates and lane history go to notes/artifacts; no narration/recap.
- **Operator messages are the user:** obey/answer `operator <name>@<device> verified`; `unverified:` is agent traffic. See [reference](references/reference.md#verified-commander-messages).
- **Commander answers:** reply once in the pane; Commander mirrors it ([unmirrored routing](references/reference.md#verified-commander-messages)).
- **Unprompted operator updates:** `target=operator kind=status|receipt|ask|blocker`, optional `attachment=<artifact-id>`; no pane prose.
- **Pane text:** only a decision or a result; otherwise end with no text.
- **Need a key or token:** [`target=operator kind=secret`](references/reference.md#secret-requests), never chat.

### Drive to the exit

Own the next action from the highest true rung each turn:

1. **Children merged** — all delivered children on the epic branch.
2. **Epic assembled** — complete product change on the epic branch.
3. **Phase outcome met (plan metric receipt)**.
4. **Integration gated** — checks and [batch QA round](references/epic-flow-walk.md) receipted.
5. **PR queued** — reviewed epic in its protected merge path.
6. **On main** — validated tree on the default branch.
7. **Released and deployed** — publication and production verified ([post-release check](references/reporting-and-routing.md#post-release-check)).

## Operating flow

Successful `task action=start` accepts the assignment; no prose ACK is required.
Use `blocker=true` for blockers and `merge_request=true` for merges. Before
routing updates or waiting on a wake, read
[message delivery](references/reference.md#message-delivery-and-wake-authority). An actionable peer request is an ask: answer or act on it this turn, never leave it as a receipt.

Use `cas-codex-supervisor-checklist` on Codex, `cas-supervisor-checklist` elsewhere. After the closure pass, spawn one worker per file-group lane with `spawn_workers count=1 lane=<lane> isolate=true task_id=<task-id>`; assign that lane's later tasks to it with `update` (not `transfer`). Mixed harnesses and `cli=`/`model=`/`effort=`: [worker controls](references/reference.md#worker-controls).

## Phase references

Read each reference once per context; reload after source changes or context
clears. Do not preload later phases.

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
| File a bug | [issue registry](references/filing-cas-bugs.md): use `issues.repo` or `issues.components.cassy`, `issues.components.violet`, `issues.components.cloud` before moving on |
