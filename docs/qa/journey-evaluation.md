# User-journey evaluation

Operator requirement (2026-09-23): the user-facing flows matter most. Go
through them and evaluate them, not only check that each piece works.

Component tests and per-state visual QA prove that pieces render. They do not
prove that a person can get from the real entry point to their goal without a
dead end, a confusing sentence, an extra step, lost context or a wait. A
journey is the unit that answers that question. This note defines journeys,
the suite that walks them, and the evaluation that must pass before a release
ships a changed user-facing surface.

## Terms

- **Journey** — one end-to-end flow from a real entry point (a URL, a
  notification, a cold start) to a user goal ("my reply reached the
  supervisor"). It crosses feature boundaries on purpose.
- **Stage** — one user-visible step inside a journey. In the suite, each
  stage is one `test.step`, and the screencast shows one chapter per stage.
- **Receipt** — the evidence one journey run leaves: a trace, a screencast,
  per-stage screenshots and a final aria snapshot.
- **Friction finding** — something a real user would trip over. It is scored
  on the rubric below; it is not a test failure.

## 1. Journey catalog

`docs/qa/journeys.md` has one section per user-facing surface. Each journey
has a stable id (`HUB-J3`), and the id is the contract. Tests, evaluation
reports and tasks all cite it. Each entry lists:

- **Entry**: where the user starts and in what state.
- **Goal**: what "done" means, in the user's words.
- **Steps**: the stages, in order.
- **Expected experience**: what a user should see and feel at each stage,
  including copy that must make sense to someone who has not read the code.
- **Edge paths**: the realistic detours, such as offline, empty, error,
  second machine, phone, dark theme or keyboard only.
- **Suite**: the spec that walks it, or `not automated` with the reason.

Any epic that changes a user-facing surface adds or updates the journeys it
touches, in the same epic (see `cas-qa-craft`).

## 2. Journey suite

For the Commander hub, the suite is `hub-web/e2e/journeys/`. It runs as the
`journeys` project of the single `hub-web/playwright.config.ts`, with
`@playwright/test` 1.63.

- **One test per journey**, titled with the catalog id
  (`HUB-J4 reply by typing`). Each stage is a `test.step`.
- **What it drives.** The production bundle (`hub-web/dist`), served at
  `/commander/` the way `cas hub` embeds it. At the network boundary, a hub
  protocol double serves the machine's HTTP and WebSocket API
  (`e2e/journeys/hub-double.ts`). Its label in evidence is `real-bundle,
  protocol-double`: the UI code is the shipped code, and the hub is not.
  The double replays payload shapes recorded from a real hub. When a journey
  depends on data a double cannot fake honestly, the catalog says so.
- **Receipts per journey**, written under the run's receipt directory:
  - `trace.zip`, recorded with `snapshots: { dom, aria, screen }`
  - `journey.webm`, a `page.screencast` recording with `showActions` on and
    one `showChapter` per stage
  - `NN-<stage>.png`, one screenshot at the end of every stage
  - `final.aria.yml`, an aria snapshot of the goal state
  - `result.json`, holding the id, the stages, the duration of each stage and
    the pass/fail result
- **Variants.** Phone (390×844), light and dark themes are separate journeys
  (`HUB-J9`, `HUB-J10`) that walk the core path in that mode. They are not a
  full matrix; `cas-619f`'s per-delivery pass owns the matrix.
- **Waits are measured, not hidden.** Each stage records its wall time. The
  evaluator sees a slow stage in `result.json` even when the test passed.
  Timings include the screencast's action annotations, roughly 0.3 s per
  action. Compare stages with each other and across runs, not against a
  stopwatch.
- **Browser.** The journeys project runs the full Chromium build
  (`channel: "chromium"`). In 1.63, `chromium-headless-shell` crashes the
  renderer when a conversation mounts its terminal surface.

## 3. Release-time journey evaluation

A release that changes a user-facing surface must carry a journey evaluation
of exactly the UI it ships.

1. **Run.** Before the cut, the supervisor runs
   `scripts/journey-eval.sh <artifact-dir>` on the release candidate, which is
   the assembled epic tip, with `npm ci` done in `hub-web/`. It runs the
   `journeys` project against the committed `hub-web/dist` and copies each
   journey's receipts, including its trace, to
   `<artifact-dir>/journeys/<id>/`. It also writes `SUMMARY.md`, with the
   `hub-web/dist` tree hash, the pass/fail result and the stage timings.
2. **Evaluate.** A different agent from any implementer of the epic, on the
   `taste` lane, watches every journey's screencast, reads its stage
   screenshots and timings, and scores each journey on the rubric. It then
   writes `docs/qa/journey-evaluations/<date>-hub-web-<tree8>.md` from
   `docs/qa/journey-evaluations/TEMPLATE.md`.
3. **Route findings** by severity (below). Blocking findings stop the cut.
   Every other finding becomes a task before the report is committed, and
   the report cites the task ids.
4. **Gate.** `scripts/check-journey-evaluation.sh <worktree>` runs at the
   start of the release train's `prep` stage, after `assemble`. It passes when
   either of these is true:
   - `hub-web/dist` is unchanged since the last release tag.
   - A committed report's `hub_web_dist:` line names the assembled
     `hub-web/dist` tree, it has a PASS row for every catalog journey, and it
     states `blocking_findings: 0`.

   Otherwise the train stops with the named blocker `journey-evaluation`.

The report is keyed by the git tree of `hub-web/dist`, not by the version.
`dist/` is the bundle `cas` embeds, and CI rebuilds it whenever the source
changes. The report stays valid for that exact UI whatever version number
ships it. A change to the shipped UI invalidates it; a change to a spec or a
doc does not.

### Friction rubric

Score each journey 0–3 on each dimension. 0 means none, 1 minor, 2
noticeable, 3 blocking.

| Dimension | What counts |
|---|---|
| Dead end | A state with no visible way forward or back; an error with no recovery action |
| Confusing copy | Words a user must decode: internal terms, ambiguous labels, a message that contradicts the screen |
| Extra steps | More actions than the goal needs; repeated input; a confirmation that protects nothing |
| Lost context | A draft, selection, scroll position, machine or conversation that the user did not choose to leave disappears |
| Waits | A stage that feels slow or shows no progress. Record the measured time, not the impression |

### Severity routing

| Severity | Trigger | Route |
|---|---|---|
| Blocking | Any dimension scored 3; any journey that fails or cannot reach its goal | The cut stops. Fix it in the release epic and evaluate again |
| High | Any 2 on the core journeys (pairing, finding a conversation, replying) | A P1 task, which must be fixed before the next release |
| Normal | Any other 2, or a 1 with a concrete fix | A P2 task |
| Note | A 1 with no clear fix | Recorded in the report only |

## 4. Relationship to sibling work

- **Per-delivery QA pass (`cas-619f`).** It uses the catalog to choose which
  journeys a single delivery touches, and walks those journeys before merge.
  The release evaluation walks all of them before a cut.
- **Evidence bundle (`cas-c3b8`).** Journey receipts are that bundle,
  applied to one journey: trace with aria and screen snapshots, screencast
  receipt, final aria snapshot and screenshots. They use the same directory
  layout and honesty labels.
- **Test Agents spike (`cas-d7b7`).** It shares `hub-web/playwright.config.ts`.
  Its fixture-site specs stay in the default project, and journeys are their
  own project.

## Limits

- The protocol double is not a hub. Pairing with a real relay, voice capture
  from a real microphone, and live daemon delivery are covered by their
  existing real-build proofs (`docs/design/hub-web/pairing-verification.md`),
  not by the suite. The catalog marks each journey that has such a gap.
- The evaluator judges the experience from receipts. It does not replace a
  human walking the product. It makes sure an agent with fresh eyes walked it
  first.
