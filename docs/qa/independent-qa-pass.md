# Independent QA and polish pass (design, cas-619f)

Status: draft for supervisor approval, 2026-09-23.

## Problem

Customers receive deliveries with bugs that anyone would spot by opening the
product, and deliveries that work but look unfinished. Today the implementer
is the only agent that runs a user-facing change before it merges. The
task-verifier reads the diff and the implementer's own QA ledger. It never
opens the product itself.

Code facts this design builds on:

- **Verification types.** `VerificationType` has only `Task` and `Epic`
  (`crates/cas-types/src/verification.rs:313`). No kind is typed QA.
- **No self-review check.** Nothing compares the verifier to
  `task.assignee`. When no live supervisor exists, a worker becomes its own
  dispatch owner (`close_ops.rs:3465`).
- **demo_statement.** It is enforced only at task creation for labelled tasks
  (`lifecycle.rs:273`). Nothing checks it at merge or close.
- **Merge path.** On the push-branch path, the worker's close parks the task
  as `AwaitingMerge` (`park_task_awaiting_merge`, `close_ops.rs:3501`). The
  supervisor then merges with `git merge --no-ff factory/<worker>` or with
  `worktree_merge`, and the task-verifier runs on the re-close, after the
  merge.
- **request_changes.** It is supervisor-only, requires `AwaitingMerge`, and
  keeps the assignee (`close_ops.rs:7634`).

So "before the supervisor merges" is the window between the park and the
merge. The pass has to live there.

## Decision summary

| Question | Decision |
| --- | --- |
| Trigger | A Cassy **QA dispatch** created when a user-facing task parks for merge. It is not a supervisor habit and not a task-verifier step. |
| Who runs it | A separate factory worker on the **taste** lane (`claude-opus-5-5`/high). It is never the implementer, and the rule is enforced in the store. |
| What it produces | A typed QA verdict (`qa_passes` row: passed/failed/waived) bound to the reviewed branch tip, plus a ledger with evidence. |
| What it blocks | `worktree_merge`, a supervisor's raw `git merge factory/<w>` (pre-tool guard), and the re-close (backstop). |
| Failure | A rejected QA verdict fires `request_changes` automatically, citing the ledger. The implementer keeps the task. |
| Cost cap | 45 min per round, 1 active pass per task, 3 rounds before escalation. Journeys are limited to those the diff touches. |

## 1. Which deliveries are user-facing

A non-epic task needs the pass when any of these hold:

1. `demo_statement` is non-empty.
2. It carries a label in `qa.user_facing_labels` (default
   `ui, hub, cli-ux, commander, frontend`).
3. Its delivery diff (the parked anchor base to the factory branch tip)
   touches a path that matches `qa.user_facing_paths`.

`qa.user_facing_paths` is a new config key holding repo-relative globs. It
defaults to generic web-surface patterns: `**/*.html`, `**/*.css`,
`**/*.scss`, `**/*.vue`, `**/*.svelte`, `**/*.tsx`, `**/*.jsx`, and
`**/DESIGN.md`. Projects extend it. cas-src adds `hub-web/**` and `site/**`.

Rust decides eligibility from config alone. It does not shell out to the
journey helper at park time. Journey selection happens inside the pass (§4).

A supervisor can waive the pass per task with `supervisor_override=true` and
a reason. The waiver is logged as a decision note and recorded on the pass as
`state=waived` with the supervisor as issuer, so the gates treat it as
recorded, not forgotten.

## 2. Trigger: the QA dispatch

When `park_task_awaiting_merge` parks an eligible task, Cassy does three
things in one transaction.

1. **Creates a `qa_passes` row.** Its fields are:
   - `id`, `task_id`, `round` (1-based)
   - `implementer_agent_id`: the assignee at park
   - `bound_head`: the factory branch tip being delivered
   - `branch`: the factory branch
   - `qa_task_id`
   - `reviewer_agent_id`: NULL until claimed
   - `state`: pending, claimed, passed, failed, timed_out, superseded, or
     waived
   - `requested_at`, `deadline_at`, `resolved_at`, `ledger_path`, `summary`

   A partial unique index allows one active (pending or claimed) pass per
   task.
2. **Creates the QA task.** Title: `QA pass: <delivery title> @<head8>`,
   label `qa-pass`, `execution_note=no-code`, linked `related` to the
   delivery, in the same epic. Its description carries the delivery's
   demo_statement, branch, head, the ledger path, and the cost cap.
3. **Wakes the supervisor** with a `<cas-qa-dispatch pass_id=…
   task_id=… qa_task_id=… bound_head=… deadline=…>` envelope. This uses a
   new wake class alongside `VerificationDispatch` and follows the same
   idempotent daemon-origin path.

