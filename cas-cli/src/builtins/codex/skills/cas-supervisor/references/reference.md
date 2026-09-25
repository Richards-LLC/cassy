# Reference — Action Names, Field Names, Dispatch Pattern

Wrong field names and invalid actions waste dispatch cycles. This section covers exact valid actions and field names.

**Valid `mcp__cs__task` actions** (do not invent others): `create`, `proposal_inbox`, `proposal_accept`, `proposal_reject`, `proposal_reconcile`, `show`, `update`, `start`, `close`, `cancel`, `reopen`, `request_changes`, `delete`, `list`, `ready`, `blocked`, `notes`, `dep_add`, `dep_remove`, `dep_list`, `claim`, `release`, `reset`, `transfer`, `available`, `mine`.

## Task risk declarations

Code tasks (`task`, `bug`, and `feature`) must carry `risk=blast-radius`,
`platform`, `concurrency`, or `none` at creation. A `blast-radius` declaration
also requires non-empty comma-separated `proof_targets`, which must cover every
source module in the attributed delivery diff. Supervisor overrides require a
non-empty audit reason and are recorded as a decision note. Workers never run
Rust builds or tests, so do not demand a scoped `--proof` receipt or a
`loaded_proof` note from them: the declared risk and `proof_targets` tell you
what your one assembly build + test of the epic tip must cover, and its
`ASSEMBLY_PROOF: head=<epic tip sha> result=PASS command=<cmd> log=<path>`
note on the epic is the proof child closes reference. A non-Rust
`risk=platform` task still carries its worker `platform_proof` receipt.

Two of those are supervisor-specific and easy to confuse:

- **`request_changes`** — the sanctioned exit from `awaiting_merge` whenever review fails: declined merge, amendment required after a merge landed, or work rejected outright. It reopens the task with its **assignee preserved**, so the same worker picks the rework back up. This is the rejection path — do not improvise one out of `update status=open`.
- **`reset`** — revive a task **orphaned by a dead session**. Atomic: force-releases the lease, clears the assignee, forces `status=open`. Because it clears the assignee it is the wrong tool for "this worker must redo it" — use `request_changes` for that. `reset` does not require you to hold the lease; add `force=true` only to override a still-heartbeating assignee (logged as a forced-reset audit note).

## Verified Commander messages

When an inbound Commander message is stamped `operator … verified` and includes `notification_id=N`, answer it with:

```
mcp__cs__coordination action=message target=operator in_reply_to=N summary="..." message="..."
```

The hub routes `operator` to the originating paired device and reports `queued for <device>` while offline; do not redirect this response to `supervisor`.
For phone-sized replies, follow the [phone reply contract](operator-reply.md).

## Supervisor override

`supervisor_override=true` is the documented override for supervisor-only close and transfer operations. It is accepted only when the caller is a **registered supervisor**, the request supplies a **non-empty reason**, and the accepted decision is recorded as a **task decision note**. Review the task state and delivery evidence first; this flag does not waive data-integrity or merge-state checks.

**Valid `mcp__cs__coordination` actions** (agent identity, messaging, reminders; an unknown action is rejected with the current list): `register`, `unregister`, `whoami`, `heartbeat`, `session_start`, `session_end`, `inbox_poll` (alias `inbox`), `message`, `interrupt`, `message_ack`, `message_status`, `remind`, `remind_list`, `remind_cancel`, `my_context`.

**Valid `mcp__cs__factory` actions** (supervisor fleet control; `coordination` still accepts these for one release with a deprecation note):
- *Fleet*: `spawn_workers`, `shutdown_workers`, `recycle_worker`, `restart_spawn_queue`, `hold_worker`, `release_worker`, `worker_status`, `worker_activity`, `sweep_tasks`, `clear_context`, `sync_all_workers`, `gc_report`, `gc_cleanup`, `epic_status`, `focus_epic`, `agent_list`, `agent_cleanup`, `lease_history`
- *Servers*: `server_start`, `server_stop`, `server_list`
- *Database branches (supervisor only)*: `db_branch_create`, `db_branch_show`, `db_branch_delete`
- *Worktree*: `worktree_create`, `worktree_list`, `worktree_show`, `worktree_cleanup`, `worktree_merge`, `worktree_status`
- *Loops and queues*: `loop_start`, `loop_cancel`, `loop_status`, `queue_notify`, `queue_poll`, `queue_peek`, `queue_ack`

