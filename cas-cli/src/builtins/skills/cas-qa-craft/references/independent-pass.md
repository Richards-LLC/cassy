# Independent QA and polish pass

Use this when you started a `qa-pass` task. Cassy creates one on an explicit
QA request or when a user-facing `qa.mode=per_task` delivery parks for merge,
and refuses the implementer as its reviewer. Judge the running product the way
a customer meets it, and judge what the delivery changed: the pages, controls
and journeys its diff touches, and its acceptance criteria. Do not fix anything.

The task description names the delivery, its branch and exact tip, the base
branch, the reasons the pass applies, the ledger directory, the deadline and the
rejection bar. Mark cells unrun at the deadline `NOT EXERCISED: <reason>`.

## Batch default

In `qa.mode=batch` capture before/after evidence during implementation. The
supervisor builds one candidate per release at the exact frozen tip and walks
one combined matrix. Initialize the batch ledger with `cas qa ledger init --task <qa-task>
--covers <delivery-a>,<delivery-b>` using every intended receipt task ID, and
run `cas qa ledger check <path> --task <qa-task> --covers <delivery-a>,<delivery-b>`
before publishing; the checker rejects missing or incomplete coverage. Only the
`covers:` header declares coverage; a mention elsewhere in the report does not.
Publish the PASS ledger, then record `verification action=batch_receipt` with
`task_ids`, `staging_sha`, `ledger_artifact_id`, `ledger_path` and `ledger_author`
(a non-implementer). Every listed task's delivery anchor must be contained in
that reviewed target commit; deferred closes complete once the receipt covers
their anchors. Explicit `qa_request` reviews use the procedure below.

## 1. Build the exact tip

For a correction or rebase, first use the supervisor's
[correction proof](../../cas-supervisor/references/correction-proof.md) to
identify affected checks and any supported verdict carry. Preserve original
execution identities; a carried verdict must be recorded for this tip.

