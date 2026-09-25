# Verifier evidence gate

The task-verifier reads this file only when a task, or any child of an epic,
has a non-empty `demo_statement`. It runs before the verifier reads the close
reason. Workers produce the ledger with this skill; the verifier consumes it.

## Demo-statement evidence mode

Fetch the task with `mcp__cas__task action=show` and inspect fields and notes
before reading the close reason. A non-empty `demo_statement` requires Step
0A. For `task_type=epic`, first enumerate ParentChild children with
`mcp__cas__task action=dep_list id=<epic-id>` and fetch every child, including
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

1. Locate `~/.cas/artifacts/<task-id>/LEDGER.md`. The task notes and eventual
   close reason should cite this same path; do not use a close reason as a
   substitute for the ledger. If the file is absent, reject with this exact
   summary: `QA evidence required: task has a demo_statement but no LEDGER.md`.
   Record that rejection with `mcp__cas__verification action=add` and stop.
2. Parse the ledger's row grammar exactly:
   `id | cell | expected | observed | verdict | label | evidence path | defect task`.
   Ignore prose and the required Constants vs expectation, Contradictions, and
   Honesty sections when counting rows. Trim cells before checking them.
3. Run every check in this machine-checkable REJECT table before opening a
   capture or making a user-outcome judgment. Any failed check rejects the
   ledger; list every failed check, row ID, and observed count/value in the
   verification summary, then stop.

   | Check | REJECT when |
   | --- | --- |
   | Required cells | Any data row has a blank `verdict` or `label` cell. |
   | Source inference | A row has `label=source-inferred` and `verdict=PASS`. |
   | Failed-cell ownership | A `verdict=FAIL` row has no non-blank `cas-*` defect task ID. |
   | Forbidden verdict | The standalone word `partial` occurs in any verdict cell, case-insensitively. |
   | Matrix breadth | Fewer than three data rows exist after the demo statement's happy-path row (the first matrix row). |
   | Headline counts | Header counts for `cells`, `PASS`, `FAIL`, or `NOT EXERCISED` do not equal the parsed row totals. |
   | PASS evidence | A `PASS` row has no evidence path, or its referenced capture is absent or unreadable. |

4. If the REJECT table passes, open every capture referenced by a `PASS` row
   with the available image-capable or terminal-capture reader. Judge the
   capture itself, not its filename, ledger prose, or source code. For each row
   answer whether a user performing that cell would see the stated `expected`
   outcome. If the capture does not show that outcome, downgrade that row to
   `FAIL` in the verification summary and state the capture path and reason;
   never silently leave it as `PASS` and do not edit the worker's ledger.
   Treat any evidence label weaker than the cell requires as `NOT EXERCISED`,
   never as `PASS`.
5. `NOT EXERCISED` rows are owed work, not failures. List each such row in the
   verification summary with every row's final verdict, label, capture
   judgment, and the REJECT-table result.
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
