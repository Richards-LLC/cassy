# Verifier evidence gate

The task-verifier applies Code review procedure and Quality checklist to every
review. Demo-statement and measurable-claim sections apply when the task, or a
child of its epic, has that requirement; those evidence gates run before the
close reason is read. Workers produce evidence; the verifier consumes it.

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
3. Keep the JSON report. Schema 2 requires cited `path:line` expectations and
   `visible:`, `terminal:` or `http:` outcomes; DB-only PASSes fail. Legacy receipts remain valid.

4. Open every PASS capture and its cited source line. Judge the visible result
   against that source: database rows, charges or a stored draft cannot prove what
   the user sees. An unsupported expectation is a spec question. Record misleading
   PASSes as FAIL with capture path and reason; never edit the worker's ledger.
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
`status=supervisor_call` with a summary that starts
`SUPERVISOR CALL: <n> NOT EXERCISED row(s)` and lists each row ID, cell, and
why it was not exercised, then say the same in your final output. The
supervisor decides whether the owed rows block the close, then answers with
`verification action=add` on the same `dispatch_id`, the final `approved` or
`rejected` status, and the answer in `summary`. The dispatch stays open; no
re-dispatch is needed.

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

Workers run only the tests covering the lines they touched, before park. The full suite runs once on the landing tree: in the merge queue, else CI, else one supervisor run per epic. QA walks impacted journeys once per release batch. Read receipts; never rerun a passing check.

## Scratch checkout lifetime

The checkout lives under the configured project scratch namespace at `verify/<task-id>/<run>/repo` (or `<cas root>/review-scratch/` when unset). A verdict or task close reclaims disposable `target/` output once the dispatch is resolved (or a terminal task has no active dispatch), with no lease or live user; existing GC/admission retries deferred cleanup. Source and its Git worktree registration retain the two-hour idle grace period before GC removes them. Checkouts with source changes are kept. This ownership record makes cleanup independent of whether your session exits normally.

## Candidate fixture isolation

Before running a candidate `serve` or any stateful command, create a disposable
project with an empty `.cas` directory and a disposable HOME outside the source
checkout. Set `CAS_ROOT` to that fixture `.cas`, `HOME` to the disposable home,
and `XDG_CONFIG_HOME` beneath it for every candidate subprocess, including the
fixture's `cas init`. Initialize the fixture store before serving it and record
the resolved store path with the command receipt. A scratch Git worktree still
discovers the main checkout's live `.cas`; unsetting `CAS_ROOT` does not isolate
it. Keep these overrides scoped to candidate subprocesses so your authenticated
reviewer MCP session retains its factory binding.

## Counted verifier lane

To run the task's tests yourself, use the counted verifier lane at the dispatch's bound head, never plain Cargo, which builds outside the host's builder cap: `cas factory verifier-test --cas-root <cas root> --task-id <task-id> --dispatch-id <dispatch-id> --head <full bound head SHA> -- nextest run -p <crate> [--lib|--test <harness>] -E 'test(module::name)'`. Omit `--dispatch-id` only if the task has no dispatch. This command creates its managed task/dispatch checkout, resolves Zig and a private target, and holds a review lease across the existing capped runner. A "Waiting for build admission" refusal means retry later; if it persists, mark the item NOT VERIFIED with that reason instead of building another way.

## Rule candidates

On a rejection that teaches a reusable lesson, run `rule action=check_similar content="<lesson>"`, then `rule action=observe content="<lesson>" tags="category:<cat>" source_ids="<task-id>:<verification-id>"`, adding `id=<candidate-id>` when check_similar lists a candidate for the same lesson. Cassy counts the observation and raises the candidate to the supervisor and operator once different tasks repeat it.

## Code review procedure

### Step 4: Read every changed file in full

Run `rule action=list` for the project rules. Read each changed file completely. In the changed code, reject:

- TODO/FIXME/XXX/HACK markers, and `todo!()`, `unimplemented!()`, `raise NotImplementedError`, `throw new Error('Not implemented')`;
- temporal shortcuts ("for now", "temporarily", "placeholder") that leave an acceptance-criteria item undone;
- new `@ts-ignore`, `#[allow(dead_code)]`, `# type: ignore` without a stated reason;
- code that duplicates existing functionality (search before approving).

### Step 5: Structural checks with receipts

