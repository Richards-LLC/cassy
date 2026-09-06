# Workflow — Worker Modes, Phases, Blockers

## Worker Modes

Workers can run in two modes:

- **Isolated** (`isolate=true`): Each worker gets its own git worktree and branch. Use when workers will modify overlapping files or when you need clean branch-based merging.
- **Shared** (`isolate=false` or omitted): Workers share the main working directory. Simpler setup, but workers must coordinate to avoid editing the same files simultaneously.

## Worker Count Strategy

Spawn workers based on independent file groups, not task count.

1. Map which files each task will modify
2. Group tasks touching the same files into one lane (prevents conflicts)
3. Workers needed = number of parallel lanes

```
# 8 tasks, but only 2 independent file groups → 2 workers, not 8
workers = min(tasks_without_file_overlap, tasks_at_same_dependency_level)
```

In shared mode, file-overlap analysis is even more critical — two workers editing the same file simultaneously will cause problems.

## Phase 1: Plan

1. Search before planning — check all three sources for prior art:
   ```
   # Similar past EPICs (patterns, sizing, what worked)
   mcp__cs__task action=list task_type=epic status=closed

   # Cassy memories for learnings, bugfixes, architectural decisions
   mcp__cs__search action=search query="<keywords>" doc_type=entry limit=10

   # Codebase for existing implementations you might duplicate or conflict with
   Grep pattern="<feature-name>" or mcp__cs__search action=search query="<keywords>" scope=code
   ```
2. Create EPIC: `mcp__cs__task action=create task_type=epic title="..." description="..."`
3. Gather the EPIC specification and task breakdown through the supervisor's task/spec workflow.
4. Review task scope and dependencies

**Standalone follow-up work (no EPIC needed).** An EPIC is for a body of work broken into
tasks. When an epic has closed and one loose task turns up — a follow-up, a late bug, a
one-off — do NOT create a single-child epic to satisfy the spawn gate. Create the task and
spawn straight onto it:

```
mcp__cs__task action=create title="..." description="..."
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-luna effort=xhigh task_id=<task-id>
```

A concrete open, unassigned `task_id` authorizes the spawn on its own. It authorizes exactly
one worker: a second spawn for the same still-queued task is refused, as is a task that is
blocked, awaiting_merge, or already assigned to another worker — those state
that no new worker can pick the task up. Ceremonial single-child epics distort epic reporting
and the "all subtasks closed -> verify and close the epic" flow, so this is the preferred path.

## Phase 2: Coordinate

1. Spawn workers:
   ```
   mcp__cs__coordination action=spawn_workers count=N isolate=true cli=codex model=gpt-5.6-luna effort=xhigh
   ```
   Omit `isolate` for shared mode.

   **Hard rule:** every `spawn_workers` call MUST include explicit `cli=`,
   `model=`, and `effort=`. The active registry matrix is Claude Haiku 4.5/low for light, Codex GPT-5.6 Luna/xhigh for standard, Claude Fable 5.1/medium for taste (Claude Opus 5/high fallback), and Codex GPT-6 Astra/high for heavy (Codex GPT-5.6 Sol/high fallback). Use taste for judgment and public decisions and heavy for implementation risk; Terra is a standing suspension.
   Omitted fields fall back through the factory config cascade and stock floor;
   the spawn acknowledgement nags because supervisors should make worker tier
   selection intentional and visible.

   **Tiered mix example** — use the active registry lanes below; each command carries explicit controls:

<!-- BEGIN GENERATED SPAWN RECIPES: cas-factory lane registry -->
Copy-paste commands generated from the registry; every recipe pins `cli`, `model`, and `effort`:

