# Independent QA and polish pass

Use this when you started a `qa-pass` task. Cassy creates one on an explicit
QA request or when a user-facing `qa.mode=per_task` delivery parks for merge. You are the second pair of eyes: you
did not build this change, and Cassy refuses the implementer here. Judge the
running product the way a customer meets it, and judge what the delivery
changed: the pages, controls and journeys its diff touches, and its acceptance
criteria. A defect shared with base is in scope when the delivery's stated
purpose, acceptance criteria or demo statement cover it, or its diff adds or edits a journey
invariant that covers it. Do not fix anything.

The task description names the delivery, its branch and exact tip, the base
branch, the reasons the pass applies, the ledger directory, and the deadline.
The time box is `qa.pass_timeout_mins`, 45 minutes by default. An honest
incomplete ledger beats a late one: mark unrun cells `NOT EXERCISED: <reason>`.

## Batch default

`qa.mode=batch` is the built-in default; `per_task` explicitly selects per-delivery
reviews. Project QA keys override user `~/.cas/config.toml`, then built-in defaults.
For batch work, capture before/after evidence during implementation. The supervisor
builds an hourly candidate at the exact epic tip and walks one combined matrix.
Publish the PASS ledger, then record `verification action=batch_receipt` with
`task_ids`, `staging_sha`, `ledger_artifact_id`, `ledger_path` and `ledger_author`
(a non-implementer). Every listed
task's delivery anchor must be contained in that reviewed target commit. Close
after the receipt covers those anchors. A deferred close reports "waiting for
batch receipt" until then. Explicit `qa_request` reviews use the procedure below.

## 1. Build the exact tip

For a correction or rebase, first use the supervisor's
[correction proof](../../cas-supervisor/references/correction-proof.md) to
identify affected checks and any supported verdict carry. Preserve original
execution identities; a carried verdict must be recorded for this tip.