Run commands, do not just opine. Use `ast-grep` on changed files, for example `ast-grep --lang rust -p '$EXPR.unwrap()' <file>`, `ast-grep --lang typescript -p '$EXPR as any' <file>`, `ast-grep --lang typescript -p 'catch ($ERR) {}' <file>`. If `ast-grep` is unavailable or cannot parse the language, fall back to:

```bash
rg -n 'unwrap\(\)|todo!|unimplemented!|console\.log|@ts-(ignore|expect-error)|except:' <changed_file>
```

Back every finding with a command output or an exact line reference.

To run the task's tests yourself, use the counted verifier lane, never plain Cargo, as `verifier-evidence-gate.md` describes under Counted verifier lane.

### Step 6: Impact, wiring and co-changes (blocking when missing)

- A changed signature, field, export or public API: search its callers (`rg '<name>'`) and confirm they were updated.
- Every new function, route, handler, command, tool, migration or config field is reachable: it has a call site or registration outside its definition. Test helpers, derive-required impls and exported library items are exempt.
- Files that change together did: tests for changed logic, a migration for a schema change, route registration for a new endpoint, defaults and docs for new config.
- Proof matches blast radius (ladder in the `verify-before-claim` skill: read → targeted test → integration or real run → user-path walk). A `risk=blast-radius` task needs each `proof_targets` entry at rung 3 or owned by `ASSEMBLY_PROOF`, plus one safety fact: what could break and why it did not. A missing safety fact is `incomplete_close_reason`, as is a "full suite" claim that ran fewer files or tests than the suite runs.

### Step 7: Honor the task's `execution_note`

- `test-first`: the delivery must add or change at least one test case, in a new or an existing file, and the task must record a run in which that case failed before the fix. Never require a new test file or a number of tests: a few tests that each prove one behaviour are the target. Added cases show in:

  ```bash
  git diff "$BASE" HEAD | grep -nE '^\+.*(#\[(tokio::)?test|fn test_|\b(it|test)\(|def test_)'
  ```

  A case whose body changed counts too, so read the test hunks before rejecting. If no test case was added or changed, or no failing-first run is recorded, reject with "REJECTED (test-first posture): no failing-first test case in the diff."
- `characterization-first`: expect added or changed tests that pin current behaviour before the change; if none, reject naming the posture.
- `additive-only`, `value-only`, `no-code`, or none: nothing to check here; the close gate enforces those.

## Quality checklist

Compare the change with how neighbouring code solves the same problem (`rg '<pattern>' -l`). Then look for, and report only with evidence:

- correctness: edge cases, error propagation, races;
- design: follows existing patterns, sensible abstraction;
- performance: redundant work, unbounded queries, quadratic loops;
- security: input validation at boundaries, parameterized queries, secrets;
- tests: each added or changed test against cas-tdd's `Test hygiene` rules: a CSS or source-text assertion, a real sleep or back-off, a dynamic `import()` in a test body, expensive setup rebuilt per test, a screenshot-only end-to-end test, or a deleted "duplicate" that another test imports.

Rate each finding `critical`, `high`, `medium`, or `low`; name its file and line, observed effect, evidence, and exact fix. Apply the verdict threshold below. Skip style nits and sweeping refactors.

## Plan conformance

Run `task action=dep_list id=<task-id>` to discover ParentChild dependencies. Read each parent epic with `task action=show id=<epic-id> full=true`, including its acceptance criteria, design and decision notes. Read the relevant sections of any plan document named by the task or epic, fetching cited artifacts and reading their contents. Follow intermediate parents to the epic when necessary. A task card and its close reason are not substitutes for the plan.

Identify the fields, behaviours and measurable targets that the plan assigns to this delivery or phase. Map each obligation to implementation and proof in the delivery found in Step 3; finalize this check after reading the changed files. Name each missing field or behaviour individually, with its plan section and delivery evidence. Compare every applicable plan target with the actual result, including targets recorded in delivered evidence even when the card only asks to run or report a measurement. Apply Step 0's measurable-claims evidence gate to these targets too. Passing tests or completing the card does not prove the plan's outcome.

Respect the planned division of work: a child implementing its assigned slice need not implement the entire epic. Cite the plan section or authoritative scope decision assigning other obligations elsewhere. A worker's narrower card, follow-up, renamed field, or lowered threshold is not an approved plan revision. If an authorized operator decision revised the plan, cite that decision and check against the revised obligations; never silently accept a scope reduction.

Record one explicit `PLAN-FIT:` line in the verdict summary:

