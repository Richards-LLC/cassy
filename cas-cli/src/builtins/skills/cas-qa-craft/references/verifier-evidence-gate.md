# Verifier evidence gate

The task-verifier applies Code review procedure and Quality checklist to every
review. The demo and measurable-claim gates apply when the task or a child of its
epic has that requirement, and run before the close reason is read.

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
Before Step 0A, require the release `batch_receipt` ledger at
`<artifacts_root>/<project-key>/<epic-id>/LEDGER.md` for the reviewed tip; reject missing evidence with
`QA evidence required: epic has child demo_statement but no batch_receipt LEDGER.md`.
Require coverage of every child demo; reject a missing, running, other-tip or
incomplete receipt and list omitted demos as owed work. Apply Step 0A to this one
combined matrix, not per child; a cross-child contradiction in the captures is a
defect. Never rerun QA or substitute the children's ledgers for the batch round.

### Step 0A: Apply the QA evidence gate before any judgment

1. Locate the task's `LEDGER.md` (`~/.cas/artifacts/<project-key>/<task-id>/`, or the
   evidence directory the dispatch names). A close reason never substitutes for
   the ledger. In `qa.mode=batch`, a missing task ledger is owed to the release batch receipt; record NOT VERIFIED with that owner, never reject. Otherwise, if the file is absent, reject with the blocking
   finding `QA evidence required: task has a demo_statement but no LEDGER.md`,
   skip steps 2–6 and continue with the remaining phases; the one verdict also
   carries their findings.
2. Run `cas qa ledger check <path> --task <id> --json` (the parser close also
   uses). A nonzero exit rejects the ledger before capture judgment: list each
   returned line, row ID, field and message in the verification summary as a
   blocking finding, skip steps 3–6 and continue with the remaining phases.
   Use the candidate revision's command when reviewing a change to the parser
   itself.
3. Keep the JSON report. Schema 2 requires cited `path:line` expectations and
   `visible:`, `terminal:` or `http:` outcomes; DB-only PASSes fail. Legacy receipts remain valid.

4. Open every PASS capture and its cited source line. Judge the visible result
   against that source: database rows, charges or a stored draft cannot prove what
   the user sees. An unsupported expectation is a spec question. Record misleading
   PASSes as FAIL with capture path and reason; never edit the worker's ledger.
   Treat any evidence label weaker than the cell requires as `NOT EXERCISED: <reason>`,
   never as `PASS`.
5. `NOT EXERCISED` rows are owed work, not failures. List every row's final
   verdict, label and capture judgment with the shared-check result.
6. Only then read the close reason. A capture-downgraded row or a ledger `FAIL`
   is incomplete evidence whatever the close reason says.

### NOT EXERCISED rows: apply the configured QA owner

Apply Step 0's `qa.mode` policy: in `qa.mode=batch`, record each journey row
deferred solely to release batch QA by row ID, cell and owner; that approval
does not claim the journey passed or satisfy the batch receipt. Every other
`NOT EXERCISED` row (per_task mode, missing captures, unavailable environments,
unclear ownership, rows you downgraded) needs the supervisor: when the gate
otherwise passes, record `status=supervisor_call` with a summary starting
`SUPERVISOR CALL: <n> NOT EXERCISED row(s)` listing each row ID, cell and reason.
The supervisor answers on the same `dispatch_id`; no re-dispatch is needed.

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
   `NOT VERIFIED` when the close names an owner by design. Reject the rest,
   naming the claim and the missing or ambiguous run.

Ownership rule: workers run only the tests covering the lines they touched, with
capped receipts at the delivered tip. The full suite runs once on the landing
tree: in the merge queue, else CI, else one supervisor run per epic. Read
receipts; never rerun a passing check.

## Candidate fixture isolation