The envelope tells the supervisor what to do:
`coordination action=spawn_workers lane=taste task_id=<qa_task_id>`.
Assigning an existing idle taste-lane worker also works, provided it is not
the implementer.

The worker's own close output changes too. MERGE REQUIRED now adds:
"Independent QA pass `<id>` dispatched. The merge waits for its verdict."

## 3. No self-review (enforced, not advised)

- **Starting the QA task.** `task start` or `claim` on a `qa-pass` task
  rejects any agent that is the delivery's `implementer_agent_id` or its
  current assignee. The same check runs when `spawn_workers task_id=` would
  pre-assign the task to one of those agents.
- **Recording the verdict.** A QA verdict (`verification action=qa_record
  task_id=<delivery> status=approved|rejected summary=… issues=…
  ledger_path=…`) is accepted only from the agent that claimed the active
  pass, and only when that agent is not the implementer.
  - This is the only verdict a worker role may record, and it is a new,
    explicit authority path. The general rule stays in place: workers cannot
    attest their own work.
  - The supervisor cannot record a `passed` QA verdict. It can only waive
    (§1), which is logged and visibly different from a pass.
- **Unrecognised verification types.** `verification_type` strings Cassy does
  not recognise stop silently mapping to `Task`
  (`verification_tools.rs:509`). An unknown type is now an error, and
  `verification_type=qa` on `add` names `qa_record` instead, so a typo cannot
  record a task verdict.

### Why the verdict lives in `qa_passes`, not `verifications`

Two facts make a `VerificationType::Qa` row in the shared `verifications`
table unsafe:

- A non-epic close reads the untyped `get_latest_for_task`
  (`close_ops.rs:4998`), so a QA approval would count as the task-verifier's
  verdict.
- Older binaries parse an unknown type string as `Task`
  (`verification_store.rs:325`, `unwrap_or_default`). A mixed-version fleet
  sharing one `cas.db` would read a QA approval as a task approval even
  after the new binary filters by type.

So the typed QA verdict is its own record: the `qa_passes` row carries
`verdict` (approved, rejected, waived), `reviewer_agent_id`, `summary`,
`issues` (JSON) and `ledger_path`. No existing reader ever sees it.

## 4. What the QA worker does

This is a new builtin skill section, `cas-qa-craft` → "Independent pass",
reusing its matrix and ledger discipline. Order of work:

1. **Build and serve the delivered branch** at `bound_head` in the QA
   worker's own worktree. Use the project's real build: for hub-web, `npm run
   build` and then serve `dist`. Record the build command and URL in the
   ledger. A build failure is a Blocking finding.
2. **Walk the journeys.** Run `scripts/journeys-for-diff.py <base>
   <bound_head>` (cas-9be7) and add the journeys the demo_statement names.
   Walk each one from its real entry point to the user's goal with
   `scripts/journey-eval.sh <dir> --grep <ID>`. Then judge the experience:
   dead ends, copy, steps, lost context, waits (scored 0–3). Where the
   catalog has no suite, walk the journey by hand with Playwright 1.63
   (`playwright-cli`) and record a trace.
3. **Check correctness paths.** Walk the demo_statement end to end, then the
   adjacent paths:
   - empty, loading, and error states
   - long content
   - phone width 390px
   - dark mode
   - keyboard only
   - reduced motion

   At most 8 matrix cells (the same cap as cas-qa-craft).
4. **Check polish.**
   - Run `node scripts/visual-qa.mjs --strict --artifact-dir <dir>/visual-qa
     <url>…`, which covers desktop 1280 and phone 390 in light and dark.
   - Score the `cas-ui-craft` critique rubric (distinctiveness, fit,
     hierarchy, craft, accessibility), with one evidence sentence per score.
   - Work through the polish checklist:
     - DESIGN.md and token consistency
     - spacing and alignment
     - typography
     - copy and microcopy
     - empty, loading, error, and disabled states
     - focus rings
     - contrast
     - truncation and overflow
     - motion
5. **Write the ledger and record the verdict** (§5, §6).

The QA worker never edits the delivery. Fixes belong to the implementer.

## 5. Finding format

Everything goes in `~/.cas/artifacts/<delivery-task>/qa/round-<n>/`, in the
cas-c3b8 evidence bundle layout (shape pending from happy-gazelle-77; this
section adopts it verbatim once received). `LEDGER.md` has these sections:

- **Header:** pass id, reviewer, implementer, branch, `bound_head`, build
  command, served URL, and the Playwright version.
- **Journeys:** the cas-9be7 row format, unchanged:
  `| ID | Run | Dead end | Copy | Steps | Context | Waits | Severity | Findings / tasks |`