Check out the reviewed tip in your own worktree with
`git switch --detach <bound_head>`. Use an existing build with verified identity
or build once with the project's real build, then serve it through `cas-servers`.
For hub-web without a matching build, run `npm run build`, then serve
`hub-web/dist`. A build or serve failure is a **Blocking** finding. Record the
command, the URL, and `npx playwright --version` in the ledger header. Where
the build guard refuses you a compile (a factory agent cannot build Rust), run
the immutable candidate the supervisor provides for `<bound_head>` instead, named as
[evidence-ledger](evidence-ledger.md) requires. A missing binary is not the
delivery's defect: ask the supervisor for one and mark its cells `NOT EXERCISED: <reason>`.
Reuse an existing candidate with verified identity before requesting another
build. On a correction, run affected independent cells and cite unaffected
prior rows with their actual executed revision; current-round bundle and
verdict validation still apply.

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
unit/typecheck receipts as well. Run independent adjacent/demo/visual cells in
steps 3–4; the automated selection is the worker's responsibility. Read the
tests the delivery adds against [Fewest tests](../../cas-tdd/SKILL.md#fewest-tests):
bloat, restated implementation, sleeps and retry-wrapped flakes are Normal findings.
Rate a delivery test that steps around its asserted invariant at least Normal.
For example, re-picking the sender after a 409 masks whether the chosen sender
survived the conflict; judge the masked failure under §3.

Where the project has a journey registry, the QA task's journey table is the
park-time impact for the reviewed head: every live journey in it needs its own
ledger row ([journey rows](journeys.md#journey-rows)), and each upkeep escape
the task lists is a claim to check against the diff. A reason that does not
hold is a Normal finding.

If that evidence is absent, failed, incomplete, or belongs to different inputs
or an incompatible environment, run the missing checks. For affected journeys:

```bash
scripts/journey-eval.sh <ledger-dir> --affected <base> --workers=4
```

A hand-picked subset cannot replace the source-impact selection. Keep an
original failure and rerun only its spec at one worker for a failure control.
The merge queue owns the full run on the final integration tree.

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

Cap this at **8 cells**. The [walk rules](../SKILL.md#walk-rules) apply to
every cell: a change to a shared step starts with its entry paths. Write each
expected result in the user's words before you run the cell. Capture a trace
and a screenshot per cell.

When a cell fails, check the base build before you blame the delivery: serve
`git merge-base <base> <bound_head>` the same way and repeat the cell. If the
base build fails it the same way, classify the finding as **pre-existing** only
when it falls outside the delivery's stated purpose, acceptance criteria and
demo statement, and outside any journey invariant the delivery diff adds or
edits. A failure inside any of those scopes is unmet even when base shares it.
For example, cas-1073's `am-deal-to-quote-3` required preserving the chosen
quote From address after a 409 split-conflict response; resetting it to the
recommended sender was unmet even though base did the same. Record only
qualifying pre-existing findings in step 5; do not count them toward the verdict.

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

- token consistency with the DESIGN.md the close's `design:` line names
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
- `receipt.webm`, only for `visual_change: true`: start it with
  `page.screencast.start({ path, size: page.viewportSize() })` and
  `showActions`, and add one
  `showRecordingChapter(page, 'F01 …', { duration: 1000 })` per finding. For
  other projects, copy the self-contained constructed-stylesheet helper from
  the [worked example](evidence-bundle.md#worked-example) into the evidence spec.
  Commander-only, in the Cassy source checkout: import that helper from
  `hub-web/e2e/journeys/receipt-directory.mjs`, adjusting the spec's relative
  path. That repository file is not installed in other projects. Both routes
  preserve `style-src 'self'`. Omit screencast and chapter calls for non-visual
  changes
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
- **Pre-existing:** every defect the base build shares, with its evidence, and
  the follow-up Cassy filed for any that carries `repro` (steps or command, plus
  the wrong output) and `affected` (who acts on it today). Say "none" when there
  are none.

Every finding cites a `trace action N` **and** its `F0N.png`. A finding
without both is not a finding. Recording the verdict cites the bundle on the
delivery as a `platform_proof` note (`qa-bundle: <abs>/bundle.json`).

## 6. Verdict

Reject only for what the delivery did or failed to do. The generated QA task
states the same bar, so decide by it, not by impression:

- a regression caused by the delivery: a Blocking or High finding it causes,
  or a strict visual-QA finding the base build does not have
- `visual-qa.mjs --strict` ran against anything but your local serves
- any rubric dimension below 3 for what the delivery changed, or
  distinctiveness, fit, or hierarchy below 4 on a public surface
- the delivery's stated purpose, an acceptance criterion, the demo statement,
  or a journey invariant its diff adds or edits is not met on the running build,
  even if base shares the same failure (see §3)
- a required mode that was not proven: forced colors, reduced motion and more
  contrast count only when the capture shows `matchMedia(...)` matching, and
  keyboard-only must reach and complete the demo's primary action. An unrun
  mode is `NOT EXERCISED: <reason>`, never PASS.

Record each finding classified as pre-existing under §3 in the ledger and pass
it to `qa_record` with `"scope": "pre-existing"`. Cassy files one as a follow-up
linked to the delivery only when it also carries `repro` and `affected`, and
lists the new task ids in the verdict. The rest stay in the verdict. A rejection
whose every issue is pre-existing is refused.

Otherwise approve. List Normal and Note findings in the summary; they stay in the
verdict for the release's polish sweep.

```text
verification action=qa_record task_id=<delivery> status=approved|rejected \
  summary="<one line: verdict and the top findings>" \
  issues='[{"severity":"high","problem":"...","suggestion":"...","file":"<screenshot>"},
          {"severity":"normal","scope":"pre-existing","problem":"...","file":"<screenshot>","repro":"<steps and wrong output>","affected":"<who acts on it today>"}]' \
  ledger_path=<ledger-dir>/LEDGER.md
```

A rejection sends the delivery back to its implementer with your ledger. The
next park opens a new round. Recording the verdict also closes your QA task,
and it cannot be revised. If you change your mind after recording, do not
record again. Message the supervisor with `blocker=true`, ask for
`request_changes` on the delivery, and name the finding.
Stop your registered servers, then report the verdict line to the supervisor.
