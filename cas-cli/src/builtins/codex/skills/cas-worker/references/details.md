# Details — Tools, Sync, Schema

## Tool Selection Guide

Pick the right tool for the job:

| Need | Tool | Example |
|------|------|---------|
| Conceptual/exploratory query | `mcp__cs__search action=search` | "how does auth work?", "where is X handled?" |
| Exact symbol or string match | `Grep` | find all callers of `process_task()` |
| Complex codebase investigation | `Agent` with `subagent_type=Explore` | tracing a data flow across multiple modules |
| Record a learning or bugfix | `mcp__cs__memory action=remember` | root cause found, pattern discovered |
| Find files by name/pattern | `Glob` | `**/*.rs`, `src/**/mod.rs` |

See the `cas-search` skill for detailed search guidance including code symbol search and hybrid queries.

## Report / Evidence Tasks

Start with sources that cannot mutate the live Cassy DB:

- MCP task/search/coordination surfaces for task records, notes, already-surfaced messages, and searchable project context
- `.cas/logs/*.log` for daemon and lifecycle timelines
- Local git/worktree status and exported report artifacts already present in the repo or worktree

If those still do not answer the question:

1. Add a progress or decision note explaining why task/log artifacts were insufficient.
2. Prefer inspecting a copied snapshot of `.cas/cas.db`. Take it with `sqlite3 <db> ".backup <path>"` into `~/.cas/artifacts/<task-id>/`, never `cp` into `/tmp`: `$TMPDIR` is RAM-backed on the operator host, so a store copy, a worktree tree, or a build cache placed there evicts memory and takes every live session's shell output down with it when it fills.
3. If you must inspect the live DB, open it with a read-only SQLite URI such as `file:/abs/path/to/.cas/cas.db?mode=ro`. Do **not** use unrestricted `sqlite3 /path/to/.cas/cas.db` for routine report/evidence work.

## Syncing (Isolated Mode)

If the supervisor asks you to sync, checkpoint uncommitted work as a commit,
then rebase. Do not use `git stash`: the stash stack is shared by every
worktree of the repository, so a bare `pop` can restore another worker's
changes.

```bash
git add <paths> && git commit -m "WIP: <task-id> sync checkpoint"   # only if the tree is dirty
git rebase <branch>          # the branch name the supervisor gives you (e.g. master, epic/<slug>)
git reset --soft HEAD~1      # optional: turn the WIP commit back into staged changes
```

**Important:** Use the **local** branch name the supervisor specifies (e.g. `master`, `epic/<slug>`), NOT `origin/master`. In factory mode, the supervisor merges into the local branch directly, so `origin/master` is stale.

If the rebase has conflicts, resolve them and `git rebase --continue` before touching the WIP commit. Message the supervisor if you're stuck.

## Schema Cheat Sheet (exact field names and valid actions)

Wrong field names are rejected. These are the **exact** names for the calls workers make most often.

**`mcp__cs__task`** — the task ID field is always `id` (NOT `task_id`, `taskId`, `_id`). Notes parameter is `notes` (plural, NOT `note`).

```
# Start / show / close
mcp__cs__task action=start id=cas-abc1
mcp__cs__task action=show id=cas-abc1
mcp__cs__task action=close id=cas-abc1 reason="Implemented X, tests pass"

# Progress notes (note_type ∈ progress|blocker|decision|discovery|question|platform_proof)
mcp__cs__task action=notes id=cas-abc1 notes="Found root cause in Y" note_type=progress

# Mark blocked
mcp__cs__task action=update id=cas-abc1 status=blocked
mcp__cs__task action=notes id=cas-abc1 notes="Blocked: <reason>" note_type=blocker
```

`platform_proof` is the note the close gate reads for `risk=platform`.

**Need a database?** You cannot create a Neon branch or hold a Neon credential. Ask the supervisor: `mcp__cs__coordination action=message target=supervisor blocker=true summary="db branch for <task-id>" message="db branch for <task-id>: <why>"`. Its `db_branch_create` writes `DATABASE_URL` to `.env.cas-db` in your worktree. Source that file; never commit it or print it. The branch is deleted when your task closes.