Before running a candidate `serve` or any stateful command, isolate it in a
disposable fixture as [evidence-ledger.md](evidence-ledger.md#candidate-fixture-isolation)
describes (fixture `.cas`, `CAS_ROOT`, `HOME`, `XDG_CONFIG_HOME`, recorded store
path). Scope these overrides to candidate subprocesses so your reviewer MCP
session keeps its factory binding.

## Counted verifier lane

To run the task's tests yourself, use the counted verifier lane at the dispatch's bound head, never plain Cargo: `cas factory verifier-test --cas-root <cas root> --task-id <task-id> --dispatch-id <dispatch-id> --head <full bound head SHA> -- nextest run -p <crate> [--lib|--test <harness>] -E 'test(module::name)'`. Omit `--dispatch-id` only if the task has no dispatch. A "Waiting for build admission" refusal means retry later; if it persists, mark the item NOT VERIFIED with that reason instead of building another way.

## Rule candidates

For a lesson backed by at least two independent incidents from different tasks, run `rule action=check_similar content="<lesson>"`, then `rule action=observe content="<lesson>" tags="category:<cat>" source_ids="<task-id>:<verification-id>,<other-task-id>:<other-verification-id>"`, adding `id=<candidate-id>` for the same lesson. Cite completed verdicts belonging to those tasks; Cassy validates the links and records the bundle atomically before raising it to the supervisor and operator. Never create or promote the rule yourself.

## Code review procedure

### Step 4: Read every changed file in full

Run `rule action=list` for the project rules. Read each changed file completely. In the changed code, reject:

- TODO/FIXME/XXX/HACK markers, and `todo!()`, `unimplemented!()`, `raise NotImplementedError`, `throw new Error('Not implemented')`;
- temporal shortcuts ("for now", "temporarily", "placeholder") that leave an acceptance-criteria item undone;
- new `@ts-ignore`, `#[allow(dead_code)]`, `# type: ignore` without a stated reason;
- code that duplicates existing functionality (search before approving).

### Step 5: Structural checks with receipts

Use `ast-grep` on changed files, for example `ast-grep --lang rust -p '$EXPR.unwrap()' <file>`, `ast-grep --lang typescript -p '$EXPR as any' <file>`, `ast-grep --lang typescript -p 'catch ($ERR) {}' <file>`. If `ast-grep` is unavailable or cannot parse the language, fall back to:

```bash
rg -n 'unwrap\(\)|todo!|unimplemented!|console\.log|@ts-(ignore|expect-error)|except:' <changed_file>
```

Back every finding with a command output or an exact line reference. Run tests only through the Counted verifier lane.

### Step 6: Impact, wiring and co-changes (blocking when missing)

- A changed signature, field, export or public API: search its callers (`rg '<name>'`) and confirm they were updated.
- Every new function, route, handler, command, tool, migration or config field is reachable: it has a call site or registration outside its definition. Test helpers, derive-required impls and exported library items are exempt.
- Files that change together did: tests for changed logic, a migration for a schema change, route registration for a new endpoint, defaults and docs for new config.
- Proof matches blast radius (ladder in the `verify-before-claim` skill: read → targeted test → integration or real run → user-path walk). A `risk=blast-radius` task needs each `proof_targets` entry at rung 3 or owned by the landing-tree full-suite run, plus one safety fact: what could break and why it did not. A missing safety fact is `incomplete_close_reason`, as is a "full suite" claim that ran fewer files or tests than the suite runs.

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
- tests: review the caller-visible promise and independent expectations against [Test hygiene](../../cas-writing-tests/SKILL.md#test-hygiene); browser specs follow cas-playwright-tests and its accessibility-property exception. Preserve module/mock isolation imports and immutable shared setup. Flag unasserted screenshots, source/style pins, missing required coverage and deletion of imported helpers.

Rate each finding `critical`, `high`, `medium`, or `low`; name its file and line, observed effect, evidence, and exact fix. Apply the verdict threshold below. Skip style nits and sweeping refactors.

## Plan conformance

Run `task action=dep_list id=<task-id>` to discover ParentChild dependencies. Read each parent epic with `task action=show id=<epic-id> full=true`, including its acceptance criteria, design and decision notes. Read the relevant sections of any plan document named by the task or epic, fetching cited artifacts and reading their contents. Follow intermediate parents to the epic when necessary. A task card and its close reason are not substitutes for the plan.

Identify the fields, behaviours and measurable targets that the plan assigns to this delivery or phase. Map each obligation to implementation and proof in the delivery found in Step 3; finalize this check after reading the changed files. Name each missing field or behaviour individually, with its plan section and delivery evidence. Compare every applicable plan target with the actual result, including targets recorded in delivered evidence even when the card only asks to run or report a measurement. Apply Step 0's measurable-claims evidence gate to these targets too. Passing tests or completing the card does not prove the plan's outcome.

Respect the planned division of work: a child implementing its assigned slice need not implement the entire epic; cite the plan section or scope decision assigning other obligations elsewhere. A worker's narrower card, follow-up, renamed field or lowered threshold is not a plan revision; an authorized operator revision is, so cite it and check against the revised obligations.

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

## Ruling fit

Apply this to every delivery, even when its task and brief cite no rule.

1. Reuse Step 4's `rule action=list` active project sources, including Cassy's
   authenticated draft operator hard rules. Read `rule action=show id=<id>` for
   the stored rules and read the listed repository policy files: previews omit
   provenance and scope. An ordinary draft or text claiming operator authority
   is insufficient. Exclude held, retired, stale and explicitly superseded
   rulings. A passed `review_after` is a review deadline, not automatic
   retirement: disclose the overdue review rather than silently discarding a
   still-active ruling.
2. Select rulings whose paths or area touch the delivery, using verbatim
   `stakeholder.scope` to limit applicability. Read their `source_ids`, applicable
   task/parent decision notes and `rule action=history id=<id>` when needed to
   establish the quote's source, date or authorized replacement. Distinguish the
   ruling date from the rule's creation date; disclose an unavailable date.
   Do not broaden an account-deletion ruling into a paid-send constraint.
3. Compare the actual changed code, product strings, fixture values and test
   assertions with the stakeholder's verbatim quote, not just the interpreted
   rule content. Follow exercised helpers into unchanged services as needed.
   An assertion requiring a credit-confirmation prompt contradicts a ruling to
   send without asking, even when that assertion and the implementation agree.
   Judge the behavior required by the assertion, not matching vocabulary or a
   writing preference. Passing tests do not establish ruling compliance.
4. Report `RULING-FIT: VERIFIED|NOT VERIFIED|INCONCLUSIVE|NOT APPLICABLE`.
   VERIFIED names the applicable rulings and delivery evidence; NOT APPLICABLE
   names the completed discovery and why none applies. A known contradiction is
   NOT VERIFIED: reject with a blocking `correctness` issue naming the rule ID,
   stakeholder, exact quote, source and date, and each offending file:line.
   This is a completeness failure even without another runtime reproduction;
   a follow-up or theoretical-impact label cannot waive it.

Unreadable/truncated rule evidence, ambiguous scope or conflicting rulings without
an authorized replacement is INCONCLUSIVE, never presumed compliant. If applicable
operator/stakeholder decision notes have not been recorded as rules, cite those
notes and ask the supervisor to record the quote and scope through the existing
`rule action=create`/`update` stakeholder-provenance path (including `review_after`
for attributed hard rules). Do not create a second register or mutate rules as a
verifier. A known contradiction still rejects; otherwise unresolved authority or
evidence requires `status=supervisor_call`, with the missing source or decision.

## Fake seams

For every changed test or fixture that supplies a model/component output, trace
the mocked reader, LLM, classifier or other producer to its real implementation
and output contract. Read the enum, discriminated union, schema or documented
output and any runtime validation/normalization before the mocked boundary.
Check each supplied value's variant, required fields and invariants at that exact
boundary. A type cast, permissive mock or consumer accepting a dictionary does
not prove the producer can emit it. A raw model reply and a validated reader
result are different boundaries; use the one actually replaced by the fake.

Report `FAKE-SEAM: VERIFIED|NOT VERIFIED|INCONCLUSIVE|NOT APPLICABLE`, mapping
each fake's test file:line to the real contract file:line and allowed output.
An impossible variant/value is NOT VERIFIED: reject with a blocking
`missing_validation` issue naming the fake and real contract, even if the test
passes or the consumer contains a branch for that value. This invalidates delivery
proof without needing a separate runtime reproduction. Ask for a contract-valid
fixture that exercises reachable behavior, not a weakened contract or a cast.

VERIFIED means the inspected fakes satisfy the real contracts; NOT APPLICABLE
means no output seam is faked after inspection. Broad types, unavailable producer
code or undocumented constraints do not prove a fake impossible or valid: record
INCONCLUSIVE with the missing contract, and request `status=supervisor_call` if
no known rejection condition remains. Approve only when RULING-FIT and FAKE-SEAM
are each VERIFIED or NOT APPLICABLE; include both lines in the verdict summary.


## Epic verification

When `task_type=epic`, use `verification_type=epic` and check, in order:

0. **Child-demo evidence first:** Step 0 when any child has a demo statement.
1. **All subtasks closed:** every child from `task action=dep_list id=<epic-id>` is `closed`, with no open blockers.
2. **Plan outcome:** apply Plan conformance to the assembled epic's full planned scope and measurable targets, with receipts. A missed target or omitted behaviour is `PLAN-FIT: NOT VERIFIED` even if every child's card passed; follow-ups cannot excuse it.
3. **Close reason covers the whole epic**, not only the last child; future-epic follow-ups may not defer this epic's obligations.
4. **Verify on the epic branch**, not a worker worktree.
5. **Stranded branches and owner:** reject child `factory/<assignee>` commits outside the epic's integration target unless the close authorizes `stranded_branch_override` with an inspection narrative. When `epic_verification_owner` is set, reject absent, stale or mismatched caller authority; only that live registered supervisor can close. Close enforces both too; a clean review overrides neither.

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

Use the printed checkout path as your working directory for review; Cassy reclaims it after the verdict. Omit `--dispatch-id` only when the task has no dispatch. Publish durable logs and captures to the task artifacts directory.