- **Correctness:**
  `| # | Path | Viewport · scheme | Expected | Actual | Severity | Evidence |`
- **Polish:** the rubric table (dimension, score, evidence sentence), the
  checklist with a pass or fail per item, and the `visual-qa.mjs --strict`
  verdict line.
- **Evidence** for every finding: `trace action N` in
  `journeys/<ID>/trace.zip` or `correctness/<n>/trace.zip`, plus one
  screenshot path. A finding without both is not a finding.
- **Severity** reuses cas-9be7: Blocking, High, Normal, or Note.

## 6. Verdict and routing

The verdict is **rejected** when any of these hold:

- a Blocking or High finding (correctness or journey)
- `visual-qa.mjs --strict` fails
- a rubric score of 0
- distinctiveness, fit, or hierarchy below 4 on a public surface
- craft or accessibility below 3

Otherwise the verdict is **approved**. Normal and Note findings are listed
in the verdict summary, and on approval they become follow-up tasks.

When the verdict is recorded:

- **Approved.** Cassy records the verdict on the pass (`state=passed`,
  reviewer, summary, issues, ledger path), closes the QA task, and wakes the
  supervisor with "QA passed for `<task>` @`<head8>`. Merge."
- **Rejected.** Cassy resolves the pass as `failed` and calls
  `request_changes_for_parked_delivery` itself, using the store function the
  supervisor tool already uses. It passes the reason "Independent QA
  (round n) found k issues: `<ledger path>`" and the Blocking and High rows.
  The implementer is woken and keeps the task. The next park opens round
  n+1.
- **Round 4.** Instead of opening another round, Cassy escalates to the
  supervisor with a blocker envelope.

`bound_head` drift supersedes the pass. When the factory branch tip moves
while a pass is pending or claimed, the pass becomes `superseded`, and the
next park issues a fresh round. This is the same rule verification
dispatches use for repository proof drift.

## 7. Gates (where "cannot merge" is enforced)

1. **`worktree_merge task_id=<t>`.** It refuses an eligible task unless a
   pass in state `passed` or `waived` exists and its `bound_head` equals the
   branch tip being merged.
2. **Supervisor pre-tool Bash guard.** It denies `git merge [...]
   factory/<w>` when `factory/<w>` is the parked branch of an eligible
   `AwaitingMerge` task that lacks that record. The denial names the pass id
   and its state. Raw git is the documented merge path today, so this is the
   primary enforcement point, not a nicety.
3. **Re-close backstop.** An eligible task's close requires a `passed` or
   `waived` pass whose `bound_head` is an ancestor of the target branch. This catches merges made outside a Claude Code hook, such as
   another harness or a shell. The rejection message carries the exact
   command to dispatch a pass.

The task-verifier's Step 0A ledger check stays as it is. It governs the
implementer's own evidence, which cas-0cd5 enforces. The independent pass is
a second, separate pair of eyes.

## 8. Cost cap

New `qa.*` config keys:

| Key | Default | Meaning |
| --- | --- | --- |
| `qa.pass_timeout_mins` | 45 | Deadline for one round. On expiry the pass is `timed_out` and the supervisor is woken to redispatch or waive. |
| `qa.max_rounds` | 3 | Rounds before escalation. |
| `qa.max_matrix_cells` | 8 | Correctness paths per round. |

One active pass per task. Journeys are limited to those the diff touches,
plus those the demo_statement names. The QA worker is a normal taste-lane
spawn and is retired after its QA task closes.

## 9. Tests (proof targets: factory ops, verification)

- **Park.** Parking an eligible task creates exactly one pass, one QA task,
  and one wake. An ineligible task creates none. A re-park with the same
  head is idempotent.
- **No self-review.** The implementer cannot start the QA task or record a
  QA verdict. The supervisor cannot record a `passed` QA verdict. An unknown
  `verification_type` is rejected.
- **Verdict routing.** A rejected verdict parks the delivery back to open
  with its assignee kept and the ledger cited. An approved verdict writes a
  typed verdict on the pass. Head drift supersedes the pass.
- **Gates.** `worktree_merge` refuses without a QA pass and accepts with
  one. The pre-tool guard denies `git merge factory/<w>`. The re-close
  backstop rejects and then accepts.
- **Rounds and timeout.** The fourth rejection escalates instead of opening
  a round. An expired deadline times out the pass.

## 10. Demonstration

One real hub-web change goes through the whole loop in this epic: park, QA
dispatch, taste-lane pass with journeys, polish and screenshots, a
request_changes round when findings exist, then an approved pass and merge.
Receipts go under that task's `qa/` artifacts.

## Out of scope

- Auto-fixing findings.
- Running the pass on epics.
- Journey catalogs for projects other than cas-src (cas-9be7 seeds hub-web).
