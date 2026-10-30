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

**Spawn threshold for review findings.** Read a verifier's `Improvements (non-blocking)` and warning issues, and leave them in the verdict. File a follow-up only when a finding shows reproduced, observable wrong behaviour that affects someone today. With three or more findings under that bar, note them together on the task and file nothing. A worker may fix a nit in its own lane before merge; a nit never reopens or extends a merged task. `task action=create` enforces this: a follow-up from a finding (`finding_origin`, a QA or verifier caller, or text naming one) needs `repro` and `affected`. A P0/P1 bug you or the operator observed directly is exempt. An epic with at least 5 completed reviews whose follow-ups from findings reach 0.5 per review over its last 10 sends you one notice and shows the ratio in `epic_status`. At release prep, run `cas qa polish --epic <id>` to list warnings by file, then batch them into one polish task for that release or drop them.

**Verification tiers.** Close records the tier on the task. Tier 0 (only docs, non-agent Markdown, `CHANGELOG.md`, or tests and journeys) has no independent verifier; CI and the mechanical gates still apply. Tier 1 is one verifier. Tier 2 (guards and hooks, `.github/workflows`, `scripts/release*`, merge-queue and close-gate code) adds an adversarial probe. QA follows the same tiers, including batch rounds: Tier 0 walks no journeys and needs no ledger row, Tier 1 walks the impacted journeys, and Tier 2 needs an `Adversarial probe:` row in the task ledger. To force a verifier on a Tier 0 task, add the `force-verifier` label before close.

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

`qa.user_facing_labels` require `demo_statement` before creation/assignment:
`As a <user>, I <do X> and see <Y>`; epics/internal tasks are exempt.

Human-facing output follows [DESIGN.md first](../../design-spec/references/design-md-first.md)
without being told; the brief adds only the target project when it is not the
checkout the worker runs in. Review the close's `design:` line against the output.

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
6. **Own the next exit rung.** If a worker owns it, use coordination updates. Directed messages wake an idle supervisor after debounce and pair rate limits. Receipts, pure acknowledgements, broadcasts and wake=false wait in the inbox on the next turn. Busy panes, in-flight tools and drafts defer wakes. Eligibility and transport delivery do not prove a model turn. If you own a time-based follow-up, schedule one `coordination remind` that names the exact check and when it fires. Do not spin-poll.

### Resuming an Existing EPIC

Workers from previous sessions are gone. Stale DB records are not live processes.

