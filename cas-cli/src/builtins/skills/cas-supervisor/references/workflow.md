# Workflow — Worker Modes, Phases, Blockers

Contents: [Worker modes](#worker-modes) · [Worker count](#worker-count-strategy) · [Phase 1: Plan](#phase-1-plan) · [Phase 2: Coordinate](#phase-2-coordinate) · [Brief fields](#brief-fields-no-spawn-without-them) · [Phase 3: Merge and sync](#phase-3-merge-and-sync-isolated-mode) · [Re-verify after a rebase](#re-verify-a-rebased-lane-by-patch-id) · [Blockers](#handling-blockers) · [Phase 4: Complete](#phase-4-complete)

## Worker Modes

Workers can run in two modes:

- **Isolated** (`isolate=true`, the recommended mode): Each worker gets its own git worktree and branch, and lanes merge cleanly through `worktree_merge`.
- **Shared** (`isolate=false` or omitted, the default): Workers share one mutable checkout and HEAD. It is contamination-prone — HEAD can switch between tool calls and commits can land on another worker's branch — and every spawn receipt warns about it. Use it only for a single worker or read-mostly work.

## Worker Count Strategy

Spawn workers based on independent file groups, not task count.

1. Map which files each task will modify
2. Group tasks touching the same files into one lane (prevents conflicts)
3. Workers needed = number of parallel lanes

```text
# 8 tasks, but only 2 independent file groups → 2 workers, not 8
workers = min(tasks_without_file_overlap, tasks_at_same_dependency_level)
```

In shared mode, file-overlap analysis is even more critical — two workers editing the same file simultaneously will cause problems.

## Phase 1: Plan

1. Search before planning — check all three sources for prior art:

   ```text
   # Similar past EPICs (patterns, sizing, what worked)
   task action=list task_type=epic status=closed

   # Cassy memories for learnings, bugfixes, architectural decisions
   search action=search query="<keywords>" doc_type=entry limit=10

   # Codebase for existing implementations you might duplicate or conflict with
   Grep pattern="<feature-name>" or search action=search query="<keywords>" doc_type=code
   ```

2. Create EPIC: `task action=create task_type=epic title="..." description="..."`
3. Gather the EPIC specification and task breakdown through the supervisor's task/spec workflow.
4. Review task scope and dependencies

**Standalone follow-up work (no EPIC needed).** An EPIC is for a body of work broken into
tasks. When an epic has closed and one loose task turns up — a follow-up, a late bug, a
one-off — do NOT create a single-child epic to satisfy the spawn gate. Create the task and
spawn straight onto it:

```text
task action=create title="..." description="..." risk=none
factory action=spawn_workers count=1 isolate=true cli=codex model=gpt-6.1-sol effort=high task_id=<task-id>
```

An open, unassigned `task_id` authorizes the spawn on its own; the refusal rules are in the
[`spawn_workers` parameter table](reference.md). Ceremonial
single-child epics distort epic reporting, so this is the preferred path.

### Brief fields: no spawn without them

Before you spawn onto a task or assign it, its task record must carry four
fields. A missing field means no spawn: fill it with `task action=update`
first. A worker cannot recover a goal or a proof method the brief never named.

| Field | Where it lives |
|---|---|
| Goal: what is true when the task is done | `description` |
| Scope: files to edit, and what is off-limits | `description` (or `design`) |
| Acceptance: the observable pass condition | `acceptance_criteria` |
| Proof expected: how the result will be checked | `proof_targets`, `demo_statement`, or the named command or grep in `acceptance_criteria` |

Workers report in one shape: the close reason starts `PASS`, or `ISSUES`
plus the known non-blocking defects, then the commit SHA and how it was
checked. A blocked worker does not close: it sets `status=blocked` and
messages you with `blocker=true`. A report without the SHA or the check
the brief named is a gap, not a pass: ask for it once, and a second miss
goes through the [retry policy](worker-recovery.md#retry-policy-by-failure-mode).

## Phase 2: Coordinate

1. Spawn workers:

   ```text
   factory action=spawn_workers count=N isolate=true cli=codex model=gpt-6.1-sol effort=high
   ```

   **Worker GitHub access (GH #1005).** Workers never inherit your GitHub
   credentials. When a task cites an issue (a github.com issue URL,
   `owner/repo#N`, or `GH #N`), Cassy attaches its body and comments at
   assignment under `<artifacts_root>/<task>/github-issues/`, and task show and
   task start list the files. Do not relay issue text by hand. For `gh issue
   view`, `gh pr checks` and `gh run view` inside workers, export a read-only
   fine-grained token as `CAS_WORKER_GITHUB_READ_TOKEN` before starting the
   factory; each worker gets it as its own `GH_TOKEN`.

   **Tier every spawn.** Pass `lane=<light|standard|taste|heavy>` (preferred: the
   registry resolves the recipe and reports any fallback loudly) or a complete
   explicit `cli=`/`model=`/`effort=` recipe to force one model — never both. An
   untiered spawn falls back through the factory config cascade and its receipt
   warns. Lane matrix and fallbacks: [model-selection.md](model-selection.md).

   **Tiered mix example** — the explicit recipe for each active registry lane:

<!-- Generated recipes retain their generator-owned fence formatting. -->
<!-- markdownlint-disable MD031 MD040 -->
<!-- BEGIN GENERATED SPAWN RECIPES: cas-factory lane registry -->
Copy-paste commands generated from the registry; every recipe pins `cli`, `model`, and `effort`:

```text
# light — recipe codex_luna_6 (fallback: claude_opus_5_5_low)
factory action=spawn_workers count=1 isolate=true cli=codex model=gpt-6-luna effort=xhigh

# standard — recipe codex_sol_6_1 (fallback: codex_sol_6)
factory action=spawn_workers count=1 isolate=true cli=codex model=gpt-6.1-sol effort=high

# taste — recipe claude_opus_5_5 (fallback: claude_opus)
factory action=spawn_workers count=1 isolate=true cli=claude model=claude-opus-5-5 effort=high

# heavy — recipe claude_opus_5_5 (fallback: codex_astra_high)
factory action=spawn_workers count=1 isolate=true cli=claude model=claude-opus-5-5 effort=high

# supervisor — recipe claude_opus_5_5 (fallback: claude_fable_high)
factory action=spawn_workers count=1 isolate=true cli=claude model=claude-opus-5-5 effort=high

```
<!-- END GENERATED SPAWN RECIPES -->
<!-- markdownlint-enable MD031 MD040 -->
   Spawn the tier mix the ready backlog needs — one `spawn_workers` call per tier.
   Full parameter table in [reference.md](reference.md).
   **Build-load guard:** `spawn_workers` measures the one-minute host load and
   live Cargo builders before queueing. It refuses a request that would push
   past `[factory] max_concurrent_builders` (default `4`) or load above CPU
   capacity; use `force=true` only for an intentional override. The receipt
   records the effective per-worker `CARGO_BUILD_JOBS` (configure with
   `[factory] worker_build_jobs`, with `cargo_build_jobs` accepted as an alias), `nice -n` priority,
   load/cap measurements, and (after isolated provisioning) build-cache
   snapshot hardlink counts.
2. Confirm the workers are live before assigning (stale DB records are not real workers): `factory action=worker_status summary_mode=true`
3. Assign tasks: `task action=update id=<id> assignee=<worker>`
4. Pin epic focus so the TUI shows it immediately: `factory action=focus_epic id=<epic-id>`. Without this, the TASKS/FACTORY panels stay empty until a worker's first `task action=start` on a subtask lets the panel infer the epic — and inference only fires once that subtask's `assignee` matches a live session agent (workers now get this for free: `task action=start` sets `assignee` automatically when unset, cas-6945). Clear with `action=focus_epic clear=true` when the epic wraps.
5. Search for relevant context and send assignment message:

   ```text
   coordination action=message target=<worker> \
     summary="Task <id> assignment" \
     message="Task <id>: <description>. Context: <findings>. Run task action=mine to see your tasks."
   ```

<!-- markdownlint-disable-next-line MD029 -->
6. **Own the next exit rung.** If a worker owns it, use inbox updates on the next turn; only authenticated typed blocker, merge, verification, or lifecycle events may wake an idle supervisor. If you own a time-based follow-up, schedule one `coordination remind` that names the exact check and when it fires. Do not spin-poll.

### Resuming an Existing EPIC

Workers from previous sessions are gone. Stale DB records are not live processes.

1. **Check for a stale binary** — run `cas factory preflight`. If it reports a stale Cassy binary, stop and ask the operator to rebuild and reconnect MCP ([preflight.md](preflight.md)). If a "fixed" bug reappears, this is the first thing to check.
2. Spawn fresh workers
3. Confirm they are live: `factory action=worker_status summary_mode=true`
4. Assign open tasks to the new workers

## Phase 3: Merge and Sync (Isolated Mode)

For independent advisory Spec/Standards reviews alongside the task-verifier,
use [cas-shadow-review](../../cas-shadow-review/SKILL.md). It binds two separate
registered children to one dispatch, keeps fixes on side refs until explicit
supervisor opt-in, and records old/new verdicts without changing merge gates.


When workers have isolated worktrees, merge their work into the epic branch after each completion, then tell other workers to sync.

```text
base branch ────────────────────► (stays clean)
          \                    /
           └─ epic/feature ───►
              \          \     /
               ├─ factory/fox ┤
               └─ factory/owl ┘
```

### Merge workers with Cassy

`factory action=worktree_merge` is the worker merge path. It resolves
the merge target from task state, enforces the trunk guard, and keeps factory tracking,
leases, and cleanup consistent.

Run the canonical merge-time diff review ([Required merge-review discipline](#required-merge-review-discipline)) before landing each lane.

```text
factory action=worktree_merge id=<worker> task_id=<task-id>
```

Projects shipping `scripts/check-lane-fast-rows.py` run the deterministic
`release-gate.sh --fast-rows` checks on a detached preview of the combined tree
before either merge path moves the target. A failed receipt names the row and
refuses the merge, including with `force=true`. Install
`markdownlint-cli2@0.18.1` on the supervisor host before merging changed docs;
the fast gate downloads no tools and invokes no Cargo commands.

Rust-touching lanes additionally require a capped compile PASS for the combined
merge tree. A lane-tip check or green CI run cannot supply that receipt. Before
merging, run the proof as a supervisor, in the background with a retained log:

```bash
python3 scripts/check-lane-compile.py . <target-branch> factory/<worker> --prove
```

The prover invokes the existing capped runner once with `--lib` and once with
`--tests`, both scoped to the affected Cargo packages, in an isolated preview.
It publishes a tree-keyed receipt only after both checks pass and the preview
and input refs remain unchanged. A failed retry removes an earlier PASS. The
normal preflight verifies this receipt without compiling. Missing proof prints
the exact command to produce it; docs/scripts-only lanes need no compile proof.
The Git merge helper also verifies the actual detached merge commit before its
compare-and-swap advances the epic, covering merge drivers and concurrent lanes.
Rust merges use the detached venue's clean-target-checkout rule; commit or stash
target-checkout dirt first. Other projects retain their existing merge policy.

### Wall-clock is a resource

A reviewed delivery or an idle worker that waits costs the epic time. Apply
these rules every turn:

- **Merge non-Rust deliveries at once.** A docs, scripts or web lane needs no
  compile proof. Merge it as soon as review passes; never queue it behind a
  Rust proof.
- **Cap Rust lanes, not the fleet.** Keep Rust-compiling lanes at or below
  `[factory] max_concurrent_builders`. Give every extra worker non-Rust work.
- **Leave nothing waiting.** In the turn you review or merge a delivery,
  assign its idle worker the next task. A reviewed delivery is either merging,
  under a running proof, or blocked on a named cause.
- **Prefer parallel proofs to serial waits.** Start every independent proof
  before you wait on any of them. With two or more reviewed Rust lanes parked,
  prove them together against their predicted merged trees, then merge them in
  the predicted order.

A predicted-tree proof for Rust lanes A then B on epic tip E chains each
predicted merge onto the previous one:

```bash
E=$(git rev-parse epic/<slug>)
T1=$(git merge-tree --write-tree "$E" factory/<worker-a>)
C1=$(git commit-tree "$T1" -p "$E" -p factory/<worker-a> -m "predicted merge: worker-a")
python3 scripts/check-lane-compile.py . "$E" factory/<worker-a> --prove > <artifacts>/lane-a.log 2>&1 &
python3 scripts/check-lane-compile.py . "$C1" factory/<worker-b> --prove > <artifacts>/lane-b.log 2>&1 &
```

For a third lane, predict `T2` and `C2` from `C1` and lane B the same way and
prove lane C against `C2`. `git merge-tree` must exit zero; a conflict means
that lane needs a rebase, so end the chain before it. Each proof is one capped
builder, so run no more at once than the cap leaves free.

Each receipt is keyed to its merged tree. Merge A, then B, with
`worktree_merge`. Merging A produces `T1`, so B's merge produces the tree
already proved and its preflight finds the receipt without compiling. If
another lane lands first, the order changes, or the actual merge tree differs,
the preflight names the missing proof; rerun that one.

A documented manual Git merge in a project that permits it uses the same
preflight, from the target checkout immediately before the merge:

```bash
python3 scripts/check-lane-fast-rows.py . <target-branch> factory/<worker> &&
  git merge --no-ff factory/<worker>
```

Run the second command only after the first exits zero. When tracked delivery
requires `worktree_merge`, use that path so receipts and leases stay consistent.

The default merge preserves the worker checkout for continued work. A later
`shutdown_workers` may remove that checkout once it is clean and the worker
has no nonterminal tasks. Keep the worker running while another process
depends on its checkout path.

After a successful merge into an `epic/` branch, the factory daemon launches a
bounded sweep of the project's detected test runner (`cargo nextest run --workspace
--no-fail-fast` for a Cargo project, the `test` script for a Node project) in a reusable
detached merged-tip worktree. The sweep is asynchronous and capped by
`[factory].merge_sweep_timeout_secs`; inspect the epic note and the durable
`<cas-root>/merge-sweeps/` log before accepting another merge when it reports
`FAILED`, `TIMED OUT`, or `SETUP FAILED`. Set `[factory].merge_sweep = false`
only when the host cannot absorb this additional validation load.
The sweep starts `[factory].merge_sweep_quiet_secs` (default 120 s) after the
last epic merge. A burst of merges runs one sweep of the newest tip, and each
merge cancels a stale running sweep at once. Set it to 0 to start immediately.
A project whose suites need their own script or environment sets
`[factory].merge_sweep_command` (run via `sh -c` instead of the detected
runner) and a `[factory.merge_sweep_env]` table in `config.toml`. Set
`[factory].merge_sweep_cwd = "web"` to run the command or detected runner in
that subdirectory; without it, detection searches the checkout and nested
directories for a test manifest. The sweep
log shows only the variable names. A sweep the build guard defers is noted on
the epic without a relay; the run that finally goes ahead sends one relay
naming its result and integration tip.

`id` accepts the worker name or `factory/<worker>`. Target resolution: an explicit
`task_id` first, then the assignee's current task binding. A `focus_epic` pin is a
**display filter and never merge authority**, and Cassy never silently defaults to
`main`/`master`/`staging`.

### Required merge-review discipline

Before landing a worker lane, do these two checks. Workers may carry capped
`check: PASS <sha>` and targeted `test: PASS <sha> <package> <filter> <count>`
evidence; verify they name the delivered commit and affected scope. The full
Rust build and suite run once, at Phase 4 assembly.

1. **Contract changes first.** If the diff changes a public contract (API shape,
   persisted field, CLI/MCP response, or behavior callers rely on), search for sibling
   tests that still pin the old contract before landing the lane. For example:
   `git grep -n '<old contract token>' -- '*test*'` (narrow the path/spec as needed).
   Update or reject the lane when those tests prove an unreviewed caller contract.
2. **Read the lane CI signal.** Inspect `gh run list --branch factory/<worker>` at
   review time. `worktree_merge` inspects the delivery range. It uses the tip's
   runs when they include code checks (a multi-commit push runs CI on the tip),
   otherwise the latest non-docs commit's runs; the receipt names that SHA.
   Skipped validation beside Docs Lint does not count as code evidence. A pure
   docs delivery can use Docs Lint at its tip. Red CI and documentation-only
   success for a code delivery refuse the merge unless a live registered
   supervisor supplies `supervisor_override=true`, `task_id`, and a non-empty
   `reason`; the decision is logged on the task. Pending, unavailable and
   no-checks results stay advisory, with code validation reported as unconfirmed.
   Investigate these results and record the decision; the lookup never polls.

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
2. Confirm: `factory action=epic_status id=<focused-epic>` and/or `task action=list status=awaiting_merge`.
   **Implementer evidence gates the park (cas-0cd5).** A user-facing delivery parks
   only after its own QA evidence bundle validates for the delivered commit. Before
   that, the worker's close returns `TASK CLOSE REJECTED: <task> is user-facing (…)` with
   the command that produces the missing piece. Send the worker that command.
   Waive only with a real reason, by running close yourself with
   `supervisor_override=true reason="…"`; the waiver is logged as a decision note.
   Added `test.fixme`/`.skip`/`.only` markers are refused on every delivery unless
   annotated with `cas-allow-skip: <reason>`.
   **User-facing delivery? Independent QA first (cas-619f).** When the park reports
   `INDEPENDENT QA DISPATCHED`, or a `<cas-qa-dispatch>` wakes you, spawn a reviewer who is
   not the implementer, on the taste recipe:
   `factory action=spawn_workers count=1 isolate=true cli=claude model=claude-opus-5-5 effort=high task_id=<qa-task>`.
   Merge only after that reviewer's `qa_record` approves the exact tip. `worktree_merge`, a raw
   `git merge factory/<worker>`, and the re-close all refuse until then. A rejection sends the
   task back to its implementer automatically. To skip the pass, waive it with a logged reason:
   `verification action=qa_waive task_id=<task-id> summary="..."`. Check a task's rounds
   with `verification action=qa_status task_id=<task-id>`.
   Already merged before anyone closed it (it never parked)? Cassy opens no round for code
   that is already on trunk. Close it yourself with `supervisor_override=true reason="…"
   commit_receipt=<merged sha>`; the waiver is recorded against that commit. A no-code task is
   never gated by independent QA.
3. Merge into the epic branch:

   ```text
   factory action=worktree_merge id=<worker> task_id=<task-id>
   ```

   The resolved task and target branch are echoed back — read them before moving on. Push if remote tracking applies.
4. Message the worker to re-close (`task action=close id=<task-id>`). After merge, normal close/review flow resumes.
5. Then clear context / hand the worker their next task. Do **not** poll for merge state.

If the merge is rejected on review rather than landed, the sanctioned exit from
`awaiting_merge` is `task action=request_changes id=<task-id>` — it reopens the
task with the assignee preserved, so the same worker resumes the rework.

### Keeping other workers current

After the epic branch advances, rebase idle or stale worktrees with one call:

```text
factory action=sync_all_workers branch=epic/<slug>
```

It rebases idle or stale worktrees only and reports every skip. A live worker's worktree
is **always skipped**, `force` or not — tell each live worker to rebase at its next task
start. `force=true` covers only a dirty tree or a stale worker record (WIP is stashed,
rebased, and restored). A worktree already **mid-rebase is always refused** — rebasing on
top of it destroys the resolution in progress; finish it or `git rebase --abort` in that
worktree first.

If `worktree_merge` cannot act, stop and ask the operator to resolve the merge; do not
invent a second merge procedure.

### Re-verify a rebased lane by patch-id

A review verdict belongs to a diff, not to a SHA. When a lane you already
reviewed is rebased (by `sync_all_workers` or by the worker), compare
patch-ids instead of re-reviewing from scratch. At review, record the base
SHA, the tip SHA and the lane's patch-id in the task note:

```bash
git diff <base>...factory/<worker> | git patch-id --stable
```

After the rebase, run the same command against the new base.

- **Same patch-id:** the diff did not change, so the verdict still holds.
  Merge without a second review.
- **Different patch-id:** the rebase changed content (a conflict resolution
  or new commits). Review only what changed:
  `git range-diff <old-base>..<old-tip> <new-base>..factory/<worker>`.

A matching patch-id proves the diff is the same. It does not prove the diff
still integrates with the new base: the merge sweep and the Phase 4 assembly
gate still run. A green CI run is not a review verdict.

## Phase 3: Review (Shared Mode)

When workers share the main directory, there's no branch merging — workers commit directly.

**Worker completes a task:**

1. Worker closes their own task
2. Review their commits
3. Clear worker context and assign next task

## Handling Blockers

- Workers set status to blocked and add a blocker note
- Help resolve or reassign the task
- **Race condition warning:** Task state updates are not atomic across supervisor and worker. After a worker's close, verify it stayed closed before proceeding — a stale `status=blocked` update can overwrite the close. If a closed task comes back, note the race on the task and message the worker to re-close.
- **Stale outbox replays:** Workers may send duplicate stale messages due to outbox replay. Before acting on a blocker notification or status change, check the task's current state with `task action=show` — the message may be outdated.

**Multiple workers complete simultaneously:**

- Merge each parked lane (`worktree_merge`) in one response turn
- Message each worker to re-close its own task
- Reassign workers immediately

## Phase 4: Complete

1. Verify every child is closed and merged: `factory action=epic_status id=<epic-id>` (the same source as the epic close gate; a `status=open` list misses `in_progress`, `blocked`, and `awaiting_merge`).
2. Hold the main merge. The epic branch is not ready for base until the assembled diff has passed review and the final gate.
3. Run the final assembled-tree gate. This is the epic's single build + test:
   targeted worker tests complement one full assembly gate run
   on the epic tip proves every child and checks cross-task integration.
   In a project with `scripts/assembly-proof.py`, the assembly command is
   `python3 scripts/assembly-proof.py prove <epic-worktree>`: it proves native
   nextest in the worktree and archive-mode in a plain clone. Both use their
   own `<worktree>/target`, seeding immutable dependencies from the worker
   build-cache; workspace freshness and Cargo locks remain private. Each log
   names its source worktree and HEAD. For scoped Cargo checks outside assembly,
   run `python3 scripts/proof_target.py run <worktree> -- cargo check --workspace --tests`;
   `scripts/run-scoped-tests.sh` uses the same isolation automatically.
   A matching code tree reuses its durable two-context PASS; integration recovery and the
   release gate cite that receipt instead of running the suite again.
   On exit 0, record a progress note on the epic:
   `ASSEMBLY_PROOF: head=<epic tip sha> result=PASS command=<cmd> log=<path>`,
   with the log under `[factory] artifacts_root/<epic-id>/`. Child task closes
   reference this proof; worker closes carry targeted receipts alongside the full assembly proof.
4. Turn any final-gate failure or review gap that needs worker action into a
   bounded epic-child fix-round task before messaging a worker. Put the finding,
   required fix, acceptance criteria, and proof command in the task description;
   the coordination message only points at the task ID.
5. After the fix lands, rerun the final assembled-tree gate yourself on the new
   tip, capture the real exit code, and record a fresh `ASSEMBLY_PROOF` for it:

   ```bash
   <assembly gate command> > <artifacts_root>/<project-key>/<epic-id>/assembly-gate.log 2>&1; echo $?
   ```

   Never pipe the test run to `tail`; that captures the pipe status, not the
   test status.
6. **Isolated mode only**: every lane already landed on the epic branch in Phase 3, before the gate ran. Once the review loop is clean and the gate exits 0, reclaim each lane's worktree (can be 10GB+ each); this end-of-lane consume is where `cleanup=true` is correct:

   ```text
   # One per worker lane — removes the worktree and deletes factory/<worker>
   factory action=worktree_merge id=<worker> task_id=<task-id> cleanup=true
   ```

   Then merge the epic branch to base. A standalone task with a declared WorkTarget
   needs no trunk flag. Only a missing-target fallback to trunk needs `allow_trunk=true`;
   its refusal names the destination and its success receipt carries a loud trunk-push warning.
   `force=true` will not authorize trunk.
   If the tracked merge cannot act, stop and ask the operator to resolve it; do not use an untracked merge path.
7. Close the epic and post release notes.
8. Shut down the epic's workers by name: `factory action=shutdown_workers worker_names=<worker>[,<worker>...]`
