# Verifier evidence gate

The task-verifier reads this file when a task, or any child of an epic, has a
non-empty `demo_statement`, and for any acceptance criterion that states a
measurable claim (see Measurable claims below). It runs before the verifier
reads the close reason. Workers produce the evidence; the verifier consumes it.

## Demo-statement evidence mode

Fetch the task with `task action=show` and inspect fields and notes
before reading the close reason. A non-empty `demo_statement` requires Step
0A. For `task_type=epic`, first enumerate ParentChild children with
`task action=dep_list id=<epic-id>` and fetch every child, including
closed children: **any child** with a non-empty `demo_statement` requires the
epic evidence gate below, even when the epic's own demo is empty. Only tasks
with no demo and epics with neither their own nor any child demo skip Step 0A.

### Epic evidence prerequisites

For an epic with child demos, use `verification_type=epic` for every verdict.
Before Step 0A, require exactly one `Epic flow walk` note and
`~/.cas/artifacts/<epic-id>/LEDGER.md`; reject missing evidence with
`QA evidence required: epic has child demo_statement but no Epic flow walk note or LEDGER.md`.
Require a completed pass on the current assembled epic tip, a 60-minute budget,
and coverage mapping for every child demo. Reject a missing, duplicate,
running, stale-tip, or incomplete-coverage receipt; list omitted demos as owed
work. The note's cells/PASS/FAIL/NOT EXERCISED counts and label split must match
the ledger. Apply the same Step 0A REJECT table and capture judgments to this
single combined matrix, not separately per child. Check Contradictions across
child surfaces against the captures; a cross-child contradiction is a defect,
not a reason to accept individually passing child receipts. Do not rerun QA
from the close verifier or substitute the children's ledgers for the epic walk.

### Step 0A: Apply the QA evidence gate before any judgment

1. Locate `~/.cas/artifacts/<project-key>/<task-id>/LEDGER.md`. The task notes and eventual
   close reason should cite this same path; do not use a close reason as a
   substitute for the ledger. If the file is absent, reject with this exact
   summary: `QA evidence required: task has a demo_statement but no LEDGER.md`.
   Record that rejection with `verification action=add` and stop.
2. Run `cas qa ledger check <path> --task <id> --json`. The CLI, worker close
   and verifier share this parser. A nonzero exit rejects the ledger before
   capture judgment. List each returned line, row ID, field and message in the
   verification summary, record rejection, and stop. Use the candidate
   revision's command when reviewing a change to the parser itself.
3. Keep the JSON report as proof of the structural check. The executable owns
   grammar, counts, labels and task-derived breadth.

4. If the shared check passes, open every capture referenced by a `PASS` row
   with the available image-capable or terminal-capture reader. Judge the
   capture itself, not its filename, ledger prose, or source code. For each row
   answer whether a user performing that cell would see the stated `expected`
   outcome. If the capture does not show that outcome, downgrade that row to
   `FAIL` in the verification summary and state the capture path and reason;
   never silently leave it as `PASS` and do not edit the worker's ledger.
   Treat any evidence label weaker than the cell requires as `NOT EXERCISED: <reason>`,
   never as `PASS`.
5. `NOT EXERCISED` rows are owed work, not failures. List each such row in the
   verification summary with every row's final verdict, label, capture
   judgment, and the shared-check result.
6. Only after this evidence gate passes may you read the close reason and
   compare it with the acceptance criteria. A capture-downgraded row or a
   ledger `FAIL` is incomplete evidence even if the close reason says the task
   is complete.

### NOT EXERCISED rows: the supervisor decides

Neither approve nor reject a ledger because of its `NOT EXERCISED` rows. When
the evidence gate otherwise passes and at least one row is `NOT EXERCISED`
(including rows you downgraded for a weak label), stop and escalate: record
`status=error` with a summary that starts
`SUPERVISOR CALL: <n> NOT EXERCISED row(s)` and lists each row ID, cell, and
why it was not exercised, then say the same in your final output. The
supervisor decides whether the owed rows block the close.

## Measurable claims: same-command baseline and treatment

When an acceptance criterion states a measurable claim (performance, size,
count, rate), with or without a demo:

1. Restate it as a check that could fail: metric, command, threshold.
2. Require a baseline run on the delivery base and a treatment run on the
   delivery, from the same command with the same inputs, both pasted in task
   notes or saved under `~/.cas/artifacts/<project-key>/<task-id>/`. Different commands, a
   missing baseline, or a remembered one make the delta unattributable.
3. Record one line per claim in the verification summary: `VERIFIED` (the
   pair shows the claimed delta), `NOT VERIFIED` (no pair; name what is
   missing), or `INCONCLUSIVE` (the pair exists but variance or a changed
   input hides the delta). `VERIFIED` meets the criterion, and so does
   `NOT VERIFIED` when the close names an owner by design (a factory worker's
   full-suite execution follows the ownership rule below). Reject the
   rest, naming the claim and the missing or ambiguous run.

With a merge queue, the queue owns the full suite; without one, the full suite must be green on the exact SHA reaching the default branch, via CI check-runs on that SHA (preferred) or the supervisor's integrated run recorded on the epic; workers run targeted tests only. Read existing receipts; each check runs once.

## Scratch checkout lifetime

The checkout lives under the configured project scratch namespace at `verify/<task-id>/<run>/repo` (or `<cas root>/review-scratch/` when unset). A verdict or task close reclaims disposable `target/` output once the dispatch is resolved (or a terminal task has no active dispatch), with no lease or live user; existing GC/admission retries deferred cleanup. Source and its Git worktree registration retain the two-hour idle grace period before GC removes them. Checkouts with source changes are kept. This ownership record makes cleanup independent of whether your session exits normally.