1. **Check deployment evidence** — call `system action=preflight` from connected MCP; otherwise run `cas --json factory preflight`. Follow the structured findings and `factory_blocked` decision in [preflight.md](preflight.md). Missing source comparison alone does not require rebuilding or reconnecting healthy MCP. If a "fixed" bug reappears, check this evidence first.
2. Inspect delivered work and apply the [closure pass](#wall-clock-is-a-resource) before new dispatch
3. Spawn fresh workers only for the work that pass selects; confirm they are live: `factory action=worker_status summary_mode=true`
4. Assign the selected tasks to those workers

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

`worktree_merge` admits a lane on its CI state and the merge preflight's
conflict and dirt checks. It runs no local release rows and needs no compile
receipt. CI runs the fast rows once per lane; the worker's pre-push hook gives
early feedback. Park reads a test receipt in each changed crate, without
trusting local fast-row receipts or exceptions. `CAS_LANE_SLIM=true` omits broad
Rust tests from lanes. The merge queue
is the Rust compile and suite proof. Rust merges still use the detached venue's
clean-target-checkout rule; commit or stash target-checkout dirt first.

### Wall-clock is a resource

A green push alone is not ready: a test-first checkpoint can be green and
incomplete. The role's readiness triggers require a completed delivery. Keep CI
and QA gates; pending/red CI needs override authority and a reason.

Finish delivered work before expanding noncritical work. Apply this closure
pass every turn, including startup and simultaneous worker completions:

1. Inspect pushed lanes, `READY TO MERGE`, `awaiting_merge` and delivered corrections. Merge every reviewed
   lane with satisfied QA gates and green required CI now; one lane's proof cannot delay
   another ready merge. Complete any remaining close action.
2. Give each held delivery a named blocker, next owner and event/reminder:
   missing candidate, review, QA, red/pending CI, or merge conflict. Assign
   available workers to those bounded gaps before opening new noncritical work.
   For a correction, use [correction proof](correction-proof.md) to retain
   unaffected evidence and repeat only what its delta invalidates.
3. If every delivery is genuinely blocked on an external event, dispatch only
   already-authorized independent work that cannot delay closure. Keeping a
   worker busy is not a reason to expand release scope; an idle worker is valid.
   Keep Rust-compiling lanes at or below `[factory] max_concurrent_builders`.

Use existing task notes and merge receipts to record first delivery time,
merge time and elapsed delivered-to-merged latency, with the cause of a hold.
Report the oldest held delivery and its next action rather than utilization;
this needs no new ledger or mandatory test matrix.

When tracked delivery requires `worktree_merge`, use that path so leases stay
consistent.

The default merge preserves the worker checkout for continued work. A later
`shutdown_workers` may remove that checkout once it is clean and the worker
has no nonterminal tasks. Keep the worker running while another process
depends on its checkout path.

A merge into an `epic/` branch runs no local suite (3.54.0). The epic's one
integration proof is the merge queue run for its PR. When that PR leaves the
queue without merging, the daemon sends one `merge_queue_ejected` wake naming
the PR and the failing job; nothing is re-enqueued automatically.

`id` accepts the worker name or `factory/<worker>`. Target resolution: an explicit
`task_id` first, then the assignee's current task binding. A `focus_epic` pin is a
**display filter and never merge authority**, and Cassy never silently defaults to
`main`/`master`/`staging`.

### Required merge-review discipline

Before landing a worker lane, do these two checks. Workers may carry capped
`check: PASS <sha>` and targeted `test: PASS <sha> <package> <filter> <count>`
evidence; verify they name the delivered commit and affected scope. The full
Rust build and suite run once, at the Phase 4 final gate: the merge queue where
base has one, otherwise local assembly.

For public surfaces, score distinctiveness, fit and hierarchy 1–5 using
cas-codebase-design before merge; the floor is 4/5. Record exceptions/remedies.
A script used on several platforms needs more than Linux proof unless a fixture
covers the difference.

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
   For normal delivery, hold pending, unavailable and no-checks results until
   required validation is green, even when the tool reports them as advisory.
   Red/pending CI is not proof that can be reused away; any supported override
   needs its explicit authority and logged reason. The lookup never polls.

Three flags that are routinely confused — they are independent (cas-0b32 / cas-369f):

| Flag | What it authorizes | What it does NOT do |
|---|---|---|
| `force=true` | Merging a **dirty** worktree | Does not authorize trunk as a target |
| `allow_trunk=true` | A genuine fallback to trunk when neither an epic branch nor task WorkTarget is declared | Is not needed for a declared WorkTarget and does not bypass dirty-tree protection |
| `cleanup=true/false` | Removing the worktree + deleting the branch after the merge | Not implied by `force` |

`cleanup` defaults to **preserve** for factory (`isolate=true`) worktrees, so a mid-epic
merge does not delete a live worker's cwd out from under it. Pass `cleanup=true` only at
end-of-lane, once the worker is done with that worktree.

**Pushed lane is green / READY TO MERGE / MERGE REQUIRED (cas-20d3):**

1. Merge on `READY TO MERGE`, a task in `awaiting_merge`, or a worker's explicit `status: ready` naming its pushed tip (a short SHA is fine). Do not wait for a merge request or for park once one of these holds, including when a close gate refused park. A pushed tip with green lane CI and no ready signal is not ready: a test-first checkpoint can pass its selected CI while incomplete (cas-fc42). Drain ready merges before free-form user replies.
2. Confirm the tip and resolved target with `factory action=epic_status id=<focused-epic>` or `task action=show id=<task-id>`; `task action=list status=awaiting_merge` shows only parked deliveries.
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
   `git merge factory/<worker>`, and task close all refuse until then. A rejection sends the
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
4. The merge starts the close: Cassy parks a pushed, unparked lane tip itself and opens the verification dispatch at the merge commit. Run the verifier on that dispatch and send the worker nothing. PASS closes the task with the merge receipt and tells the worker once. FAIL returns it Open to the same worker with the findings (`request_changes`). Message the worker only when the merge or verdict receipt names another gate that still holds.
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

- **Same patch-id:** reuse the source-review verdict; inspect changed base
  dependencies for integration risk. Continue through current QA and CI gates.
- **Different patch-id:** the rebase changed content (a conflict resolution
  or new commits). Review only what changed:
  `git range-diff <old-base>..<old-tip> <new-base>..factory/<worker>`.

A matching patch-id proves the diff is the same. It does not prove the diff
still integrates with the new base: the merge queue run on the epic PR
still proves that. A green CI run is not a review verdict. Runtime evidence
and independent QA have separate reuse limits in [correction proof](correction-proof.md).

## Phase 3: Review (Shared Mode)

When workers share the main directory, there's no branch merging — workers commit directly.

**Worker completes a task:**

1. Worker closes their own task
2. Review their commits
3. Clear worker context; apply the closure pass before assigning another task

### Lessons must remove causes

Fix a recurring failure on the highest rung that fits — a default or auto-fix, then a seconds-long preflight, then a gate row, prose last. A lesson learned twice needs an open P0/P1 task that removes the cause, not another note. Tag lessons `operation:release-cut|spawn|report|epic-close`; starting that operation surfaces them.

## Handling Blockers

- Workers set status to blocked and add a blocker note
- Help resolve or reassign the task
- **Reopened task:** Read its current state and findings before acting; the merge verdict owns completion.
- **Stale outbox replays:** Workers may send duplicate stale messages due to outbox replay. Before acting on a blocker notification or status change, check the task's current state with `task action=show` — the message may be outdated.

**Multiple workers complete simultaneously:**

- Merge each parked lane (`worktree_merge`) in one response turn
- Let each merge verdict close the task or return findings
- Apply the closure pass before reassigning workers

### Supervisor-run fixture scripts

When a walk needs records a worker can't create (accounts, tokens or data in a shared environment whose credentials the supervisor holds), the worker drafts a guarded script in its worktree and the supervisor runs it:

1. The script checks the target host against the allowed environment and exits otherwise, refuses on a colliding record instead of overwriting it, writes its outputs mode 600 under the task artifact directory, and prints sanitized errors with no secrets or tokens.
2. The supervisor reviews the script against those four guards, runs it from the worker's worktree (the shared checkout may lack the file), and sends the worker the output path.

## Batch QA

Batch QA is the built-in default; `qa.mode=per_task` explicitly enables per-delivery
reviews. Project QA keys override user `~/.cas/config.toml`, then built-in defaults.
Every hour with new deliveries, build a candidate at the exact epic tip, walk the
combined matrix and publish its PASS ledger. Record `verification action=batch_receipt`
with `task_ids`, `staging_sha`, `ledger_artifact_id` and `ledger_path`; every listed
delivery anchor must be contained in that reviewed target commit. Then close those
deliveries. A deferred close waits for this receipt. Explicit `qa_request` reviews
remain available.

## Phase 4: Complete

1. Verify every child is closed and merged: `factory action=epic_status id=<epic-id>` (the same source as the epic close gate; a `status=open` list misses `in_progress`, `blocked`, and `awaiting_merge`).
2. Hold the main merge. The epic branch is not ready for base until the assembled diff has passed review and the final gate. Where a merge queue is the final gate (step 3), enqueueing the reviewed PR is the hold: the queue lands it only after its run passes.
3. Run the final assembled-tree gate. This is the epic's single build + test:
   targeted worker tests complement one full assembly gate run
   on the epic tip proves every child and checks cross-task integration.
   **When the base branch lands through a merge queue that runs the full suite
   on the merged tree, that queue run is this gate.** Do not run a local full
   suite before the PR: lane CI admits lanes, the epic PR
   enters the queue, and the queue proves the exact tree that becomes base. The
   epic closes once its tip is contained in base.
   With a merge queue, the queue owns the full suite; without one, the full suite must be green on the exact SHA reaching the default branch, via CI check-runs on that SHA (preferred) or the supervisor's integrated run recorded on the epic; workers run targeted tests only. Read existing receipts; each check runs once.
   Without such a queue, prefer existing CI check-runs on the exact default-branch SHA;
   if unavailable, run the project's assembly gate on that SHA and record it on the epic. In a project
   with `scripts/assembly-proof.py` that gate, and the opt-in diagnostic after a
   failed queue run, is
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
   reference the epic's integration proof; worker closes carry targeted receipts alongside it.
4. Turn any final-gate failure or review gap that needs worker action into a
   bounded epic-child fix-round task before messaging a worker. Put the finding,
   required fix, acceptance criteria, and proof command in the task description;
   the coordination message only points at the task ID.
5. After the fix lands, rerun the final assembled-tree gate on the new tip:
   re-enqueue the PR, or for a local gate capture the real exit code and record
   a fresh `ASSEMBLY_PROOF` for it:

   ```bash
   <assembly gate command> > <artifacts_root>/<project-key>/<epic-id>/assembly-gate.log 2>&1; echo $?
   ```

   Never pipe the test run to `tail`; that captures the pipe status, not the
   test status.
6. **Isolated mode only**: every lane already landed on the epic branch in Phase 3, before the gate ran. Once the review loop is clean and the gate passes (the local gate exits 0, or the queue lands the PR), reclaim each lane's worktree (can be 10GB+ each); this end-of-lane consume is where `cleanup=true` is correct:

   ```text
   # One per worker lane — removes the worktree and deletes factory/<worker>
   factory action=worktree_merge id=<worker> task_id=<task-id> cleanup=true
   ```

   Then merge the epic branch to base. A standalone task with a declared WorkTarget
   needs no trunk flag. Only a missing-target fallback to trunk needs `allow_trunk=true`;
   its refusal names the destination and its success receipt carries a loud trunk-push warning.
   `force=true` will not authorize trunk.
   If the tracked merge cannot act, stop and ask the operator to resolve it; do not use an untracked merge path.
7. Start the [post-release check](reporting-and-routing.md#post-release-check) when a deployed product's merge lands; close the epic and post release notes once it is clean.
8. Shut down the epic's workers by name: `factory action=shutdown_workers worker_names=<worker>[,<worker>...]`