**`hold_worker` / `release_worker` — pause a worker without faking a task state.** `action=hold_worker target=<worker>` marks a worker as deliberately paused: the Director stops accumulating idle ticks for them and emits no `WorkerIdle` nudges until you `release_worker`. Use it for "stand by while I sort out the merge base" instead of parking the task in a misleading status. Supervisor-only, requires a live worker in your factory session; the hold survives a daemon restart of that session and clears on worker removal or session shutdown.

**`sweep_tasks` — fan out an integration failure report.** Omit `accept` to preview `.cas/merge-sweeps/sweep-tasks.json`, which contains one class with its failing tests, targets, assertion text, log path, and suggested lane. A live supervisor may pass `accept=true` to create one local bug task and queue one isolated, task-preassigned worker per class. Acceptance is idempotent: retrying resumes classes whose task or spawn receipt is missing without duplicating completed classes.

**`server_start` / `server_stop` / `server_list` — the sanctioned way to run a long-lived server.** A raw `npm run dev &` from a worker dies with the worker and leaves no record of what is listening. Register it instead:

```
mcp__cs__factory action=server_start command="npm run dev" cwd=<path> port=3000 shared=true
mcp__cs__factory action=server_list
mcp__cs__factory action=server_stop ...
```

`shared=true` places the server outside worker containment so it outlives worker teardown; the default (`false`) ties its lifetime to the worker that started it. `port` is advisory — `server_list` reports the ports actually bound, plus who started each server. stdout/stderr are captured to a log file, never inherited.

**`db_branch_create` / `db_branch_show` / `db_branch_delete` — a disposable database for one task (supervisor only).** When a worker needs a database to reproduce a bug, it asks with a blocker message; it cannot create a Neon branch itself and never sees a credential. Provision one:

```
mcp__cs__factory action=db_branch_create task_id=<task> [branch=<dev|staging|branch id>] [target=<worker>]
mcp__cs__factory action=db_branch_show [task_id=<task>]
mcp__cs__factory action=db_branch_delete task_id=<task> [id=<branch id>]
```

Your `cas serve` creates `cas-<task-id>-<n>` through the proxy's `neon.*` tools. The project and parent come from the repository's `neon-database` skill file: `dev` by default, else `staging`, and a production parent is always refused. It writes `DATABASE_URL` to `.env.cas-db` in the worker's worktree (mode 600, git-excluded) and records the branch in `.cas/db-branches/<task>.json` and a task note. The connection string is never shown. Closing or cancelling the task deletes the branch; a worker's own close queues the deletion, and your next coordination call performs it. A task has at most 3 branches, each with a 72-hour TTL and a small compute ceiling, and `gc_report` flags any that outlive their task, worktree or TTL.

**`spawn_workers` parameters:**

