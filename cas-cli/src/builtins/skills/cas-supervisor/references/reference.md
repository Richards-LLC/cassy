# Reference — Action Names, Field Names, Dispatch Pattern

Wrong field names and invalid actions waste dispatch cycles. This section covers exact valid actions and field names.

**Valid `mcp__cas__task` actions** (do not invent others): `create`, `proposal_inbox`, `proposal_accept`, `proposal_reject`, `proposal_reconcile`, `show`, `update`, `start`, `close`, `cancel`, `reopen`, `request_changes`, `delete`, `list`, `ready`, `blocked`, `notes`, `dep_add`, `dep_remove`, `dep_list`, `claim`, `release`, `reset`, `transfer`, `available`, `mine`.

Two of those are supervisor-specific and easy to confuse:

- **`request_changes`** — the sanctioned exit from `awaiting_merge` whenever review fails: declined merge, amendment required after a merge landed, or work rejected outright. It reopens the task with its **assignee preserved**, so the same worker picks the rework back up. This is the rejection path — do not improvise one out of `update status=open`.
- **`reset`** — revive a task **orphaned by a dead session**. Atomic: force-releases the lease, clears the assignee, forces `status=open`. Because it clears the assignee it is the wrong tool for "this worker must redo it" — use `request_changes` for that. `reset` does not require you to hold the lease; add `force=true` only to override a still-heartbeating assignee (logged as a forced-reset audit note).

## Supervisor override

`supervisor_override=true` is the documented override for supervisor-only close and transfer operations. It is accepted only when the caller is a **registered supervisor**, the request supplies a **non-empty reason**, and the accepted decision is recorded as a **task decision note**. Review the task state and delivery evidence first; this flag does not waive data-integrity or merge-state checks.

**Valid `mcp__cas__coordination` actions** (do not invent others):
- *Agent*: `register`, `unregister`, `whoami`, `heartbeat`, `agent_list`, `agent_cleanup`, `session_start`, `session_end`, `loop_start`, `loop_cancel`, `loop_status`, `lease_history`, `queue_notify`, `queue_poll`, `queue_peek`, `queue_ack`, `inbox_poll`, `message`, `interrupt`, `message_ack`, `message_status`
- *Factory*: `spawn_workers`, `shutdown_workers`, `hold_worker`, `release_worker`, `worker_status`, `worker_activity`, `clear_context`, `my_context`, `sync_all_workers`, `gc_report`, `gc_cleanup`, `epic_status`, `focus_epic`, `remind`, `remind_list`, `remind_cancel`, `server_start`, `server_stop`, `server_list`
- *Worktree*: `worktree_create`, `worktree_list`, `worktree_show`, `worktree_cleanup`, `worktree_merge`, `worktree_status`

**`hold_worker` / `release_worker` — pause a worker without faking a task state.** `action=hold_worker target=<worker>` marks a worker as deliberately paused: the Director stops accumulating idle ticks for them and emits no `WorkerIdle` nudges until you `release_worker`. Use it for "stand by while I sort out the merge base" instead of parking the task in a misleading status. Supervisor-only, requires a live worker in your factory session; the hold survives a daemon restart of that session and clears on worker removal or session shutdown.

**`server_start` / `server_stop` / `server_list` — the sanctioned way to run a long-lived server.** A raw `npm run dev &` from a worker dies with the worker and leaves no record of what is listening. Register it instead:

```
mcp__cas__coordination action=server_start command="npm run dev" cwd=<path> port=3000 shared=true
mcp__cas__coordination action=server_list
mcp__cas__coordination action=server_stop ...
```

`shared=true` places the server outside worker containment so it outlives worker teardown; the default (`false`) ties its lifetime to the worker that started it. `port` is advisory — `server_list` reports the ports actually bound, plus who started each server. stdout/stderr are captured to a log file, never inherited.

**`spawn_workers` parameters:**