```text
# light — recipe claude_haiku
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=claude model=claude-haiku-4-5-20251001 effort=low

# standard — recipe codex_luna
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-luna effort=xhigh

# taste — recipe claude_fable (fallback: claude_opus)
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=claude model=claude-fable-5-1 effort=medium

# heavy — recipe codex_astra_high (fallback: codex_sol)
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-6-astra effort=high

# supervisor — recipe claude_fable (fallback: claude_opus)
mcp__cs__coordination action=spawn_workers count=1 isolate=true cli=claude model=claude-fable-5-1 effort=medium

```
<!-- END GENERATED SPAWN RECIPES -->
   `cli`, `model`, and `effort` are per-spawn controls for the workers spawned
   by that call.
   Spawn the tier mix the ready backlog needs — one `spawn_workers` call per tier; rubric
   and routing in [model-selection.md](model-selection.md).
   Full parameter table in [reference.md](reference.md#spawn_workers-parameters).
2. Verify workers appear in TUI before assigning (stale DB records are not real workers)
3. Assign tasks: `mcp__cs__task action=update id=<id> assignee=<worker>`
4. Pin epic focus so the TUI shows it immediately: `mcp__cs__coordination action=focus_epic id=<epic-id>`. Without this, the TASKS/FACTORY panels stay empty until a worker's first `task action=start` on a subtask lets the panel infer the epic — and inference only fires once that subtask's `assignee` matches a live session agent (workers now get this for free: `task action=start` sets `assignee` automatically when unset, cas-6945). Clear with `action=focus_epic clear=true` when the epic wraps.
5. Search for relevant context and send assignment message:
   ```
   mcp__cs__coordination action=message target=<worker> \
     summary="Task <id> assignment" \
     message="Task <id>: <description>. Context: <findings>. Run mcp__cs__task action=mine to see your tasks."
   ```
6. **Own the next exit rung.** If a worker owns it, wait for that worker's injected event. If you own a time-based follow-up, schedule one `coordination remind` that names the exact check and when it fires. Do not spin-poll.

### Resuming an Existing EPIC

Workers from previous sessions are gone. Stale DB records are not live processes.

1. **Check for binary/source drift** — fixes merged to main since last session don't take effect until rebuild. Run `~/.cargo/bin/cargo build --release` if Cassy source changed, then restart `cas serve`. If a "fixed" bug reappears, this is the first thing to check.
2. Spawn fresh workers
3. Verify they appear in TUI
4. Assign open tasks to the new workers

## Phase 3: Merge and Sync (Isolated Mode)

When workers have isolated worktrees, merge their work into the epic branch after each completion, then tell other workers to sync.

```
base branch ────────────────────► (stays clean)
          \                    /
           └─ epic/feature ───►
              \          \     /
               ├─ factory/fox ┤
               └─ factory/owl ┘
```

### Merge workers with Cassy

`mcp__cs__coordination action=worktree_merge` is the worker merge path. It resolves
the merge target from task state, enforces the trunk guard, and keeps factory tracking,
leases, and cleanup consistent.

Run the canonical merge-time diff review in Phase 3 after a successful merge and before assigning the next task.

```
mcp__cs__coordination action=worktree_merge id=<worker> task_id=<task-id>
```

`id` accepts the worker name or `factory/<worker>`. Target resolution: an explicit
`task_id` first, then the assignee's current task binding. A `focus_epic` pin is a
**display filter and never merge authority**, and Cassy never silently defaults to
`main`/`master`/`staging`.

### Required merge-review discipline

Before accepting a scoped worker receipt and landing its lane, do these two checks:

1. **Contract changes first.** If the diff changes a public contract (API shape,
   persisted field, CLI/MCP response, or behavior callers rely on), search for sibling
   tests that still pin the old contract before accepting the scoped receipt. For example:
   `git grep -n '<old contract token>' -- '*test*'` (narrow the path/spec as needed).
   Update or reject the receipt when those tests prove an unreviewed caller contract.
2. **Read the lane CI signal.** Inspect `gh run list --branch factory/<worker>` at
   review time. `worktree_merge` also reports its best-effort CI workflow verdict, but
   this explicit review check catches a new run or a result that arrived after the
   merge command's lookup. A red or unknown result is a review signal, not a v1 merge
   refusal: investigate and record the decision rather than silently ignoring it.

Three flags that are routinely confused — they are independent (cas-0b32 / cas-369f):

| Flag | What it authorizes | What it does NOT do |
|---|---|---|
| `force=true` | Merging a **dirty** worktree | Does not authorize trunk as a target |
| `allow_trunk=true` | A genuine fallback to trunk when neither an epic branch nor task WorkTarget is declared | Is not needed for a declared WorkTarget and does not bypass dirty-tree protection |
| `cleanup=true/false` | Removing the worktree + deleting the branch after the merge | Not implied by `force` |

`cleanup` defaults to **preserve** for factory (`isolate=true`) worktrees, so a mid-epic
merge does not delete a live worker's cwd out from under it. Pass `cleanup=true` only at
end-of-lane, once the worker is done with that worktree.

**Worker hits MERGE REQUIRED / `awaiting_merge` (cas-c145):**
1. This is a **push signal**, not optional chat. Drain the merge queue before free-form user replies.
2. Confirm: `mcp__cs__coordination action=epic_status id=<focused-epic>` and/or `mcp__cs__task action=list status=awaiting_merge`.
3. Merge into the epic branch:
   ```
   mcp__cs__coordination action=worktree_merge id=<worker> task_id=<task-id>
   ```
   The resolved task and target branch are echoed back — read them before moving on. Push if remote tracking applies.
4. Message the worker to re-close (`mcp__cs__task action=close id=<task-id>`). After merge, normal close/review flow resumes.
5. Then clear context / hand the worker their next task. Do **not** poll for merge state.

If the merge is rejected on review rather than landed, the sanctioned exit from
`awaiting_merge` is `mcp__cs__task action=request_changes id=<task-id>` — it reopens the
task with the assignee preserved, so the same worker resumes the rework.

### Keeping other workers current

After the epic branch advances, rebase the other worktrees with one call rather than
messaging each worker a `git rebase` recipe:

```
mcp__cs__coordination action=sync_all_workers branch=epic/<slug>
```

It deliberately **skips** worktrees that are dirty or whose assignee is mid-task, and
reports why. `force=true` is consent for exactly those two cases (WIP is stashed, rebased,
and restored). A worktree already **mid-rebase is always refused**, `force` or not — sync
did not create that state and rebasing on top of it destroys the resolution in progress;
finish it or `git rebase --abort` in that worktree first.

If `worktree_merge` cannot act, stop and ask the supervisor to resolve the merge; do not
invent a second merge procedure.

## Phase 3: Review (Shared Mode)

When workers share the main directory, there's no branch merging — workers commit directly.

**Worker completes a task:**
1. Worker closes their own task
2. Review their commits
3. Clear worker context and assign next task

## Handling Blockers

- Workers set status to blocked and add a blocker note
- Help resolve or reassign the task
- **Race condition warning:** Task state updates are not atomic across supervisor and worker. After closing a task, verify it stayed closed before proceeding — a worker's stale `status=blocked` update can overwrite the close. If a worker resurrects a closed task, re-close with an audit trail noting the race.
- **Stale outbox replays:** Workers may send duplicate stale messages due to outbox replay. Before acting on a blocker notification or status change, check the task's current state with `mcp__cs__task action=show` — the message may be outdated.

**Multiple workers complete simultaneously:**
- Run verification calls in parallel (single response turn)
- Close approved tasks in a second parallel pass
- Reassign workers immediately

## Phase 4: Complete

1. Verify all tasks closed: `mcp__cs__task action=list status=open epic=<epic-id>`
2. Hold the main merge. The epic branch is not ready for base until the assembled diff has passed review and the final gate.
3. Run the final assembled-tree gate. Phase 3 review receipts cover each
   worker merge; this gate checks cross-task integration on the final tree:
   ```bash
   cargo nextest run -p cas
   ```
4. Turn any final-gate failure or review gap that needs worker action into a
   bounded epic-child fix-round task before messaging a worker. Put the finding,
   required fix, acceptance criteria, and proof command in the task description;
   the coordination message only points at the task ID.
5. After the fix lands, rerun the final assembled-tree gate yourself and capture
   the real exit code:
   ```bash
   cargo nextest run -p cas > /tmp/<epic-id>-cargo-nextest.log 2>&1; echo $?
   ```
   Never pipe the test run to `tail`; that captures the pipe status, not the
   nextest status.
6. **Isolated mode only**: land the lanes and reclaim the worktrees (can be 10GB+ each) only after the review loop is clean and the full gate exits 0. This is the end-of-lane consume, so `cleanup=true` is correct here:
   ```
   # One per worker lane — removes the worktree and deletes factory/<worker>
   mcp__cs__coordination action=worktree_merge id=<worker> task_id=<task-id> cleanup=true
   mcp__cs__coordination action=shutdown_workers count=0
   ```
   Then merge the epic branch to base. A standalone task with a declared WorkTarget
   needs no trunk flag. Only a missing-target fallback to trunk needs `allow_trunk=true`;
   its refusal names the destination and its success receipt carries a loud trunk-push warning.
   `force=true` will not authorize trunk.
   If the tracked merge cannot act, stop and ask the supervisor to resolve it; do not use an untracked merge path.
7. Close the epic and post release notes.
8. Shutdown workers: `mcp__cs__coordination action=shutdown_workers count=0`