| Parameter | Type | Description |
|---|---|---|
| `count` | int | Number of workers to spawn |
| `lane` | string | Registry lane to resolve: `light`, `standard`, `taste`, or `heavy`. The registry picks the recipe and reports any fallback in the receipt. Never combine with `cli`, `model`, or `effort`. |
| `isolate` | bool | Each worker gets its own git worktree and branch (default false; pass `true` — shared mode is contamination-prone and every receipt warns) |
| `worker_names` | string | Comma-separated names for the spawned workers |
| `cli` | string | Explicit CLI backend for this spawn: `claude`, `codex`, `grok`, or `opencode` (OpenCode routes are receipt-gated; see [model-selection.md](model-selection.md#opencode-lane-route-specific-conformance)). If omitted, resolves through factory config, then stock fallback. |
| `model` | string | Explicit model name. Accepted slugs per `cli` and the lane matrix live in [model-selection.md](model-selection.md#model-slug-table). Passed as `-m`/`--model`. If omitted, resolves through factory config, then the selected harness's stock default. |
| `effort` | string | Explicit reasoning effort. Cassy vocabulary: `minimal` \| `low` \| `medium` \| `high` \| `xhigh` (alias `x-high`) \| `max` (only where the registry recipe lists it: Fable, Opus 5, Astra, Sol, GPT-6 Luna — not Opus 5.5; never a default). Mapping: Claude `--effort`; Codex `--config model_reasoning_effort=<v>`; Grok `--reasoning-effort`; OpenCode generated primary-agent `variant` (QwenCloud Token Plan and Alibaba PAYG: `low`, `medium`, `xhigh`). Token Plan pins OpenAI-compatible `extra_body.enable_thinking`; PAYG uses `reasoning_effort`. If omitted, resolves through factory config, then stock fallback. For multi-step Claude workers prefer `high` as the ceiling — see [model-selection.md](model-selection.md). |
| `task_id` | string | Pre-assign this task to the spawned worker. **Single-worker requests only** (`count=1`) — a multi-worker spawn is rejected. An open, unassigned `task_id` also *authorizes* the spawn on its own, so a post-epic follow-up needs no ceremonial single-child epic. Refused when the task is closed, already assigned, blocked/awaiting_merge, or when a spawn for that task is already queued and unconsumed. |
| `config_dir` | string | Account directory for the spawned workers: `CLAUDE_CONFIG_DIR` for Claude (e.g. `~/.claude-alt`), `CODEX_HOME` for Codex. An explicit value wins and is preflighted before queueing; otherwise the requesting supervisor's own directory for that harness is captured **at enqueue time**. Grok has no account directory, so the acknowledgement warns. An explicit Claude value also strips inherited `ANTHROPIC_API_KEY` so the selected OAuth account is actually used. |

Pass `lane=` or a complete `cli=`/`model=`/`effort=` recipe on every `spawn_workers` call, never both; these controls apply to the workers spawned by this call only. Omitted controls resolve through the config cascade and the acknowledgement warns. Copy-paste recipes: [workflow.md](workflow.md#phase-2-coordinate).

**On `mcp__cs__task`, the task ID is always `id`** — not `task_id`, `taskId`, or `_id`. The exceptions are coordination actions that reference a task belonging to *another* object: `spawn_workers task_id=`, `worktree_merge task_id=`, and `worktree_create task_id=` all take `task_id` (their `id` means worker/worktree). Rule of thumb: `id` names the thing the action operates on; `task_id` names a task the action merely points at.

**Priority** is `0=Critical, 1=High, 2=Medium (default), 3=Low, 4=Backlog`. Accepts numeric OR named alias: `priority=1` ≡ `priority="high"`. Other aliases: `critical`, `medium`, `low`, `backlog`, `p0`-`p4`.

**Initial assignment uses `update`, NOT `transfer`:**

```
# CORRECT — initial assignment of an unclaimed task
mcp__cs__task action=update id=cas-abc1 assignee=<worker-name>

# WRONG — transfer requires an ALREADY-CLAIMED lease, otherwise errors
# with "No active lease found". Use transfer only to reassign between
# workers after one has claimed.
mcp__cs__task action=transfer id=cas-abc1 to_agent=<worker>
```

The `transfer` action's target field is `to_agent` (not `assignee`). The `update` action's target field is `assignee` (not `to_agent`). Yes, they disagree. Remember: `update assignee=...` for initial assignment; `transfer to_agent=...` only when reassigning a claimed task.

**Reassigning a task owned by a live worker:**

When a task is claimed by a live worker and you need to reassign it without shutting the worker down, use `supervisor_override=true` on `transfer` as described in [Supervisor override](#supervisor-override):

```
# Force-transfer from a live worker to another agent (single atomic step)
mcp__cs__task action=transfer id=cas-abc1 to_agent=<new-worker> supervisor_override=true \
  notes="Reassigned due to <reason>"
```

This force-releases the live worker's lease, updates the assignee, attempts to pre-claim for the target agent, and appends an audit entry to the task notes with your supervisor session ID and the prior lease holder. The old worker loses its lease silently — message them separately if they need to know.

Two-step alternative (if the atomic path errors):

```
# Step 1: Drop the live lease and reset the task to Open
mcp__cs__task action=reset id=cas-abc1

# Step 2: Assign to the new worker
mcp__cs__task action=update id=cas-abc1 assignee=<new-worker>

# Step 3: Notify the new worker
mcp__cs__coordination action=message target=<new-worker> summary="..." message="..."
```

`reset` does NOT require you to own the lease — it is safe to call on any non-closed task regardless of who holds the current lease.

**Dispatching tasks is a two-step operation.** Sending a coordination message telling a worker to "claim tasks X and Y" does not actually dispatch work — workers react to `assignee` changes on the task, not to message content. Full pattern:

```
# 1. Create
mcp__cs__task action=create title="Fix login bug" priority=high risk=none \
  description="..." acceptance_criteria="..."

# 2. Assign (this is what causes the worker to pick it up)
mcp__cs__task action=update id=cas-abc1 assignee=<worker>

# 3. (optional) Provide extra context as a separate message
mcp__cs__coordination action=message target=<worker> \
  summary="cas-abc1 briefing" \
  message="Extra context for cas-abc1: ..."
```

Skipping step 2 leaves the task unassigned — the worker will go idle regardless of how clear the message in step 3 was.

**Coordination messages require BOTH `message` and `summary`:**

```
mcp__cs__coordination action=message target=worker-1 \
  summary="c29a ready for review" \
  message="Please verify cas-c29a. Commit dfe824b on main."
```

Missing either field is a rejection. `summary` is the one-line UI preview; `message` is the full body.

Factory traffic is hard-capped: ordinary message bodies default to 1,200 characters, blocker/merge-request bodies to 2,500, and appended task notes to 1,500; put longer evidence in `[factory] artifacts_root/<task-id>/<name>.md` and send its path with a one-paragraph summary. Your own over-cap message to a worker is not refused: Cassy writes the full text to `artifacts_root/<task-id or _messages>/message-<time>-<hash>.md` and delivers its head with that path.

**Urgent / interrupt delivery — course-correct a worker mid-turn (cas-c931):**

Normal messages land only *between* turns: a worker that is mid-turn going down the wrong path finishes the wrong turn before it ever reads "stop, do X instead." For those cases, send an **urgent** message — it breaks the worker's in-flight turn and injects your correction as its next prompt:

```
# Urgent flag on the normal message action
mcp__cs__coordination action=message target=<worker> urgent=true \
  summary="..." message="Stop — you're editing the wrong file. Switch to ..."

# Shorthand — forces urgent even without the flag
mcp__cs__coordination action=interrupt target=<worker> \
  summary="..." message="Stop — wrong approach. Do ... instead."
```

When urgent, the message: breaks the target's in-flight turn (Esc), waits a bounded settle window, then injects the correction as its next prompt; bypasses the Claude Code inbox even in agent-teams mode; forces Critical priority (queue jump) when none is given; skips idle-message dedup; targets the worker **by name**, independent of TUI focus.

**Caveat — urgent DISCARDS the worker's in-flight reasoning / partial work.** Use it ONLY when the worker is demonstrably off the rails (wrong file, wrong approach, ignoring the ticket). For routine nudges or FYIs, use a normal `action=message` (non-disruptive, lands between turns).

**Task notes** parameter is `notes` (plural), not `note`:

```
mcp__cs__task action=notes id=cas-abc1 notes="Progress update" note_type=progress
```

**Booleans** accept native bool, string `"true"`/`"false"`, or numeric `1`/`0`.

## Context budgeting

`project_session_start_truncation.md`: **Immutable Core** (the cas-supervisor SKILL.md body, 8 KB cap), **Task Context** (on demand), and **Ephemeral** output. Details go in `references/`.
