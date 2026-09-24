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

Ports: hub-web's Playwright config never reuses a running server. Each
checkout gets its own default port pair in 20000–32767, derived from its
path. A port that is already taken fails the run instead of testing another
checkout's build. For runs in parallel from one checkout, set
`HUB_E2E_PORT`/`HUB_JOURNEY_PORT` to a distinct pair in 20000–32767 (below
Linux's ephemeral range), and name the ports in the ledger header.

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

Point `<url>` at your local serve of the reviewed tip from step 1, never the
production site. `qa_record` refuses a `visual_qa_status: "pass"` bundle
unless `visual-qa/visual-qa.json` records a strict PASS run against local URLs,
generated after the round opened.

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

## 5. Ledger and evidence bundle

The round directory `<ledger-dir>` is a cas-qa-craft evidence bundle, made
to the same contract as the implementer's bundle. Its `bundle.json` must
have:

- `"producer": "independent-qa"`
- `task_id`: the delivery
- `head_sha`: the reviewed tip, in full

`qa_record` refuses a verdict whose bundle is missing or names another
producer, task, or tip. It also refuses a claimed visual-QA pass that has no
matching local run (see step 4). The files, all listed in `bundle.json`:

- `trace.zip`, recorded with
  `{ mode: 'on', snapshots: { dom: true, aria: true, screen: true }, screenshots: false, sources: true }`
- `trace-actions.txt`: the output of `npx playwright trace actions`
- `receipt.webm`: start it with
  `page.screencast.start({ path, size: page.viewportSize() })` and
  `showActions`, and add one `showChapter('F01 …', { duration: 1000 })` per
  finding
- `final.aria.yml` and `final.aria.json`
- `F01.png`, `F02.png`, …: one capture per finding, listed in `files.cells`
- `visual-qa/` and `visual-qa.stdout`
- `critique.md`, whose scores match `critique_score`
- the three a11y captures, when the change is visual
- one `journeys/<ID>/` folder per journey

Write `<ledger-dir>/LEDGER.md` beside `bundle.json`. It has these sections:

- **Header:** pass id, reviewer, implementer, branch, tip, build command,
  URL, and Playwright version.
- **Journeys:**
  `| ID | Run | Dead end | Copy | Steps | Context | Waits | Severity | Findings / tasks |`
- **Correctness:**
  `| # | Path | Viewport · scheme | Expected | Actual | Severity | Evidence |`
- **Polish:** the rubric table, the checklist, and the
  `visual-qa.mjs --strict` verdict line.

Every finding cites a `trace action N` **and** its `F0N.png`. A finding
without both is not a finding. Recording the verdict cites the bundle on the
delivery as a `platform_proof` note (`qa-bundle: <abs>/bundle.json`).

## 6. Verdict

Reject when any of these hold. The generated QA task states the same bar, so
decide by it, not by impression:

- a Blocking or High finding
- `visual-qa.mjs --strict` fails, or ran against anything but your local
  serve of the reviewed tip
- any rubric dimension below 3
- distinctiveness, fit, or hierarchy below 4 on a public surface
- an easy-to-spot bug on the touched path, including a pre-existing one on
  the path the delivery claims to fix
- a required mode that was not proven: forced colors, reduced motion and more
  contrast count only when the capture shows `matchMedia(...)` matching, and
  keyboard-only must reach and complete the demo's primary action. An unrun
  mode is `NOT EXERCISED`, never PASS.

Otherwise approve. List Normal and Note findings in the summary so the
supervisor can turn them into follow-ups.

```text
verification action=qa_record task_id=<delivery> status=approved|rejected \
  summary="<one line: verdict and the top findings>" \
  issues='[{"severity":"blocking","problem":"...","suggestion":"...","file":"<screenshot>"}]' \
  ledger_path=<ledger-dir>/LEDGER.md
```

A rejection sends the delivery back to its implementer with your ledger. The
next park opens a new round. Recording the verdict also closes your QA task,
and it cannot be revised. If you change your mind after recording, do not
record again. Message the supervisor with `blocker=true`, ask for
`request_changes` on the delivery, and name the finding.
Stop your registered servers, then report the verdict line to the supervisor.
