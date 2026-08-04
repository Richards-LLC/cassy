# Reference — Action Names, Field Names, Dispatch Pattern

Wrong field names and invalid actions waste dispatch cycles. This section covers exact valid actions and field names.

**Valid `mcp__cas__task` actions** (do not invent others): `create`, `show`, `update`, `start`, `close`, `reopen`, `delete`, `list`, `ready`, `blocked`, `notes`, `dep_add`, `dep_remove`, `dep_list`, `claim`, `release`, `transfer`, `available`, `mine`.

**Valid `mcp__cas__coordination` actions** (do not invent others):
- *Agent*: `register`, `unregister`, `whoami`, `heartbeat`, `agent_list`, `agent_cleanup`, `session_start`, `session_end`, `loop_start`, `loop_cancel`, `loop_status`, `lease_history`, `queue_notify`, `queue_poll`, `queue_peek`, `queue_ack`, `inbox_poll`, `message`, `message_ack`, `message_status`
- *Factory*: `spawn_workers`, `shutdown_workers`, `worker_status`, `worker_activity`, `clear_context`, `my_context`, `sync_all_workers`, `gc_report`, `gc_cleanup`, `epic_status`, `focus_epic`, `remind`, `remind_list`, `remind_cancel`
- *Worktree*: `worktree_create`, `worktree_list`, `worktree_show`, `worktree_cleanup`, `worktree_merge`, `worktree_status`

**`spawn_workers` parameters:**

| Parameter | Type | Description |
|---|---|---|
| `count` | int | Number of workers to spawn |
| `isolate` | bool | Each worker gets its own git worktree and branch (default false) |
| `worker_names` | string | Comma-separated names for the spawned workers |
| `cli` | string | Explicit CLI backend for this spawn: `claude`, `codex`, or `grok`. If omitted, resolves through factory config, then stock fallback. |
| `model` | string | Explicit model name. Codex uses `gpt-5.6-terra` for light/standard/taste work and `gpt-5.6-sol` for heavy/frontier work (no `-codex` suffix; bare `gpt-5.6` is invalid). Claude: `opus` for exceptional architecture/safety/rescue/challenge; Sonnet is not a normal worker lane. Grok: `grok-4.5` or `grok-composer-2.5-fast` (from `grok models`). Passed as `-m`/`--model`. If omitted, resolves through factory config, then backend stock fallback. |
| `effort` | string | Explicit reasoning effort. CAS vocabulary: `minimal` \| `low` \| `medium` \| `high` \| `xhigh` (alias `x-high`). Mapping: Claude `--effort`; Codex `--config model_reasoning_effort=<v>`; Grok `--reasoning-effort`. If omitted, resolves through factory config, then stock fallback. For multi-step Claude workers prefer `high` as the ceiling — see [model-selection.md](model-selection.md). |

`cli`, `model`, and `effort` are per-spawn controls — they apply to the workers spawned by this call only. Supervisors MUST pass explicit `cli=`, `model=`, and `effort=` on every `spawn_workers` call; omitted fields resolve through the config cascade as a fallback and produce an acknowledgement warning. Copy-paste recipes for all three backends: [model-selection.md](model-selection.md#spawn-cookbook-all-three-harnesses).

**Task ID is always `id`** — not `task_id`, `taskId`, or `_id`.

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

**Reassigning a task owned by a live worker (supervisor override):**

When a task is claimed by a live worker and you need to reassign it without shutting the worker down, use `bypass_code_review=true` as the supervisor-override flag on `transfer`:

```
# Force-transfer from a live worker to another agent (single atomic step)
mcp__cas__task action=transfer id=cas-abc1 to_agent=<new-worker> bypass_code_review=true \
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