| Parameter | Type | Description |
|---|---|---|
| `count` | int | Number of workers to spawn |
| `isolate` | bool | Each worker gets its own git worktree and branch (default false) |
| `worker_names` | string | Comma-separated names for the spawned workers |
| `cli` | string | Explicit CLI backend for this spawn: `claude`, `codex`, `grok`, or `opencode`. OpenCode's QwenCloud Token Plan route is validated by receipt `opencode-1.18.23-hosted-token-plan-2026-08-27`; local and Alibaba PAYG routes remain pending-conformance and are refused before queue insertion. If omitted, resolves through factory config, then stock fallback. |
| `model` | string | Explicit model name. Registry routes are Claude `claude-haiku-4-5-20251001`/low for light, Codex `gpt-5.6-luna`/xhigh for standard, Claude `claude-fable-5-1`/medium for taste, and Codex `gpt-5.6-sol`/high for heavy. Terra is standing-suspended; never spawn it. Luna must not use lower effort. Grok models are `grok-4.5` and `grok-4.6`, but provider capacity is not an active registry lane. Claude's stock fallback remains the verified `opus` alias. OpenCode defaults explicitly to `qwencloud/qwen3.8-max` on the receipt-gated Token Plan route; `alibaba/qwen3.8-max` and `alibaba-cn/qwen3.8-max` select PAYG explicitly. Passed as `-m`/`--model`. If omitted, resolves through factory config, then the selected harness's stock default. |
| `effort` | string | Explicit reasoning effort. Cassy vocabulary: `minimal` \| `low` \| `medium` \| `high` \| `xhigh` (alias `x-high`). Mapping: Claude `--effort`; Codex `--config model_reasoning_effort=<v>`; Grok `--reasoning-effort`; OpenCode generated primary-agent `variant` (QwenCloud Token Plan and Alibaba PAYG: `low`, `medium`, `xhigh`). Token Plan pins OpenAI-compatible `extra_body.enable_thinking`; PAYG uses `reasoning_effort`. If omitted, resolves through factory config, then stock fallback. For multi-step Claude workers prefer `high` as the ceiling — see [model-selection.md](model-selection.md). |
| `task_id` | string | Pre-assign this task to the spawned worker. **Single-worker requests only** (`count=1`) — a multi-worker spawn is rejected. An open, unassigned `task_id` also *authorizes* the spawn on its own, so a post-epic follow-up needs no ceremonial single-child epic. Refused when the task is closed, already assigned, blocked/awaiting_merge, or when a spawn for that task is already queued and unconsumed. |
| `config_dir` | string | Claude account directory for the spawned workers (e.g. `~/.claude-alt`). **Claude-only** — Codex/Grok workers ignore it and the acknowledgement carries a warning. Resolution: an explicit `config_dir` wins; otherwise the requesting supervisor's `CLAUDE_CONFIG_DIR` is captured **at enqueue time** (the daemon may consume the queue row under a different environment). An explicit value also strips inherited `ANTHROPIC_API_KEY` so the selected OAuth account is actually used. |

