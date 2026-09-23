# Independent QA and polish pass

Use this when you started a `qa-pass` task. Cassy creates one when a
user-facing delivery parks for merge. You are the second pair of eyes: you
did not build this change, and Cassy refuses the implementer here. Judge the
running product the way a customer meets it. Do not judge the diff, and do
not fix anything.

The task description names the delivery, its branch and exact tip, the base
branch, the reasons the pass applies, the ledger directory, and the deadline.
The time box is `qa.pass_timeout_mins`, 45 minutes by default. An honest
incomplete ledger beats a late one: mark unrun cells `NOT EXERCISED`.

## 1. Build the exact tip

Check out the reviewed tip in your own worktree with
`git switch --detach <bound_head>`. Build with the project's real build and
serve it through `cas-servers`. For hub-web, run `npm run build`, then serve
`hub-web/dist`. A build or serve failure is a **Blocking** finding. Record the
command, the URL, and `npx playwright --version` in the ledger header.

## 2. Walk the journeys the change touches

```bash
scripts/journeys-for-diff.py <base> <bound_head>   # JSON: id, title, suite, reason
scripts/journey-eval.sh <ledger-dir> --grep <ID>   # receipts in <ledger-dir>/journeys/<ID>/
```

Add any journey whose Goal the demo statement names. Walk each journey from
its real entry point to the user's goal, across feature boundaries. When a
journey has no suite, drive it by hand with `npx playwright cli` and keep a
trace. Score each journey 0–3 on dead end, copy, steps, context, and waits.
Severity follows the catalog rules:

| Severity | When |
| --- | --- |
| Blocking | any dimension scores 3, or the journey cannot reach its goal |
| High | any 2 on a core journey |
| Normal | any other 2, or a 1 with a concrete fix |
| Note | everything else |

## 3. Correctness paths

Walk the demo statement end to end, then the obvious adjacent paths:

- empty, loading, and error states
- long content
- phone width (390px)
- dark mode
- keyboard only
- reduced motion

Cap this at **8 cells**. Write each expected result in the user's words
before you run the cell. Capture a trace and a screenshot per cell.

## 4. Polish

```bash
node scripts/visual-qa.mjs --strict --artifact-dir <ledger-dir>/visual-qa <url>...
```

This captures desktop 1280 and phone 390, each in light and dark. Then score
the `cas-ui-craft` critique rubric: distinctiveness, fit, hierarchy, craft,
and accessibility. Give one evidence sentence per score. Walk this checklist
and mark each item pass or fail:

- DESIGN.md and token consistency
- spacing and alignment
- typography
- copy and microcopy
- empty, loading, error, and disabled states
- focus rings
- contrast
- truncation and overflow
- motion

## 5. Ledger

Write `<ledger-dir>/LEDGER.md` beside the evidence. Use the same file layout
as the implementer's cas-qa-craft bundle: `trace.zip`, `trace-actions.txt`,
`final.aria.yml`, per-cell PNGs, and `visual-qa/`. Add a `journeys/<ID>/`
folder per journey. The ledger has these sections:

- **Header:** pass id, reviewer, implementer, branch, tip, build command, URL,
  and Playwright version.
- **Journeys:**
  `| ID | Run | Dead end | Copy | Steps | Context | Waits | Severity | Findings / tasks |`
- **Correctness:**
  `| # | Path | Viewport · scheme | Expected | Actual | Severity | Evidence |`
- **Polish:** the rubric table with an evidence sentence per score, the
  checklist, and the `visual-qa.mjs --strict` verdict line.

Every finding cites a `trace action N` from `npx playwright trace actions`
**and** a screenshot path. A finding without both is not a finding.

## 6. Verdict

Reject when any of these hold:

- a Blocking or High finding
- `visual-qa.mjs --strict` fails
- any rubric score of 0
- distinctiveness, fit, or hierarchy below 4 on a public surface
- craft or accessibility below 3

Otherwise approve. List Normal and Note findings in the summary so the
supervisor can turn them into follow-ups.

```text
verification action=qa_record task_id=<delivery> status=approved|rejected \
  summary="<one line: verdict and the top findings>" \
  issues='[{"severity":"blocking","problem":"...","suggestion":"...","file":"<screenshot>"}]' \
  ledger_path=<ledger-dir>/LEDGER.md
```

A rejection sends the delivery back to its implementer with your ledger. The
next park opens a new round. Recording the verdict also closes your QA task.
Stop your registered servers, then report the verdict line to the supervisor.
