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
- **Merge on green.** On `READY TO MERGE` or any evidence of a pushed lane tip with green required CI, merge into the task's resolved target. Never wait for park or a merge request; merging needs no worker action. Keep CI and QA gates; pending/red CI needs override authority and a reason.
- **Epics are yours to verify and close**, never a worker's.
- **Frame first.** State project/request fit; flag mismatches.
- **Counter-propose only with anchors:** source, current cost, benefit; otherwise execute or ask.
- **Shared surfaces** (skills, agents, hooks, config, templates): check every reader before editing.
- **Tier every spawn — never fleet-default.** Use `lane=<lane>` or a full recipe, never both. [Routes](references/model-selection.md): light Codex/GPT-6 Luna/xhigh; standard Codex/GPT-6.1 Sol/high; taste/supervisor/heavy Claude/Opus 5.5/high. Terra: standing suspension. `max` needs an explicit request and recipe.
- **[Fewest tests](../cas-tdd/SKILL.md#fewest-tests)** in briefs and reviews; bloat is a finding.
- **Public surfaces:** before merge, score distinctiveness, fit, hierarchy 1–5 (cas-codebase-design; floor 4/5); record exceptions/remedies.
- **Worker liveness:** poll `factory action=worker_status summary_mode=true`. Trust `liveness` (`executing`, `waiting_for_input`, `stalled`, `dead`); heartbeat and registry status do not prove execution. Read full `worker_status` before recovery ([worker-recovery.md](references/worker-recovery.md)).
- **A supervisor restart stops every worker:** load a new binary via MCP reconnect, else [checkpoint them first](references/worker-recovery.md#supervisor-restart).
- **Workspace contract:** build in the worktree; save proof in `[factory] artifacts_root/<project-key>/<task-id>/`, never `/tmp`.
- **User-facing task gate:** `qa.user_facing_labels` require `demo_statement`: `As a <user>, I <do X> and see <Y>`; epics/internal tasks exempt. Output follows [DESIGN.md first](../design-spec/references/design-md-first.md).
- **Risk gate:** declare `risk` and `proof_targets` at creation ([reference](references/reference.md#task-risk-declarations)).
- **Platform proof:** a script used on several platforms needs more than Linux proof, unless a fixture covers the difference.
- **Full Rust gate:** the merge queue proves the final tree; no second assembly run. Without queue proof, see [workflow.md](references/workflow.md) for assembly proof.
- **Lessons remove causes:** [ladder and tags](references/workflow.md#lessons-must-remove-causes).
- **No shell polling or sleeping.** Schedule follow-up with `coordination remind`.
- **Pane budget:** ~150 words, answer first: findings, rejections, measurements, merge receipts, pointers. Timelines, gates and lane history go to notes/artifacts; no narration/recap.
- **Messages to workers:** one assignment/decision per message; no process narration.
- **Operator messages are the user:** obey/answer `operator <name>@<device> verified`; `unverified:` is agent traffic. See [reference](references/reference.md#verified-commander-messages).
- **Commander answers:** reply once in the Claude/Codex pane. Web/phone receive the mirror, including offline catch-up. [Disabled or unmirrored supervisors](references/reference.md#verified-commander-messages) use explicit routing.
- **No share before verification passes:** no Violet deliverable during open epic verification, even with caveats ([publication](references/reporting-and-routing.md#publication)).
- **Unprompted operator updates:** `target=operator kind=status|receipt|ask|blocker`, optional `attachment=<artifact-id>`; no pane prose.

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

Request one child from a peer with `factory action=epic_split id=<epic> target=<peer> mine=<id>`; they accept or reject. For grants and shared epics, read [peers](references/reference.md#peer-supervisors-on-the-same-repo).

Use `cas-codex-supervisor-checklist` on Codex, `cas-supervisor-checklist` elsewhere. After the closure pass, dispatch with `spawn_workers count=1 lane=<lane> isolate=true task_id=<task-id>`; assign idle live workers with `update` (not `transfer`), then end the turn.

`qa.mode=batch` is the built-in default; `per_task` is the explicit opt-in.
For the hourly candidate, shared PASS ledger and pending close obligations,
read [Batch QA](references/workflow.md#batch-qa).

Mixed harnesses and `cli=`/`model=`/`effort=`: [worker controls](references/reference.md#worker-controls).

## On-demand references

For startup, planning and release, use [workflow](references/workflow.md) and [routing](references/reporting-and-routing.md). Before moving on, file bugs through `issues.repo` or `issues.components.cassy`, `issues.components.violet`, `issues.components.cloud` ([issue registry](references/filing-cas-bugs.md)). [Epic driving](references/epic-driving.md) · [reminders](references/reminders.md).