`cli`, `model`, and `effort` are per-spawn controls — they apply to the workers spawned by this call only. Supervisors MUST pass explicit `cli=`, `model=`, and `effort=` on every `spawn_workers` call; omitted fields resolve through the config cascade as a fallback and produce an acknowledgement warning. Copy-paste recipes for all four backends: [workflow.md](workflow.md#phase-2-coordinate).

For OpenCode Token Plan fan-out, honor the operator-declared concurrency tier: Lite 1–2 agents, Standard 3–4, or Pro 6–8. Warn or cap requests beyond that tier; do not scrape the operator console.

**On `mcp__cas__task`, the task ID is always `id`** — not `task_id`, `taskId`, or `_id`. The exceptions are coordination actions that reference a task belonging to *another* object: `spawn_workers task_id=`, `worktree_merge task_id=`, and `worktree_create task_id=` all take `task_id` (their `id` means worker/worktree). Rule of thumb: `id` names the thing the action operates on; `task_id` names a task the action merely points at.

**Priority** is `0=Critical, 1=High, 2=Medium (default), 3=Low, 4=Backlog`. Accepts numeric OR named alias: `priority=1` ≡ `priority="high"`. Other aliases: `critical`, `medium`, `low`, `backlog`, `p0`-`p4`.

**Initial assignment uses `update`, NOT `transfer`:**

```
# CORRECT — initial assignment of an unclaimed task
mcp__cas__task action=update id=cas-abc1 assignee=<worker-name>

# WRONG — transfer requires an ALREADY-CLAIMED lease, otherwise errors
# with "No active lease found". Use transfer only to reassign between
# workers after one has claimed.
mcp__cas__task action=transfer id=cas-abc1 to_agent=<worker>
```

The `transfer` action's target field is `to_agent` (not `assignee`). The `update` action's target field is `assignee` (not `to_agent`). Yes, they disagree. Remember: `update assignee=...` for initial assignment; `transfer to_agent=...` only when reassigning a claimed task.

**Reassigning a task owned by a live worker:**

When a task is claimed by a live worker and you need to reassign it without shutting the worker down, use `supervisor_override=true` on `transfer` as described in [Supervisor override](#supervisor-override):

```
# Force-transfer from a live worker to another agent (single atomic step)
mcp__cas__task action=transfer id=cas-abc1 to_agent=<new-worker> supervisor_override=true \
  notes="Reassigned due to <reason>"
```

This force-releases the live worker's lease, updates the assignee, attempts to pre-claim for the target agent, and appends an audit entry to the task notes with your supervisor session ID and the prior lease holder. The old worker loses its lease silently — message them separately if they need to know.

Two-step alternative (if the atomic path errors):

```
# Step 1: Drop the live lease and reset the task to Open
mcp__cas__task action=reset id=cas-abc1

# Step 2: Assign to the new worker
mcp__cas__task action=update id=cas-abc1 assignee=<new-worker>

# Step 3: Notify the new worker
mcp__cas__coordination action=message target=<new-worker> summary="..." message="..."
```

`reset` does NOT require you to own the lease — it is safe to call on any non-closed task regardless of who holds the current lease.

**Dispatching tasks is a two-step operation.** Sending a coordination message telling a worker to "claim tasks X and Y" does not actually dispatch work — workers react to `assignee` changes on the task, not to message content. Full pattern:

```
# 1. Create
mcp__cas__task action=create title="Fix login bug" priority=high \
  description="..." acceptance_criteria="..."

# 2. Assign (this is what causes the worker to pick it up)
mcp__cas__task action=update id=cas-abc1 assignee=<worker>

# 3. (optional) Provide extra context as a separate message
mcp__cas__coordination action=message target=<worker> \
  summary="cas-abc1 briefing" \
  message="Extra context for cas-abc1: ..."
```

Skipping step 2 leaves the task unassigned — the worker will go idle regardless of how clear the message in step 3 was.

**Coordination messages require BOTH `message` and `summary`:**

```
mcp__cas__coordination action=message target=worker-1 \
  summary="c29a ready for review" \
  message="Please verify cas-c29a. Commit dfe824b on main."
```

Missing either field is a rejection. `summary` is the one-line UI preview; `message` is the full body.

**Urgent / interrupt delivery — course-correct a worker mid-turn (cas-c931):**

Normal messages land only *between* turns: a worker that is mid-turn going down the wrong path finishes the wrong turn before it ever reads "stop, do X instead." For those cases, send an **urgent** message — it breaks the worker's in-flight turn and injects your correction as its next prompt:

```
# Urgent flag on the normal message action
mcp__cas__coordination action=message target=<worker> urgent=true \
  summary="..." message="Stop — you're editing the wrong file. Switch to ..."

# Shorthand — forces urgent even without the flag
mcp__cas__coordination action=interrupt target=<worker> \
  summary="..." message="Stop — wrong approach. Do ... instead."
```

When urgent, the message: breaks the target's in-flight turn (Esc), waits a bounded settle window, then injects the correction as its next prompt; bypasses the Claude Code inbox even in agent-teams mode; forces Critical priority (queue jump) when none is given; skips idle-message dedup; targets the worker **by name**, independent of TUI focus.

**Caveat — urgent DISCARDS the worker's in-flight reasoning / partial work.** Use it ONLY when the worker is demonstrably off the rails (wrong file, wrong approach, ignoring the ticket). For routine nudges or FYIs, use a normal `action=message` (non-disruptive, lands between turns).

**Task notes** parameter is `notes` (plural), not `note`:

```
mcp__cas__task action=notes id=cas-abc1 notes="Progress update" note_type=progress
```

**Booleans** accept native bool, string `"true"`/`"false"`, or numeric `1`/`0`.
