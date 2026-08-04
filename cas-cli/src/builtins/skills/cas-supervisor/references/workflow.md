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
   mcp__cas__task action=list task_type=epic status=closed

   # CAS memories for learnings, bugfixes, architectural decisions
   mcp__cas__search action=search query="<keywords>" doc_type=entry limit=10

   # Codebase for existing implementations you might duplicate or conflict with
   Grep pattern="<feature-name>" or mcp__cas__search action=search query="<keywords>" scope=code
   ```
2. Create EPIC: `mcp__cas__task action=create task_type=epic title="..." description="..."`
3. Gather spec with `/epic-spec`, break down with `/epic-breakdown`
4. Review task scope and dependencies

## Phase 2: Coordinate

1. Spawn workers:
   ```
   mcp__cas__coordination action=spawn_workers count=N isolate=true cli=codex model=gpt-5.6-terra effort=high
   ```
   Omit `isolate` for shared mode.

   **Hard rule:** every `spawn_workers` call MUST include explicit `cli=`,
   `model=`, and `effort=`. Codex is the default matrix
   (`cli=codex model=gpt-5.6-terra` for light/standard); taste/judgment uses
   `cli=codex model=gpt-5.6-terra effort=high`, while Sol/high is heavy/frontier only; Opus is exceptional, and Grok
   is the health-gated capacity route.
   Omitted fields fall back through the factory config cascade and stock floor;
   the spawn acknowledgement nags because supervisors should make worker tier
   selection intentional and visible.

   **Tiered mix example** — Codex-first standard floor + light + heavy:
   ```
   # Standard floor (Codex gpt-5.6-terra high)
   mcp__cas__coordination action=spawn_workers count=2 cli=codex model=gpt-5.6-terra effort=high isolate=true

   # Light / bulk (Codex gpt-5.6-terra low)
   mcp__cas__coordination action=spawn_workers count=1 cli=codex model=gpt-5.6-terra effort=low worker_names="lt-ada" isolate=true

   # Heavy (Codex gpt-5.6-sol high); frontier is model=gpt-5.6-sol
   mcp__cas__coordination action=spawn_workers count=1 cli=codex model=gpt-5.6-sol effort=high worker_names="hv-ada" isolate=true
   ```
   `cli`, `model`, and `effort` are per-spawn controls for the workers spawned
   by that call.
   Spawn the tier mix the ready backlog needs — one `spawn_workers` call per tier; rubric
   and routing in [model-selection.md](model-selection.md).
   Full parameter table in [reference.md](reference.md#spawn_workers-parameters).
2. Verify workers appear in TUI before assigning (stale DB records are not real workers)
3. Assign tasks: `mcp__cas__task action=update id=<id> assignee=<worker>`
4. Pin epic focus so the TUI shows it immediately: `mcp__cas__coordination action=focus_epic id=<epic-id>`. Without this, the TASKS/FACTORY panels stay empty until a worker's first `task action=start` on a subtask lets the panel infer the epic — and inference only fires once that subtask's `assignee` matches a live session agent (workers now get this for free: `task action=start` sets `assignee` automatically when unset, cas-6945). Clear with `action=focus_epic clear=true` when the epic wraps.
5. Search for relevant context and send assignment message:
   ```
   mcp__cas__coordination action=message target=<worker> \
     summary="Task <id> assignment" \
     message="Task <id>: <description>. Context: <findings>. Run mcp__cas__task action=mine to see your tasks."
   ```
6. **End your turn immediately.** Stop here. Do not monitor, poll, or run any commands. Workers will push a message to you when done or blocked. Your next action is triggered by their message, not by checking.

### Resuming an Existing EPIC

Workers from previous sessions are gone. Stale DB records are not live processes.

1. **Check for binary/source drift** — fixes merged to main since last session don't take effect until rebuild. Run `~/.cargo/bin/cargo build --release` if CAS source changed, then restart `cas serve`. If a "fixed" bug reappears, this is the first thing to check.
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

**Worker hits MERGE REQUIRED / `awaiting_merge` (cas-c145):**
1. This is a **push signal**, not optional chat. Drain the merge queue before free-form user replies.
2. Confirm: `mcp__cas__coordination action=epic_status id=<focused-epic>` and/or `mcp__cas__task action=list status=awaiting_merge`.
3. Merge `factory/<worker>` into the epic branch (FF preferred; else `git merge --no-ff factory/<worker>` on the epic checkout). Push if remote tracking applies.
4. Message the worker to re-close (`mcp__cas__task action=close id=<task-id>`). After merge, normal close/review flow resumes.
5. Then clear context / hand the worker their next task. Do **not** poll for merge state.

**Worker completes a task:**
1. Worker closes their own task
2. Review changes in the worker worktree: `git -C .cas/worktrees/<worker> log --oneline main..HEAD`
3. Cherry-pick to base branch: `git cherry-pick <commit-sha>` (one per commit)
   - **If conflicts arise:** (a) non-overlapping additions (e.g., both workers added to Cargo.toml) — keep both entries, (b) semantic conflicts — review both changes and pick the correct merge, (c) if unsure — message the worker who committed for context before resolving
4. Verify build after cherry-pick: `~/.cargo/bin/cargo build --quiet`
5. Run the lightweight per-merge gate. Do **not** run the full multi-persona
   `/cas-code-review` pipeline here by default; that review runs once in
   Phase 4 after the epic is code-complete. For this merge:
   - Read the direct diff against the task spec and acceptance criteria.
   - Check ownership boundaries, obvious defects, missing files/tests, and
     whether the worker proved the right command.
   - Run targeted mechanical verification only when warranted by the diff.
   - Record the audit trail:
     `mcp__cas__verification action=add task_id=<task-id> status=approved summary="<per-merge gate: diff read + proof checked>"`.
   - If the single diff is exceptionally risky, you may run
     `/cas-code-review mode=interactive base_sha=<pre-cp-sha> task_id=<task-id>`
     by explicit judgment; this is an exception, not the default cadence.
6. Message other active workers to sync onto the **local** branch (not `origin/`):
   ```
   mcp__cas__coordination action=message target=<other-worker> \
     summary="Epic branch updated" \
     message="Branch updated after cherry-pick. Sync: git stash && git rebase <base-branch> && git stash pop"
   ```
7. Clear completed worker's context: `mcp__cas__coordination action=clear_context target=<worker>`
8. Assign next task

## Phase 3: Review (Shared Mode)

When workers share the main directory, there's no branch merging — workers commit directly.

**Worker completes a task:**
1. Worker closes their own task
2. Review their commits
3. Clear worker context and assign next task

## Handling Blockers

- Workers set status to blocked and add a blocker note
- Help resolve or reassign the task
- **Race condition warning:** Task state updates are not atomic across supervisor and worker. After closing a task (especially via the escape hatch), verify it stayed closed before proceeding — a worker's stale `status=blocked` update can overwrite the close. If a worker resurrects a closed task, re-close with an audit trail noting the race.
- **Stale outbox replays:** Workers may send duplicate stale messages due to outbox replay. Before acting on a blocker notification or status change, check the task's current state with `mcp__cas__task action=show` — the message may be outdated.

**Multiple workers complete simultaneously:**
- Run verification calls in parallel (single response turn)
- Close approved tasks in a second parallel pass
- Reassign workers immediately

## Phase 4: Complete

1. Verify all tasks closed: `mcp__cas__task action=list status=open epic=<epic-id>`
2. Hold the main merge. The epic branch is not ready for base until the assembled diff has passed review and the final gate.
3. Run the single required full multi-persona review against the assembled EPIC
   diff. The Phase 3 per-merge gate catches obvious per-task problems; this
   step is the full `/cas-code-review` pass that catches cross-task integration
   issues (e.g., two tasks individually clean but semantically conflicting).
   From inside the epic branch checkout, invoke:
   `/cas-code-review mode=interactive base_sha=<base-branch>`
   (substitute `main`, `develop`, or your actual base branch name for `<base-branch>`)
   For large diffs, write the literal diff to a file first:
   ```bash
   git diff <base-branch>..HEAD > /tmp/<epic-id>-diff.patch
   ```
   Stay on the epic branch and pass that file path in the review context so personas read the literal assembled diff while exploring the correct tree.
4. Turn any review finding that needs worker action into a bounded epic-child fix-round task before messaging a worker. Put the finding, required fix, acceptance criteria, and proof command in the task description; the coordination message only points at the task ID.
5. After the fix lands, rerun the full gate yourself and capture the real exit code:
   ```bash
   cargo test --no-fail-fast > /tmp/<epic-id>-cargo-test.log 2>&1; echo $?
   ```
   Never pipe the test run to `tail`; that captures the pipe status, not the cargo status.
6. **Isolated mode only**: Merge epic to base branch and cleanup worktrees (can be 10GB+ each) only after the review loop is clean and the full gate exits 0:
   ```bash
   git checkout <base-branch> && git merge epic/<slug>
   mcp__cas__coordination action=shutdown_workers count=0
   git worktree remove <path>  # for each worker worktree
   git branch -d epic/<slug>
   ```
7. Close the epic and post release notes.
8. Shutdown workers: `mcp__cas__coordination action=shutdown_workers count=0`