Check out the reviewed tip in your own worktree with
`git switch --detach <bound_head>`. Use an existing build with verified identity
or build once with the project's real build, then serve it through `cas-servers`.
<!-- cas-src:begin -->
For hub-web without a matching build, run `npm run build`, then serve
`hub-web/dist`. A build or serve failure is a **Blocking** finding. Record the
<!-- cas-src:else -->
A build or serve failure is a **Blocking** finding. Record the
<!-- cas-src:end -->
command, the URL, and `npx playwright --version` in the ledger header. Where
the build guard refuses you a compile, run the immutable candidate for
`<bound_head>` named as [evidence-ledger](evidence-ledger.md#binary-sources)
requires. A missing binary is not the delivery's defect: ask the supervisor for
one and mark its cells `NOT EXERCISED: <reason>`. On a correction, run affected
independent cells and cite unaffected prior rows with their actual executed
revision; current-round bundle and verdict validation still apply.

## 2. Review the worker's affected-journey evidence

Derive the selection at the exact reviewed tip:

```bash
scripts/journeys-for-diff.py <base> <bound_head>   # JSON: id, title, suite, reason
```

Consume the implementer's passing receipt when its full base/head SHAs match
this selection, every selected ID has a positive pass count and no failures or
skips, and the native suite exit and runner errors are zero/empty. Check the
native report and recorded Playwright version against the reviewed build's
lockfile and execution environment. Cite the original receipt/log paths in the
round bundle and ledger; preserve their producer and timestamps. Reuse matching
unit/typecheck receipts as well. Read the tests the delivery adds against
[Fewest tests](../../cas-writing-tests/SKILL.md#fewest-tests): bloat, restated
implementation, sleeps and retry-wrapped flakes are Normal findings, and a test
that steps around its asserted invariant is at least Normal.

Where the project has a journey registry, the QA task's journey table is the
park-time impact for the reviewed head: every live journey in it needs its own
ledger row ([journey rows](journeys.md#journey-rows)), and each escape the task
lists is a claim to check against the diff. A reason that does not hold is a
Normal finding.

If that evidence is absent, failed, incomplete, or belongs to different inputs
or an incompatible environment, run the missing checks. For affected journeys:

<!-- cas-src:begin -->
```bash
scripts/journey-eval.sh <ledger-dir> --affected <base> --workers=4
```
<!-- cas-src:else -->
Use the project's affected-journey selection (its CLAUDE.md or AGENTS.md
names the command).
<!-- cas-src:end -->

A hand-picked subset cannot replace the source-impact selection. Keep an
original failure and rerun only its spec at one worker for a failure control.
<!-- cas-src:begin -->
The merge queue owns the full run on the final integration tree. For parallel
runs, set ports as [journeys.md](journeys.md#run-them-and-keep-the-evidence) describes.
<!-- cas-src:else -->
The final integration gate (CI on the landing SHA, or the supervisor's
integrated run) owns the full run.
<!-- cas-src:end -->

Add any journey whose Goal the demo statement names, walk each from its real
entry point to the user's goal, and score it per
[journeys.md](journeys.md#score-the-experience). When a journey has no suite,
drive it by hand with `npx playwright cli` and keep a trace.

## 3. Correctness paths

Walk the demo statement end to end, then the obvious adjacent paths: empty,
loading and error states; long content; phone width (390px); dark mode;
keyboard only; reduced motion. Cap this at **8 cells**. The
[walk rules](../SKILL.md#walk-rules) apply to every cell. Write each expected
result in the user's words before you run the cell, and capture a trace and a
screenshot per cell.

When a cell fails, check the base build before you blame the delivery: serve
`git merge-base <base> <bound_head>` the same way and repeat the cell. A
failure the base build shares is **pre-existing** only when it falls outside the
delivery's stated purpose, acceptance criteria and demo statement, and outside
any journey invariant the delivery diff adds or edits; inside any of those
scopes it is unmet. Record qualifying pre-existing findings in step 5; they do
not count toward the verdict. A finding outside the frozen release scope goes in
the ledger's next-board list and does not fail the round, unless it loses data
or hides state.

## 4. Polish

```bash
npm exec --yes --package=playwright -- node <skills-dir>/cas-ui-craft/scripts/visual-qa.mjs --strict --artifact-dir <ledger-dir>/visual-qa <url>...
```

Point `<url>` at your local serve of the reviewed tip from step 1, never the
production site. `qa_record` refuses a `visual_qa_status: "pass"` bundle unless
`visual-qa/visual-qa.json` records a strict PASS run against local URLs,
generated after the round opened. Findings the base build shares follow the
`visual_qa_status: "scoped"` rules in [evidence-bundle.md](evidence-bundle.md#layout):
run the same script against a local base serve into `<ledger-dir>/visual-qa-baseline/`.

This captures desktop 1280 and phone 390, each in light and dark. Score the
`cas-ui-craft` critique rubric for what the delivery changed, not the whole
page, with one evidence sentence per score, and mark each item pass or fail over
the changed parts: token consistency with the DESIGN.md the close's `design:`
line names, spacing and alignment, typography, copy and microcopy, empty/
loading/error/disabled states, focus rings, contrast, truncation and overflow,
motion.

## 5. Ledger and evidence bundle

The round directory `<ledger-dir>` is a cas-qa-craft
[evidence bundle](evidence-bundle.md) with `"producer": "independent-qa"`,
`task_id` set to the delivery and `head_sha` to the full reviewed tip;
`qa_record` refuses another producer, task or tip. Differences from the
implementer's bundle:

- the trace is recorded with
  `{ mode: 'on', snapshots: { dom: true, aria: true, screen: true }, screenshots: false, sources: true }`;
- for `visual_change: true`, `receipt.webm` adds one
  `showRecordingChapter(page, 'F01 …', { duration: 1000 })` per finding, using the
  helper from the [worked example](evidence-bundle.md#worked-example);
- captures are `F01.png`, `F02.png`, …, one per finding, listed in `files.cells`;
- each journey gets its own `journeys/<ID>/` folder.

Write `<ledger-dir>/LEDGER.md` beside `bundle.json` with these sections:

- **Header:** pass id, reviewer, implementer, branch, tip, build command,
  URL, and Playwright version.
- **Journeys:**
  `| ID | Run | Dead end | Copy | Steps | Context | Waits | Severity | Findings / tasks |`
- **Correctness:**
  `| # | Path | Viewport · scheme | Expected | Actual | Severity | Evidence |`
- **Polish:** the rubric table, the checklist, and the
  `visual-qa.mjs --strict` verdict line (plus the base run's, when scoped).
- **Pre-existing:** every defect the base build shares, with its evidence, and
  the follow-up Cassy filed for any that carries `repro` (steps or command, plus
  the wrong output) and `affected` (who acts on it today), or "none".

Every finding cites a `trace action N` **and** its `F0N.png`; a finding
without both is not a finding.

## 6. Verdict

Decide by the rejection bar in the QA task description. Pass each pre-existing
finding to `qa_record` with `"scope": "pre-existing"`; Cassy files a follow-up
linked to the delivery only when it carries `repro` and `affected`, and refuses
a rejection whose every issue is pre-existing. Otherwise approve, listing Normal
and Note findings in the summary for the release's polish sweep.

```text
verification action=qa_record task_id=<delivery> status=approved|rejected \
  summary="<one line: verdict and the top findings>" \
  issues='[{"severity":"high","problem":"...","suggestion":"...","file":"<screenshot>"},
          {"severity":"normal","scope":"pre-existing","problem":"...","file":"<screenshot>","repro":"<steps and wrong output>","affected":"<who acts on it today>"}]' \
  ledger_path=<ledger-dir>/LEDGER.md
```

A rejection sends the delivery back to its implementer with your ledger; the
next park opens a new round. In a release batch round there is no next round:
list every failing row in one ledger; the supervisor batches the fix and asks for
a re-walk of those rows only. Recording the verdict closes your QA task and cannot
be revised. To change your mind afterwards, message the supervisor with
`blocker=true`, ask for `request_changes` on the delivery, and name the finding.
Stop your registered servers, then report the verdict line to the supervisor.