- **VERIFIED**: every applicable planned obligation is implemented and proven; cite the plan sections and evidence, including a receipt for each measurable target.
- **NOT VERIFIED**: the delivery or card is narrower than its assigned plan scope, a planned field or behaviour is omitted, or an applicable plan target was missed. Set `status=rejected`, name every omission or missed target as blocking, and include a supervisor decision question: restore the planned scope/outcome, or obtain an authorized plan revision? Do not broaden the worker's assignment yourself. An owner, follow-up or non-blocking label does not excuse a known plan miss.
- **INCONCLUSIVE**: the referenced plan cannot be read, the assigned slice is ambiguous, or outcome evidence is unavailable. State the missing source or proof and ask the supervisor to resolve it; use `status=supervisor_call` only if no known rejection condition applies. Never approve by assuming plan fit.
- **NOT APPLICABLE**: no parent epic, referenced plan or applicable plan target exists after checking dependencies and evidence. State that reason; do not invent a plan.


## Rule-reference procedure

For a task or brief citing an existing stakeholder rule, compare the causal code/test sites
with its `Rule-driven sites:` declaration and `cas rules refs <id> --json`.
Reject omitted rule-driven sites or missing adjacent `cas-rule:` markers; a
marker on an unrelated function does not cover them. Check the verbatim scope
against `stakeholder.scope` when present, without expanding it to unrelated work.


## Epic verification

When `task_type=epic`, use `verification_type=epic` and check, in order:

0. **Child-demo evidence first:** the evidence gate in Step 0 when any child has a demo statement.
1. **All subtasks closed:** every child from `task action=dep_list id=<epic-id>` is `closed`; otherwise reject.
2. **No open blockers.**
3. **Plan outcome:** apply Step 2 to the assembled epic's full planned scope and measurable acceptance targets, with receipts for actual outcomes. Closed children and passing checks do not prove those outcomes. A missed target or omitted planned behaviour is `PLAN-FIT: NOT VERIFIED` and rejects, even if every child's card passed. Follow-ups cannot excuse an unmet obligation of this epic.
4. **Close reason covers the whole epic**, not only the last child. Follow-ups that belong to future epics are fine only when they do not defer an obligation of this epic's plan.
5. **Verify on the epic branch**, not a worker worktree.
6. **Stranded-branch gate:** a clean review does not override the merge-state gate. Every child `factory/<assignee>` branch must have no commits outside the epic's integration target. A `stranded_branch_override` is valid only when the close response authorizes it for a live registered supervisor who records the inspection narrative.
7. **Epic verification owner gate:** if `epic_verification_owner` is set, only that live registered supervisor may close the epic. Treat an unassigned, stale, or mismatched owner as a blocking authority failure.

The close reason may come from the verification prompt, the epic's latest note, or its close-reason field.

## Verdict recording

One template; set `status`, `summary`, `confidence` and `issues` for the outcome:

```text
verification action=add task_id=<id> dispatch_id=<dispatch-id> status=<approved|rejected|error> confidence=<0.0-1.0> files="file1,file2" summary="<verdict>\n\nBlocking:\n- <file:line: what must be done>\n\nImprovements (non-blocking):\n- <file:line: suggestion>" issues='[{"file":"src/file","line":42,"severity":"blocking","category":"stub","code":"<snippet>","problem":"<what is missing>","suggestion":"<exact fix>"}]'
```

- Categories describe the defect, not whether it blocks: `correctness`, `todo_comment`, `temporal_shortcut`, `placeholder`, `stub`, `dead_code`, `incomplete_close_reason`, `code_duplication`, `error_handling`, `performance`, `security`, `naming`, `pattern_inconsistency`, `missing_edge_case`, `readability`, `unnecessary_complexity`, `missing_validation`, `resource_leak`. Issue severity stays `blocking` or `warning`; put the impact rating in the problem text.

## Delivery checkout

The task record names the delivery: `deliverables.files_changed`, `deliverables.commit_hash`, and the target branch (`Target: … @ <branch>`). In factory mode, inspect the delivery in a managed disposable checkout:

```bash
cas factory scratch-checkout --cas-root <cas root> --task-id <task-id> --dispatch-id <dispatch-id> --head <full bound head SHA>
```

The command prints the checkout path; use it as your working directory for review. Omit `--dispatch-id` only when the task has no dispatch. Publish durable logs and captures to the task artifacts directory. See Scratch checkout lifetime above for location and cleanup.
Use this managed checkout as the working directory for review.
