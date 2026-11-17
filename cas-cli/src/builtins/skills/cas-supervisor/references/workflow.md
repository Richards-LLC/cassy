# Workflow — Worker Modes, Phases, Blockers

Contents: [Phase 1: Plan](#phase-1-plan) · [Brief fields](#brief-fields-no-spawn-without-them) · [Phase 2: Coordinate](#phase-2-coordinate) · [Phase 3: Merge and sync](#phase-3-merge-and-sync-isolated-mode) · [Re-verify after a rebase](#re-verify-a-rebased-lane-by-patch-id) · [Blockers](#handling-blockers) · [Batch QA](#batch-qa) · [Queue precheck](#queue-precheck) · [Phase 4: Complete](#phase-4-complete)

## Worker modes

Spawn with `isolate=true`: each worker gets its own worktree and branch, and lanes merge through `worktree_merge`. Shared mode (the default) puts every worker on one checkout and HEAD, so commits can land on another worker's branch; use it only for one worker or read-mostly work.

## Phase 1: Plan

1. Search for prior art: closed epics (`task action=list task_type=epic status=closed`), memories (`search action=search query="<keywords>" doc_type=entry limit=10`) and code (`doc_type=code` or a grep).
2. Create the EPIC with `task action=create task_type=epic title="<epic>"`, then break it down with [planning.md](planning.md).

**Severity × reach for review findings.** Every finding names its impact, reach and journey with `severity:money|data_loss|broken|awkward|cosmetic`, `reach:main|secondary|theoretical` and `journey:<id>` labels plus `repro` and `affected` for observed failures; see `docs/qa/triage.md`. Priority derives from those labels, so do not hand-pick P0: a code-read finding is theoretical P4 backlog, a reproduced main-journey failure outranks it, and money or data loss on a declared main journey is the only theoretical exception (P2). Declare main journeys in `docs/qa/journeys.yaml` and set `factory.focus_mode` for inherited operator intent. A nit never reopens a merged task. At release prep, `cas qa polish --epic <id>` lists warnings by file; batch observed improvements or leave them in the backlog.

**Verification tiers.** Close records the tier. Tier 0 (docs, non-agent Markdown, `CHANGELOG.md`, or tests and journeys only) gets no independent verifier and walks no journeys. Tier 1 gets its own pre-merge verifier in every QA mode. Tier 2 (guards and hooks, `.github/workflows`, `scripts/release*`, integration and close-gate code) also needs an `Adversarial probe:` ledger row. The release cut combines existing merge approvals and the green frozen-train receipt; it needs no separate review ledger.

**Standalone follow-up work.** For one loose task after an epic closes, create the task and spawn onto it rather than a single-child epic; an open, unassigned `task_id` authorizes the spawn:

```text
task action=create title="..." description="..." risk=none
factory action=spawn_workers count=1 lane=standard isolate=true task_id=<task-id>
```

### Brief fields: no spawn without them

Pass a stakeholder ask's Slack permalink or GitHub issue/PR URL as `external_ref`. Creation and staffing refuse a source a peer holds and name the owner; open a second task on it with `supervisor_override=true reason="<agreement>"` only once that peer agrees.

A worker cannot recover a goal or a proof method the brief never named, so fill all four fields (`task action=update`) before you spawn onto or assign a task:

| Field | Where it lives |
|---|---|
| Goal: what is true when the task is done | `description` |
| Scope: files to edit, and what is off-limits | `description` (or `design`) |
| Acceptance: the observable pass condition | `acceptance_criteria` |
| Proof expected: the touched tests; never name suites or evals | `proof_targets`, `demo_statement`, or the named command or grep in `acceptance_criteria` |

A brief names a full or stress run only as a named, justified exception.

For rule-driven work, cite each rule id and its verbatim stakeholder scope. Declare known causal anchors with `Rule-driven sites:` (`rule_id`, `path`, `anchor`) or require them in the close receipt; code and tests carry `cas-rule: <id> scope: <JSON string>` comments. Unrelated work does not inherit the rule's scope.

Tasks with `qa.user_facing_labels` need a `demo_statement` (`As a <user>, I <do X> and see <Y>`). Human-facing output follows [DESIGN.md first](../../design-spec/references/design-md-first.md); name the target project only when it is not the worker's checkout, and review the close's `design:` line against the output.

A worker's close reason starts `PASS`, or `ISSUES` with the known defects, then the commit SHA and the check it ran. Ask once for a missing SHA or check; a second miss goes through the [retry policy](worker-recovery.md#retry-policy-by-failure-mode).

## Phase 2: Coordinate

1. Group tasks that touch the same files into one lane; the worker count is the number of parallel lanes. Spawn one isolated worker per lane, pre-assigned to its first task, with `lane=<light|standard|taste|heavy>` (the registry reports any fallback) or a complete recipe ([model-selection.md](model-selection.md)):

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
2. Assign that lane's later tasks with `task action=update id=<id> assignee=<worker>`. Pin the epic in the TUI with `factory action=focus_epic id=<epic-id>` (`clear=true` when it wraps).
3. **Own the next exit rung.** The brief lives in the task record; send no assignment message. Directed messages wake the recipient (the send result says whether it woke); receipts, acknowledgements, broadcasts and wake=false wait in the inbox on the next turn. Wait on a concrete event with `coordination action=remind remind_event=<event> remind_message="..."` or the harness background-task notice.

### Resuming an existing EPIC

Workers from previous sessions are gone; stale DB records are not live processes. Run preflight ([preflight.md](preflight.md)), apply the [closure pass](#wall-clock-is-a-resource), and spawn fresh workers only for the work it selects.

## Phase 3: Merge and Sync (Isolated Mode)

`factory action=worktree_merge id=<worker> task_id=<task-id>` is the only worker merge path: it resolves the target from task state and keeps tracking, leases and cleanup consistent. Its refusals name the gate. Lane CI admits lanes;
<!-- cas-src:begin -->
the merge queue is the Rust compile and suite proof.
<!-- cas-src:else -->
the Phase 4 final gate is the full suite proof.
<!-- cas-src:end -->
For optional advisory Spec/Standards reviews beside the verifier, use [cas-shadow-review](../../cas-shadow-review/SKILL.md).

### Wall-clock is a resource

A green push alone is not ready: a test-first checkpoint can be green and incomplete. Apply this closure pass every turn, including startup and simultaneous completions:

1. Inspect pushed lanes, `READY TO MERGE`, `awaiting_merge` and delivered corrections. Merge every reviewed lane whose QA gates hold and plan target is met; one lane's proof never delays another ready merge.
2. Give each held delivery a named blocker, next owner and event or reminder: missing candidate, review, QA, red CI or merge conflict. Staff those gaps before new noncritical work; for a correction, use [correction proof](correction-proof.md).
3. If every delivery waits on an external event, dispatch only already-authorized independent work. An idle worker is valid; keeping one busy is no reason to expand scope.

Report the oldest held delivery and its next action, not utilization. The default merge keeps the worker checkout; `cleanup=true` belongs at end of lane. A merge into an `epic/` branch runs no local suite.
<!-- cas-src:begin -->
When the epic PR leaves the merge queue unmerged, one `merge_queue_ejected` wake names the PR and failing job.
<!-- cas-src:end -->
A `focus_epic` pin is a display filter, never merge authority.

### Required merge-review discipline

Before landing a lane, check that the worker's capped `check: PASS <sha>` and targeted `test: PASS <sha> <package> <filter> <count>` receipts name the delivered commit and affected scope; the full build and suite run once, at the Phase 4 final gate. For public surfaces, score distinctiveness, fit and hierarchy 1–5 with cas-codebase-design; the floor is 4/5. A script used on several platforms needs more than Linux proof unless a fixture covers the difference.

G-61 changed-test stability covers Jest, Vitest and Playwright as well as Rust: the worker runs `cas factory worker-check --cas-root "$CAS_ROOT" -- jest|vitest|playwright <files>`, and the refusal names the exact command. Never waive it as cargo-only.

- **Contract changes first.** If the diff changes a public contract (API shape, persisted field, CLI/MCP response, or behavior callers rely on), search for tests that still pin the old contract, for example `git grep -n '<old contract token>' -- '*test*'`, and update or reject the lane.
- **Lane CI.** The merge refuses red, pending or unvalidated code CI and names the gate; a pending refusal arms an exact-SHA retry that merges on green. Override red or unvalidated CI with `supervisor_override=true` and a reason only for a CI defect, never to skip a real failure.

`force=true` merges a dirty worktree; never use it over a worker's uncommitted work without the operator. `allow_trunk=true` is only for a genuine missing-target fallback to trunk.

**READY TO MERGE / MERGE REQUIRED:**

1. Merge first; a worker's own message is not a trigger, and green lane CI without a park is not ready. Hold deliveries below their plan target and their dependents; ship independent work. Confirm the tip and target with `factory action=epic_status id=<focused-epic>` or `task action=show id=<task-id>`.
2. In qa.mode=per_task only, a user-facing delivery parks after its own QA evidence bundle validates; send the worker the command the refusal names. When only a deployment can sign in, set `cas config set qa.deployed_origins <origin>` and cite a `deployed` bundle. On `INDEPENDENT QA DISPATCHED` or a `<cas-qa-dispatch>` wake, spawn a reviewer who is not the implementer, on the account with the most room and the cheapest recipe that has passed this kind of review (`factory action=spawn_workers count=1 lane=standard isolate=true task_id=<qa-task>`); merge and close wait for its `qa_record` on the exact tip. Waive only with a logged reason (`verification action=qa_waive task_id=<task-id> summary="..."`). Code already merged without parking gets no round: close it with `commit_receipt=<merged sha>`.
3. The merge starts the close. For a delivery that needs its own verifier, `worktree_merge` prepares a candidate and opens a pre-merge dispatch, and Cassy starts its daemon-owned verifier on it; send the worker nothing. A verifier-stopped notice names the retry and the direct-verdict route. PASS publishes and closes the task; FAIL returns it to the same worker with the findings. Message the worker only when a receipt names a gate that still holds.

To reject a parked delivery on review, use `task action=request_changes id=<task-id>`; it reopens the task for the same worker.

### Keeping other workers current

After the epic branch advances, run `factory action=sync_all_workers branch=epic/<slug>`. It rebases idle or stale worktrees and reports each live or mid-rebase one it skips; tell those workers to rebase at their next task start.

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

Fix a recurring failure on the highest rung that fits: a default or auto-fix, then a seconds-long preflight, then a gate row, prose last. A lesson learned twice needs an open P0/P1 task that removes the cause, not another note. Tag lessons `operation:release-cut|spawn|report|epic-close`; starting that operation surfaces them.

## Handling Blockers

- Help resolve a blocked task or reassign it. For a reopened task, read its state and findings first; the merge verdict owns completion.
- Outbox replays can resend stale messages: check `task action=show` before acting on one.
- When several workers finish at once, merge each parked lane in one turn, let each verdict close or return its task, then apply the closure pass.

### Supervisor-run fixture scripts

When a walk needs records only your credentials can create, the worker drafts a guarded script in its worktree. Check that it verifies the target host, refuses on a colliding record, writes outputs mode 600 under the task artifact directory and prints no secrets; run it from the worker's worktree and send the worker the output path.

## Batch QA

In qa.mode=batch (the default) the release's batch QA round at the frozen tip is the only walk; per-task QA evidence applies only in qa.mode=per_task. The round runs after the fix-diff re-review and before enqueue: build a candidate at the frozen tip, walk the combined matrix ([epic flow walk](epic-flow-walk.md)) within `qa.pass_timeout_mins` and report every failing row in one ledger.

Grade paid/account routes on server state and apply the [server read-back ruling](../../cas-qa-craft/references/evidence-ledger.md#server-read-back-and-harness-failures) before choosing product fixes: a matching read-back with no visible product defect is product PASS plus a harness defect task and does not block promotion; missing read-back remains owed evidence. Only a read-back mismatch or visible product defect enters the product-failure batch. Fix those in one batch and re-walk only those rows, once; a row still failing is reverted to the next release unless it loses operator data.

Record `verification action=batch_receipt` with `task_ids`, `staging_sha`, `ledger_artifact_id`, `ledger_path` and `ledger_author` (who walked it and implemented no listed task); every listed delivery must be in that commit. A QA receipt cannot replace a missing code approval.

A lane not merged at freeze ships next release. After freeze, a new finding waits for the next board unless it loses data, hides state or hurts someone today; a fix for something hurting the operator ships alone
<!-- cas-src:begin -->
with `scripts/release.sh --hotfix`.
<!-- cas-src:else -->
as a hotfix release.
<!-- cas-src:end -->

Lift a release freeze on your own train with `factory action=train_freeze id=<train> decision=lift reason="<why>"` only when a Cassy gate blocks the cut on a green frozen tip or the operator asked for new work in the release, never as a CI hold. The lift is recorded on the train and sent to the operator; a freeze the operator applied in Commander (or one from before supervisor lifts existed) stays theirs unless they set `release.supervisor_lifts_operator_freeze`.

## Queue precheck

<!-- cas-src:begin -->
To prove a reviewed or corrected SHA against the merge queue's required job set before enqueueing, run `cas release precheck <full-sha>`. It pushes `precheck/<full-sha>`, dispatches `ci.yml` on hosted runners and records a receipt; the checkout needs only Git, authenticated `gh` and Python. Read the result with `cas release precheck <full-sha> --status`; a completed red run exits nonzero. This is not merged-tree proof; the queue still owns that.
<!-- cas-src:else -->
This precheck belongs to the Cassy source repository. In other projects, CI
check-runs on the reviewed SHA (or the supervisor's integrated run of the
commands the project's CLAUDE.md or AGENTS.md names) are the precheck.
<!-- cas-src:end -->

## Phase 4: Complete

1. Verify every child is closed and merged with `factory action=epic_status id=<epic-id>`, then run the closeout once, in order: freeze scope; one full review; one batch fix; re-review only the fix diff; the [batch QA round](#batch-qa); enqueue (merge directly only without a queue). Fix tasks run only their touched tests; the final gate (step 4) is the one full-suite proof. Restart from a handoff only when your own context nears its limit, not at fixed phases.
2. **Phase outcome met (plan metric receipt)**: require evidence that the measured phase target was achieved before advancing.
3. Hold the main merge until the assembled diff passes review and the final gate.
   <!-- cas-src:begin -->
   With a merge queue, enqueueing the reviewed PR is the hold.
   <!-- cas-src:end -->
4. Run the final assembled-tree gate:
   <!-- cas-src:begin -->
   On Cassy's release train, the daemon dispatches the full set (`full-validation.yml`: six Rust shards, web, journeys and doctests) once per frozen train tip; a repair tip gets one run of its own, and a run that stays queued with zero jobs for ninety seconds is replaced once at the same head. Read the complete log artifact and failure list. A known red holds `train_merge` until green or a visible supervisor override for that run; do not treat a baseline red as safe. A repair targeting the frozen train merges once its lane's two no-build checks pass, without waiting for its unfinished Rust impact jobs (a finished red one still refuses), because the frozen tip's full set supplies its Rust proof. `scripts/release.sh` requires a green full-set receipt for the train tip before version preparation, so run no local pre-cut full suite. The merge queue remains the final-tree gate and proves the version commit once. Workers run only touched tests before park; batch QA owns the release's user-path walk.
   <!-- cas-src:else -->
   the epic's single full-suite run on the tree that lands; skip it when CI already ran it. Workers run only the tests covering the lines they touched, before park. The full suite runs once on the landing tree: in the merge queue, else CI, else one supervisor run per epic. QA walks impacted journeys once per release batch. Read receipts; never rerun a passing check. Without check-runs on the exact default-branch SHA, run the project's full suite (the commands its CLAUDE.md or AGENTS.md names) on that SHA. A matching code tree reuses its durable PASS. On exit 0, note on the epic: `ASSEMBLY_PROOF: head=<epic tip sha> result=PASS command=<cmd> log=<path>`, with the log under `[factory] artifacts_root/<epic-id>/`.
   <!-- cas-src:end -->
5. Put every review, precheck and QA finding that needs worker action into one batch: a bounded epic-child fix-round task per worker, started together, each description carrying the finding, fix, acceptance criteria and proof command.
6. After a failed queue run, run the [queue precheck](#queue-precheck) at the fix tip. Once the fix lands, rerun only the failed checks, then re-enqueue the PR. Keep the complete test output and exit status; a `tail` pipe loses both.
7. Once the gate passes, reclaim each lane's worktree with `factory action=worktree_merge id=<worker> task_id=<task-id> cleanup=true`. Queued landing integrates to base; merge locally only without a queue. If the tracked merge cannot act, ask the operator rather than use an untracked merge path.
8. Start the [post-release check](reporting-and-routing.md#post-release-check) when a deployed product's merge lands; close the epic and post release notes once it is clean.
9. Shut down the epic's workers by name once each has pushed and parked: `factory action=shutdown_workers worker_names=<worker>[,<worker>...]`.
