# Workflow — Worker Modes, Phases, Blockers

Contents: [Worker modes](#worker-modes) · [Worker count](#worker-count-strategy) · [Phase 1: Plan](#phase-1-plan) · [Phase 2: Coordinate](#phase-2-coordinate) · [Brief fields](#brief-fields-no-spawn-without-them) · [Phase 3: Merge and sync](#phase-3-merge-and-sync-isolated-mode) · [Re-verify after a rebase](#re-verify-a-rebased-lane-by-patch-id) · [Blockers](#handling-blockers) · [Phase 4: Complete](#phase-4-complete)

## Worker Modes

- **Isolated** (`isolate=true`, recommended): each worker gets its own worktree and branch, and lanes merge through `worktree_merge`.
- **Shared** (the default): workers share one checkout and HEAD, so commits can land on another worker's branch. Use it only for one worker or read-mostly work.

## Worker Count Strategy

Group tasks that touch the same files into one lane; the worker count is the number of parallel lanes. Spawn one isolated worker per lane, pre-assigned to its first task (`spawn_workers count=1 lane=<lane> isolate=true task_id=<task-id>`), and assign that lane's later tasks to the same worker.

## Phase 1: Plan

1. Search for prior art: closed epics (`task action=list task_type=epic status=closed`), memories (`search action=search query="<keywords>" doc_type=entry limit=10`) and code (`doc_type=code` or a grep).
2. Create the EPIC with `task action=create task_type=epic title="..." description="..."`, then break it down with [planning.md](planning.md).

**Severity × reach for review findings.** Every finding names its impact, reach and journey. Use `severity:money|data_loss|broken|awkward|cosmetic`, `reach:main|secondary|theoretical` and `journey:<id>` labels with `repro`; see `docs/qa/triage.md`. `task action=create` accepts a code-read finding as theoretical P4 backlog; a reproduced main-journey failure outranks it. Money or data loss on a declared main journey is the only theoretical exception (P2). Use the existing `repro` and `affected` fields for observed failures and the severity/reach/journey labels; do not hand-pick P0. Declare main journeys in `docs/qa/journeys.yaml` and set `factory.focus_mode` for inherited operator intent. A staffing nudge names unowned main-journey FAILs; give those the worker first. A nit never reopens a merged task. At release prep, `cas qa polish --epic <id>` lists warnings by file; batch observed improvements or leave them in the backlog.

**Verification tiers.** Close records the tier. Tier 0 (docs, non-agent Markdown, `CHANGELOG.md`, or tests and journeys only) gets no independent verifier and walks no journeys. Tier 1 in `qa.mode=batch` shares one release verdict, recorded with `verification action=batch_receipt verification_type=release`; `qa.mode=per_task`, the `force-verifier` label or an unmeasurable diff gets its own verifier. Tier 2 (guards and hooks, `.github/workflows`, `scripts/release*`, integration and close-gate code) always gets its own verifier plus an `Adversarial probe:` ledger row.

**Standalone follow-up work.** For one loose task after an epic closes, do not create a single-child epic. Create the task and spawn onto it; an open, unassigned `task_id` authorizes the spawn:

```text
task action=create title="..." description="..." risk=none
factory action=spawn_workers count=1 lane=standard isolate=true task_id=<task-id>
```

### Brief fields: no spawn without them

Pass a stakeholder ask's Slack permalink or GitHub issue/PR URL as `external_ref`. Creation and staffing refuse a source a peer holds and name the owner: coordinate with that peer, and use `supervisor_override=true reason="<agreement>"` for a second task only once they agree. Send ownership messages that need action as `kind=status`.

A task needs four fields before you spawn onto it or assign it; fill a missing one with `task action=update` first. A worker cannot recover a goal or a proof method the brief never named.

| Field | Where it lives |
|---|---|
| Goal: what is true when the task is done | `description` |
| Scope: files to edit, and what is off-limits | `description` (or `design`) |
| Acceptance: the observable pass condition | `acceptance_criteria` |
| Proof expected: the touched tests; never name suites or evals | `proof_targets`, `demo_statement`, or the named command or grep in `acceptance_criteria` |

A brief names a full or stress run only as a named, justified exception. After a CI failure, gate only on the failed and never-run checks; a worker reruns only the failing spec.

For rule-driven work, cite each rule id and its verbatim stakeholder scope. Declare known causal anchors with `Rule-driven sites:` (`rule_id`, `path`, `anchor`) or require them in the close receipt; code and tests carry `cas-rule: <id> scope: <JSON string>` comments. Unrelated work does not inherit the rule's scope.

Tasks with `qa.user_facing_labels` need a `demo_statement` (`As a <user>, I <do X> and see <Y>`). Human-facing output follows [DESIGN.md first](../../design-spec/references/design-md-first.md); name the target project only when it is not the worker's checkout, and review the close's `design:` line against the output.

A worker's close reason starts `PASS`, or `ISSUES` with the known defects, then the commit SHA and the check it ran. A report without the SHA or the named check is a gap: ask once, and a second miss goes through the [retry policy](worker-recovery.md#retry-policy-by-failure-mode).

## Phase 2: Coordinate

1. Spawn one worker per lane ([worker count](#worker-count-strategy)). **Tier every spawn:** pass `lane=<light|standard|taste|heavy>` (the registry reports any fallback) or a complete `cli=`/`model=`/`effort=` recipe, never both ([model-selection.md](model-selection.md)). Spawn one `spawn_workers` call per tier the ready backlog needs:

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
   `spawn_workers` refuses a request past `[factory] max_concurrent_builders` or host CPU capacity; pass `force=true` only for a deliberate override. Workers never inherit your GitHub credentials: Cassy attaches a cited issue's body and comments at assignment, and a read-only token exported as `CAS_WORKER_GITHUB_READ_TOKEN` before the factory starts becomes each worker's `GH_TOKEN`.
2. Confirm the workers are live: `factory action=worker_status summary_mode=true`.
3. Assign: `task action=update id=<id> assignee=<worker>`. Pin the epic in the TUI with `factory action=focus_epic id=<epic-id>` (`clear=true` when it wraps).
4. **Own the next exit rung.** The brief lives in the task record; send no assignment message. Directed messages wake the recipient (the send result says whether it woke); receipts, acknowledgements, broadcasts and wake=false wait in the inbox on the next turn. Wait on a concrete event with `coordination action=remind remind_event=<event> remind_message="..."` or the harness background-task notice; never poll.

### Resuming an Existing EPIC

Workers from previous sessions are gone; stale DB records are not live processes.

1. Call `system action=preflight` (or `cas --json factory preflight`) and follow [preflight.md](preflight.md). If a "fixed" bug reappears, check this evidence first.
2. Apply the [closure pass](#wall-clock-is-a-resource) to delivered work.
3. Spawn fresh workers only for the work that pass selects, and confirm they are live.

## Phase 3: Merge and Sync (Isolated Mode)

Merge each completed lane into the epic branch. For optional advisory Spec/Standards reviews beside the verifier, use [cas-shadow-review](../../cas-shadow-review/SKILL.md).

### Merge workers with Cassy

`factory action=worktree_merge` is the only worker merge path. It resolves the target from task state and keeps tracking, leases and cleanup consistent. Run the canonical merge-time diff review ([Required merge-review discipline](#required-merge-review-discipline)) before landing each lane.

```text
factory action=worktree_merge id=<worker> task_id=<task-id>
```

It admits a lane on its CI state and the preflight's conflict and dirt checks, and its refusals name the gate. Lane CI admits lanes;
<!-- cas-src:begin -->
the merge queue is the Rust compile and suite proof.
<!-- cas-src:else -->
the Phase 4 final gate is the full suite proof.
<!-- cas-src:end -->

### Wall-clock is a resource

A green push alone is not ready: a test-first checkpoint can be green and incomplete. Finish delivered work before expanding noncritical work. Apply this closure pass every turn, including startup and simultaneous completions:

1. Inspect pushed lanes, `READY TO MERGE`, `awaiting_merge` and delivered corrections. Merge every reviewed lane whose QA gates hold and plan target is met; one lane's proof never delays another ready merge.
2. Give each held delivery a named blocker, next owner and event or reminder: missing candidate, review, QA, red CI or merge conflict. Staff those gaps before new noncritical work; for a correction, use [correction proof](correction-proof.md).
3. If every delivery waits on an external event, dispatch only already-authorized independent work. An idle worker is valid; keeping one busy is no reason to expand scope.

Report the oldest held delivery and its next action, not utilization. The default merge keeps the worker checkout; `cleanup=true` belongs at end of lane. A merge into an `epic/` branch runs no local suite.
<!-- cas-src:begin -->
When the epic PR leaves the merge queue unmerged, one `merge_queue_ejected` wake names the PR and failing job.
<!-- cas-src:end -->
A `focus_epic` pin is a display filter, never merge authority.

### Required merge-review discipline

Before landing a lane, check that the worker's capped `check: PASS <sha>` and targeted `test: PASS <sha> <package> <filter> <count>` receipts name the delivered commit and affected scope. The full build and suite run once, at the Phase 4 final gate. For public surfaces, score distinctiveness, fit and hierarchy 1–5 with cas-codebase-design; the floor is 4/5. A script used on several platforms needs more than Linux proof unless a fixture covers the difference.

1. **Contract changes first.** If the diff changes a public contract (API shape, persisted field, CLI/MCP response, or behavior callers rely on), search for tests that still pin the old contract, for example `git grep -n '<old contract token>' -- '*test*'`. Update or reject the lane when they prove an unreviewed caller contract.
2. **Read the lane CI signal.** `gh run list --branch factory/<worker>` at review time; the merge receipt names the code-check SHA it used. Red CI, or docs-only success on a code delivery, refuses the merge. Override it with `supervisor_override=true` and a reason only for a CI defect, never to skip a real failure. Pending, unavailable and no-checks results are advisory: the merge proceeds and the receipt marks code validation unconfirmed.

`force=true` merges a dirty worktree; never use it over a worker's uncommitted work without the operator. `allow_trunk=true` is only for a genuine missing-target fallback to trunk.

**READY TO MERGE / MERGE REQUIRED:**

1. READY TO MERGE, MERGE REQUIRED or task_awaiting_merge → merge first; a worker's own message is not a trigger. Green lane CI without a park is not ready. Hold deliveries below their plan target and their dependents; ship independent work. Drain ready merges before free-form replies.
2. Confirm the tip and target with `factory action=epic_status id=<focused-epic>` or `task action=show id=<task-id>`.
   In qa.mode=per_task only, a user-facing delivery parks only after its own QA evidence bundle validates; send the worker the command the refusal names. When only a deployment can sign in, set `cas config set qa.deployed_origins <origin>` and cite a `deployed` bundle. Waive with `supervisor_override=true reason="…"` only when no evidence can be produced.
   On `INDEPENDENT QA DISPATCHED` or a `<cas-qa-dispatch>` wake, spawn a reviewer who is not the implementer, on the account with the most room and the cheapest recipe that has passed this kind of review:
   `factory action=spawn_workers count=1 lane=standard isolate=true task_id=<qa-task>`.
   Merge and close refuse until its `qa_record` approves the exact tip, and a rejection returns the task to its implementer. Waive only with a logged reason (`verification action=qa_waive task_id=<task-id> summary="..."`). Code already merged without parking gets no round: close it with `commit_receipt=<merged sha>`.
3. Merge into the epic branch and read the echoed task and target:

   ```text
   factory action=worktree_merge id=<worker> task_id=<task-id>
   ```

4. The merge starts the close. In qa.mode=batch, Cassy closes merged deliveries when the release batch_receipt lands; the worker does nothing. A delivery with its own verifier (Tier 2, `force-verifier`, `qa.mode=per_task`): `worktree_merge` prepares a candidate and opens a pre-merge dispatch; run the verifier on it and send the worker nothing. When `worktree_merge` returns `PRE-MERGE VERIFICATION`, use your Task/Agent tool to spawn `subagent_type="task-verifier"` with `prompt="Verify task <task-id>"` for that exact dispatch. PASS publishes and closes the task; FAIL returns it to the same worker with the findings. Message the worker only when a receipt names a gate that still holds.
5. Hand the worker its next task. Do not poll for merge state.

To reject a parked delivery on review, use `task action=request_changes id=<task-id>`; it reopens the task for the same worker.

### Keeping other workers current

After the epic branch advances, run `factory action=sync_all_workers branch=epic/<slug>`. It rebases idle or stale worktrees, skips live and mid-rebase ones, and reports each skip; tell live workers to rebase at their next task start.


### Re-verify a rebased lane by patch-id

A review verdict belongs to a diff, not a SHA. At review, record the base, the tip and the lane's patch-id in the task note; after a rebase, rerun against the new base:

```bash
git diff <base>...factory/<worker> | git patch-id --stable
```

- **Same patch-id:** reuse the source-review verdict; inspect changed base dependencies for integration risk, then continue through current QA and CI gates.
- **Different patch-id:** review only what changed: `git range-diff <old-base>..<old-tip> <new-base>..factory/<worker>`.

A matching patch-id does not prove integration with the new base;
<!-- cas-src:begin -->
the merge queue run still does.
<!-- cas-src:else -->
the final gate on the epic PR still does.
<!-- cas-src:end -->
Runtime evidence and independent QA have separate reuse limits in [correction proof](correction-proof.md).

## Phase 3: Review (Shared Mode)

Workers commit directly and close their own tasks. Review their commits, then apply the closure pass before assigning more.

### Lessons must remove causes

Fix a recurring failure on the highest rung that fits — a default or auto-fix, then a seconds-long preflight, then a gate row, prose last. A lesson learned twice needs an open P0/P1 task that removes the cause, not another note. Tag lessons `operation:release-cut|spawn|report|epic-close`; starting that operation surfaces them.

## Handling Blockers

- Help resolve a blocked task or reassign it. For a reopened task, read its state and findings first; the merge verdict owns completion.
- Outbox replays can resend stale messages: check `task action=show` before acting on one.
- When several workers finish at once, merge each parked lane in one turn, let each verdict close or return its task, then apply the closure pass.

### Supervisor-run fixture scripts

When a walk needs records only your credentials can create, the worker drafts a guarded script in its worktree. Check that it verifies the target host, refuses on a colliding record, writes outputs mode 600 under the task artifact directory and prints no secrets; run it from the worker's worktree and send the worker the output path.

## Batch QA

In qa.mode=batch (the default) the release's batch QA round at the frozen tip is the only walk; per-task QA evidence applies only in qa.mode=per_task. The round runs after the fix-diff re-review and before enqueue: build a candidate at the frozen tip, walk the combined matrix ([epic flow walk](epic-flow-walk.md)) within `qa.pass_timeout_mins` and report every failing row in one ledger. Grade paid/account routes on server state and apply the [server read-back ruling](../../cas-qa-craft/references/evidence-ledger.md#server-read-back-and-harness-failures) before choosing product fixes: matching read-back with no visible product defect is product PASS plus a harness defect task, and does not block promotion. Missing read-back remains owed evidence; preserve native runner failures and required mechanical gates. Only a read-back mismatch or visible product defect enters the product-failure batch. Fix those failures in one batch and re-walk only those rows, once; a row still failing is reverted to the next release unless it loses operator data. Record `verification action=batch_receipt` with `task_ids`, `staging_sha`, `ledger_artifact_id`, `ledger_path` and `ledger_author` (who walked it and implemented no listed task); every listed delivery must be in that commit. In qa.mode=batch the release batch_receipt closes merged deliveries.

A lane not merged at freeze ships next release. After freeze, a new finding waits for the next board unless it loses data, hides state or hurts someone today; a fix for something hurting the operator ships alone
<!-- cas-src:begin -->
with `scripts/release.sh --hotfix`.
<!-- cas-src:else -->
as a hotfix release.
<!-- cas-src:end -->

## Interim queue precheck

<!-- cas-src:begin -->
Until a hosted precheck ships, run this diagnostic in a new, clean checkout at the reviewed or corrected SHA. Replace the source path, destination and SHA; the destination must not exist. Use a shell with Node, Cargo/nextest and Chromium system dependencies:

```bash
set -e
precheck_source=/absolute/path/to/cassy
precheck_dir=/absolute/path/to/precheck
precheck_sha=FULL_COMMIT_SHA
git clone --no-hardlinks "$precheck_source" "$precheck_dir"
cd "$precheck_dir"
git checkout --detach "$precheck_sha"
npm ci
npm --prefix hub-web ci
export PATH="$PWD/hub-web/node_modules/.bin:$PATH"
export NODE_PATH="$PWD/hub-web/node_modules"
export PLAYWRIGHT_BROWSERS_PATH="$PWD/target/playwright-browsers"
(cd hub-web && npx --no-install playwright install chromium)
node -e 'require("fs").accessSync(require("@playwright/test").chromium.executablePath())'
cargo nextest run --workspace --no-fail-fast
make -C cas-cli test-ci-tiers
```

Keep both test logs and their exit codes. This is not merged-tree proof; the queue still owns that. Do not substitute `assembly-proof.py`: its scrubbed environment cannot resolve the browser installation.
<!-- cas-src:else -->
This precheck belongs to the Cassy source repository. In other projects, CI
check-runs on the reviewed SHA (or the supervisor's integrated run of the
commands the project's CLAUDE.md or AGENTS.md names) are the precheck.
<!-- cas-src:end -->

## Phase 4: Complete

1. Verify every child is closed and merged with `factory action=epic_status id=<epic-id>`.
   <!-- cas-src:begin -->
   Then run the closeout once, in order: freeze scope; one full review; one batch fix; re-review only the fix diff; the [batch QA round](#batch-qa); enqueue. Fix tasks run only their touched tests. The merge queue is the one full-suite proof. Restart from a handoff only when your own context nears its limit, not at fixed phases.
   <!-- cas-src:else -->
   Then run the closeout once, in order: freeze scope; one full review; one batch fix; re-review only the fix diff; the [batch QA round](#batch-qa); merge or enqueue. Fix tasks run only their touched tests. The final gate (step 4) is the one full-suite proof. Restart from a handoff only when your own context nears its limit, not at fixed phases.
   <!-- cas-src:end -->
2. **Phase outcome met (plan metric receipt)** — require evidence that the measured phase target was achieved before advancing.
3. Hold the main merge until the assembled diff passes review and the final gate.
   <!-- cas-src:begin -->
   With a merge queue, enqueueing the reviewed PR is the hold.
   <!-- cas-src:end -->
4. Run the final assembled-tree gate:
   <!-- cas-src:begin -->
   On Cassy's release train, the existing CI archive/shards run the full Rust suite
   on each newest train tip; older runs are cancelled. Read the complete log
   artifact and failure list. A known red holds `train_merge` until green or a
   visible supervisor override for that run; do not treat a baseline red as safe.
   `scripts/release.sh` checks a green receipt for the starting train tip before
   version preparation. Do not run a local pre-cut full suite. The merge queue
   remains the final-tree gate, proving the version commit once. Workers run
   only touched tests before park; batch QA owns the release's user-path walk.
   <!-- cas-src:else -->
   the epic's single full-suite run on the tree that lands; skip it when CI already ran it. Workers run only the tests covering the lines they touched, before park. The full suite runs once on the landing tree: in the merge queue, else CI, else one supervisor run per epic. QA walks impacted journeys once per release batch. Read receipts; never rerun a passing check. Without check-runs on the exact default-branch SHA, run the project's full suite (the commands its CLAUDE.md or AGENTS.md names) on that SHA. A matching code tree reuses its durable PASS. On exit 0, note on the epic: `ASSEMBLY_PROOF: head=<epic tip sha> result=PASS command=<cmd> log=<path>`, with the log under `[factory] artifacts_root/<epic-id>/`.
   <!-- cas-src:end -->
5. Put every review, precheck and QA finding that needs worker action into one batch: a bounded epic-child fix-round task per worker, started together, each description carrying the finding, fix, acceptance criteria and proof command.
6. After a failed queue run, use the [interim queue precheck](#interim-queue-precheck) at the fix tip. Once the fix lands, rerun only the failed checks, then re-enqueue the PR.

   Never pipe the test run to `tail`; retain the complete output and the test command's exit status.
7. Once the gate passes, reclaim each lane's worktree:

   ```text
   factory action=worktree_merge id=<worker> task_id=<task-id> cleanup=true
   ```

   Queued landing integrates to base; merge locally only without a queue. If the tracked merge cannot act, ask the operator; do not use an untracked merge path.
8. Start the [post-release check](reporting-and-routing.md#post-release-check) when a deployed product's merge lands; close the epic and post release notes once it is clean.
9. Shut down the epic's workers by name once each has pushed and parked: `factory action=shutdown_workers worker_names=<worker>[,<worker>...]`.
