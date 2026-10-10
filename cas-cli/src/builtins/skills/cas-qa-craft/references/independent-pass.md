# Independent QA and polish pass

Use this when you started a `qa-pass` task. Cassy creates one when a
user-facing delivery parks for merge. You are the second pair of eyes: you
did not build this change, and Cassy refuses the implementer here. Judge the
running product the way a customer meets it, and judge what the delivery
changed: the pages, controls and journeys its diff touches, and its acceptance
criteria. A defect that was already on the base build is not this delivery's
to fix, unless fixing it is the delivery's stated purpose. Do not fix anything.

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

## 2. Cover the source-impact-selected journeys

```bash
scripts/journeys-for-diff.py <base> <bound_head>   # JSON: id, title, suite, reason
scripts/journey-eval.sh <ledger-dir> --affected <base> --workers=4
```

For hub-web source or dist changes, derive selection at the exact bound tip
from `journeys-for-diff`, then cover every selected ID. Include
`journey_receipt` in the round bundle per
[evidence-bundle.md](evidence-bundle.md). A hand-picked subset that omits a
selected ID refuses QA. Reuse the implementer's receipt when it covers the
same tip and selection; walk independent cells and inspect actual pixels.
Workers and reviewers use affected journeys; the supervisor owns the one
full-suite run per epic assembly and the merge queue. If a test fails, retain
that run and rerun only its spec at one worker to distinguish a flake.

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

When a cell fails, check the base build before you blame the delivery: serve
`git merge-base <base> <bound_head>` the same way and repeat the cell. If the
base build fails it the same way, the finding is **pre-existing**. Record it
(step 5) and do not count it toward the verdict, unless the delivery claims to
fix it; then it is an unmet acceptance criterion.

## 4. Polish

```bash
npm exec --yes --package=playwright -- node <skills-dir>/cas-ui-craft/scripts/visual-qa.mjs --strict --artifact-dir <ledger-dir>/visual-qa <url>...
```

`<skills-dir>` is the harness skill directory (`.claude/skills`, `.codex/skills`
or `.grok/skills`). Point `<url>` at your local serve of the reviewed tip from step 1, never the
production site. `qa_record` refuses a `visual_qa_status: "pass"` bundle
unless `visual-qa/visual-qa.json` records a strict PASS run against local URLs,
generated after the round opened.

When the strict run reports findings, they may be the page's older backlog.
Run the same script against a local serve of the base build, over the same
pages, into `<ledger-dir>/visual-qa-baseline/`. Then set
`visual_qa_status: "scoped"` and list the base report as
`files.visual_qa_baseline_json`. Cassy accepts the pair when every finding of
the reviewed tip pairs once with a base finding by type, rule/reason, page,
render state, scheme, full viewport and stable element identity. Identity uses
role + accessible name when supplied, or matching text + shared bounds within
0.5 CSS pixels. A CSS class rename alone does not add a finding; a new rule,
text or geometry still does. Historical reports lacking identity evidence
require matching selectors. A finding only the tip has is one the delivery introduced,
and `qa_record` refuses the pair. Either report shape works: a top-level
`findings` list, or per-render `renders[].issues` with the page in `input`.

This captures desktop 1280 and phone 390, each in light and dark. Then score
the `cas-ui-craft` critique rubric for what the delivery changed, not for the
whole page: distinctiveness, fit, hierarchy, craft, and accessibility. Give
one evidence sentence per score. Walk this checklist over the changed parts
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
  `visual-qa.mjs --strict` verdict line (plus the base run's, when scoped).
- **Pre-existing:** every defect the base build shares, with its evidence and
  the follow-up task Cassy filed for it. Say "none" when there are none.

Every finding cites a `trace action N` **and** its `F0N.png`. A finding
without both is not a finding. Recording the verdict cites the bundle on the
delivery as a `platform_proof` note (`qa-bundle: <abs>/bundle.json`).

## 6. Verdict

Reject only for what the delivery did or failed to do. The generated QA task
states the same bar, so decide by it, not by impression:

- a regression the delivery introduced: a Blocking or High finding it causes,
  or a strict visual-QA finding the base build does not have
- `visual-qa.mjs --strict` ran against anything but your local serves
- any rubric dimension below 3 for what the delivery changed, or
  distinctiveness, fit, or hierarchy below 4 on a public surface
- an acceptance criterion or the demo statement not met on the running build;
  a defect the delivery claims to fix that still reproduces counts here
- a required mode that was not proven: forced colors, reduced motion and more
  contrast count only when the capture shows `matchMedia(...)` matching, and
  keyboard-only must reach and complete the demo's primary action. An unrun
  mode is `NOT EXERCISED`, never PASS.

A pre-existing defect never rejects, even on a page the delivery touches.
Record it under **Pre-existing** in the ledger and pass it to `qa_record` with
`"scope": "pre-existing"`. Cassy files each one as a follow-up task linked to
the delivery and lists the new task ids in the verdict. A rejection whose
every issue is pre-existing is refused.

Otherwise approve. List Normal and Note findings in the summary so the
supervisor can turn them into follow-ups.

```text
verification action=qa_record task_id=<delivery> status=approved|rejected \
  summary="<one line: verdict and the top findings>" \
  issues='[{"severity":"high","problem":"...","suggestion":"...","file":"<screenshot>"},
          {"severity":"normal","scope":"pre-existing","problem":"...","file":"<screenshot>"}]' \
  ledger_path=<ledger-dir>/LEDGER.md
```

A rejection sends the delivery back to its implementer with your ledger. The
next park opens a new round. Recording the verdict also closes your QA task,
and it cannot be revised. If you change your mind after recording, do not
record again. Message the supervisor with `blocker=true`, ask for
`request_changes` on the delivery, and name the finding.
Stop your registered servers, then report the verdict line to the supervisor.