**Priority** accepts numeric (0–4) OR named alias: `critical`/`high`/`medium`/`low`/`backlog`. `priority="high"` is the same as `priority=1`.

**Booleans** on `with_deps`, etc. accept `true`/`false`, `"true"`/`"false"`, or `1`/`0`.

**`mcp__cs__coordination action=message`** requires BOTH `message` and `summary`:

```
mcp__cs__coordination action=message target=supervisor \
  summary="task blocked on verification" \
  message="cas-abc1 needs schema review before I can proceed"
```

Sending `message` alone without `summary` is rejected. `summary` is the one-line preview shown in the UI.

Factory traffic is hard-capped: ordinary message bodies default to 1,200 characters, blocker/merge-request bodies to 2,500, and appended task notes to 1,500; put longer evidence in `[factory] artifacts_root/<task-id>/<name>.md` and send its path with a one-paragraph summary.

**Valid `mcp__cs__task` actions** (do not invent others): `create`, `proposal_inbox`, `proposal_accept`, `proposal_reject`, `proposal_reconcile`, `show`, `update`, `start`, `close`, `cancel`, `reopen`, `request_changes`, `delete`, `list`, `ready`, `blocked`, `notes`, `dep_add`, `dep_remove`, `dep_list`, `claim`, `release`, `reset`, `transfer`, `available`, `mine`.

`request_changes` and `reset` exist but are supervisor moves, not yours: `request_changes` is the sanctioned exit from `AwaitingMerge` when review fails (it reopens the task with the assignee preserved), and `reset` revives a task orphaned by a dead session (force-releases the lease, clears the assignee, forces `status=open`). Know them so you can read what happened to your task; don't run them on yourself.

**`ready` and `available` are read-only backlog visibility — not self-dispatch.** They exist for supervisors planning work and for you to sanity-check task state after an explicit assignment. Seeing a task there is never grounds to `start` it yourself; see "Never self-dispatch" in the main skill.

**`mcp__cs__coordination` actions workers routinely use**: `message`, `message_ack`, `message_status`, `inbox_poll`, `whoami`, `heartbeat`, `queue_poll`, `queue_ack`. Read-only diagnostics such as `gc_report`, `worker_status`, and `worktree_list` are also available to you.

Only `hold_worker` and `release_worker` are hard role-gated to supervisors (`only supervisors may change a worker's director hold state`). The rest of the factory/worktree surface — `spawn_workers`, `worktree_merge`, `gc_cleanup force=true` — is not blocked by a role check, which is exactly why you must not call it: those actions dispatch or destroy work across *every* worker on the host, and they are the supervisor's to run. Ask, don't invoke.

## Structured execution state

Use the task's compact structured execution state as the machine resume surface at
each meaningful milestone. Patch it in the same task update round-trip as any
ordinary task-field update when possible; `null` deletes a field. The schema is
bounded to `phase`, `receipts` (each `{command, exit_status}`), `files_touched`,
`decisions`, and `next_step`:

```
mcp__cs__task action=update id=<task-id> \
  state_patch='{"phase":"verify","receipts":[{"command":"npx vitest run","exit_status":0}],"files_touched":["src/app.ts"],"next_step":"push branch"}'
```

Read it first after a context clear with `action=start brief=true` or `action=show`.
Keep prose progress notes as the human/audit trail; structured state supplements
notes and never replaces them.

## Context budgeting

Three layers (`project_session_start_truncation.md`):

- **Immutable Core** — this skill body is the protected SessionStart guidance;
  keep it below 8 KB. The assembled payload has a 9 KB budget
  (`SESSION_START_BUDGET_BYTES`): degradable listings collapse to a heading and
  their reprint command when needed, while role guidance stays verbatim.
- **Task Context** — EPIC, task, and memories, loaded on demand.
- **Ephemeral** — command output and transcript; expendable.

Add guidance to the body only if every session needs it. Otherwise put it in a
reference such as this file.
