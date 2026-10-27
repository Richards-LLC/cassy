# QA evidence by surface

## API surfaces

`qa.api_paths` selects non-rendered API/server code, including server-only TSX;
keep rendered components out of these globs. Follow [Fewest tests](../../cas-tdd/SKILL.md#fewest-tests):
one transcript per changed status/error code, including refusal cases. The only
required artifacts are `LEDGER.md` (the transcript itself) and `contracts.diff`
(captured comparison against vendored contract fixtures, empty when they agree).
Record `head_sha: <delivered SHA>` and `origin: <HTTP origin>` in the transcript,
a PASS/real-build ledger row for approval, then `request: <METHOD> <absolute URL>`,
`status: <numeric code>` and `body: <captured response>` for each cell.
Exercise the real server build (for example `next build` then `next start`), or
an origin in `qa.deployed_origins` with project-provided sandbox credentials.
For deployed runs, add `deployed_sha: <SHA>` covering this delivery and
`deployment_proof: <version endpoint response naming that SHA>` inline.
Redact entire auth/cookie values; record sandbox reset and verified zero smoke
rows inline. API-only evidence needs no screenshots, trace or `bundle.json`;
mixed rendered changes still require the web bundle below.

## Playwright evidence bundle

Rendered web/hub cells use this bundle with `@playwright/test` `^1.63`.
Older Playwright means `NOT EXERCISED`. CLI cells use terminal captures.

## Layout

The bundle lives at `~/.cas/artifacts/<project-key>/<task-id>/qa/`. `LEDGER.md` stays one
level up. An independent QA round uses `independent-qa/round-<n>/` and a
journey uses `journeys/<journey-id>/`, each with the same shape and its own
`bundle.json`.

| `files` key | name | required |
| --- | --- | --- |
| `trace` | `trace.zip` | always |
| `trace_actions` | `trace-actions.txt` (`npx playwright trace actions`, ≥1 `Expect` row) | always |
| `receipt` | `receipt.webm` (screencast with action annotations and one chapter per cell) | `visual_change: true` |
| `aria_yaml`, `aria_json` | `final.aria.yml`, `final.aria.json` (final state) | always |
| `cells` | `M01.png`, `M02.png`, … one per run cell, named by ledger id | always |
| `polish_screenshots` | `visual-qa/<slug>-{light,dark}-{desktop,phone}.png` | always |
| `visual_qa`, `visual_qa_json`, `visual_qa_stdout` | `visual-qa/visual-qa.md`, `visual-qa/visual-qa.json`, `visual-qa.stdout` | always |
| `visual_qa_baseline_json` | `visual-qa-baseline/visual-qa.json` (the same strict run against a local build of the base commit) | `visual_qa_status: "scoped"` |
| `critique` | `critique.md` (the cas-ui-craft rubric table) | always |
| `a11y` | `a11y-forced-colors.png`, `a11y-reduced-motion.png`, `a11y-contrast-more.png` | visual change |
| `pairs` | `pairs/pairs.json` ([before/after pairs](#beforeafter-pairs)) | visual change with qa.capture configured, unless reasoned unavailable |

Use the existing `visual_change` flag for changes to rendered appearance:
layout, colour, typography, imagery or motion. For `visual_change: false`, omit
`files.receipt`, screencast/chapter calls and media-mode captures. Keep the
trace, assertions, ARIA, cell captures, polish and affected journey receipts/logs.
A recording listed voluntarily still needs to be valid and fresh.

Every web bundle needs polish evidence. A
`journey` bundle ([journeys.md](journeys.md)) may set
`visual_qa_status: "unavailable"` and omit the polish keys, because the release
journey evaluation scores polish for it.

`bundle.json` records:

- `schema: 1`, `task_id`, and `producer` (`cas-qa-craft`, `independent-qa`, or `journey`)
- `head_sha`: the full SHA of the build under test
- `build_url`, `playwright_version`, `created_at` (RFC 3339)
- `visual_change` (boolean) and `visual_qa_status` (`pass`, `fail`, or `unavailable`).
  `pass` is a claim that close checks against the run's own report,
  `visual-qa/visual-qa.json`. The bundled script reports `"status": "PASS"`.
  A project script's single-source report may instead say `"totalIssues": 0`
  with nonempty `renders`, `validRenders` equal to the render count, and
  `"strict": true`. In either format, the JSON report is the verdict; the
  Markdown heading and stdout footer may include the source name and counts.
  The report's `generatedAt` must be later than the delivered commit. Every URL in
  `urls` (or a single-source `input`) must be a local build: loopback,
  `*.localhost`, or a file. The run
  must not record `"strict": false`. A run against a production or other
  remote origin never counts, because it checks what is deployed there,
  not this commit. With no run, write `unavailable`; never `pass`.
  `scoped` is for a page that already had findings before this delivery. It
  needs `files.visual_qa_baseline_json`: the same strict script run against a
  local build of the base commit (`git merge-base <target> <head>`) over the
  same pages. Close accepts it when every finding in the delivered build's
  report pairs once with a base finding by type, rule/reason, page, render
  state, scheme and full viewport. Element identity uses role + accessible
  name when supplied, or matching text + shared bounds within 0.5 CSS pixels.
  CSS class renames do not add a finding when that identity agrees. Historical
  reports without identity evidence require matching selectors; keep them
  fail-closed. The delivered run must
  still be fresh, strict and local. Classify base-build findings under
  [independent-pass §3](independent-pass.md#3-correctness-paths): failures within
  the delivery's stated purpose, acceptance criteria, demo statement, or a
  journey invariant its diff adds or edits are unmet even when base shares them.
  Only findings outside those scopes are pre-existing follow-ups in the ledger.
  Both reports must record their findings in one of two shapes: a top-level
  `findings` list (the builtin visual-qa.mjs), or per render as
  `renders[].issues` with the page in `input` (a per-source project
  visual-qa.mjs). Close compares either shape the same way.
- `files`: the keys above, with paths relative to the bundle
- `critique_score`: `distinctiveness`, `fit`, `hierarchy`, `craft`, and `accessibility`, each 0–5, matching `critique.md`
- `pairs_status` (visual change only): `complete`, `partial`, or `unavailable`, as defined in
  [before/after pairs](#beforeafter-pairs)
- `deployed` (optional): use it only when a local build cannot
  authenticate, for example when the staging backend's CORS rejects
  localhost on its session endpoint. The run then goes against the deployed
  origin, with these fields:
  - `origin`: the deployment, which must be listed in the project's
    `qa.deployed_origins`;
  - `reason`: why local auth is impossible, in a full sentence;
  - `deployed_sha`: the full commit the deployment serves. It must be the
    delivered head or a descendant of it;
  - `deployment_proof`: a bundle file recording what the deployment reports
    about itself (its version endpoint or deployment metadata). It must name
    `deployed_sha`.

  The visual-QA report's URLs may then be on that origin. The bundle must
  carry no credential: no storage state, cookie, session JWT, bearer token
  or auth header, including the headers Playwright records in `trace.zip`'s
  network log. Close refuses such a bundle and names the file, never the
  value. Log in through a setup project that is not traced. Before listing
  the trace, replace each entire cookie or authorization value with exactly
  `REDACTED`, `[REDACTED]`, `<redacted>`, `***`, or an empty value;
  `visual-qa.mjs --scrub-trace` does this for the runner's trace. These
  placeholders pass in JSON headers, text headers and cookie arrays; a real
  value of 8 or more characters is refused. A placeholder embedded in a real
  value does not exempt it. Saved storage-state files, session JWTs, bearer
  tokens and API tokens remain refused.

## Before/after pairs

Reports and release notes take their screenshots from one place per delivery
task, so they can find them without asking:
`~/.cas/artifacts/<project-key>/<task-id>/qa/pairs/`. An independent round or a
journey that captures a pair writes it there too, not under its own directory.
For `visual_change: true`, run `cas deliverable capture --task <task-id> --target <target> --side both`.
Projects with `[qa.capture]` configured require `files.pairs` or `pairs_status: "unavailable"`
with a one-line `pairs_reason` in the bundle. An unavailable capture always permits close
when no pairs are cited; a base build that cannot serve does not block delivery.
Without capture configuration, absence is advisory. On deploy-only projects, run
`cas deliverable capture --task <task-id> --side before` at task start, before deploying.

1. **Capture both sides in one session.** Before comes from a local build of
   the merge base (`git merge-base <target> <head>`), or, when that cannot be
   served, from the deployed production origin while the change is live only
   on staging. After comes from the delivered build. Keep viewport, scheme,
   login, data and masks identical, so the only difference is this delivery.
   Use real captures only; never mock, edit or reconstruct a side.
2. **Write the files.** `<id>.before.png` and `<id>.after.png` hold the full
   captures; `<id>.before.jpg` and `<id>.after.jpg` are the embed copies, at
   most 1440 px wide, quality 80 and 250 KB each. `<id>` is a stable
   kebab-case shot name; reuse it for the same surface in later deliveries.
3. **Write `pairs.json`.** Each side records its files, both PNG and JPG SHA-256 digests, the
   commit or origin it came from, its source and its capture time. A shot with
   only one side goes in `unpaired[]`: it is kept for the record and never rendered.

   ```json
   {"schema": 1, "task_id": "<task-id>", "base_sha": "<full SHA>", "head_sha": "<full SHA>", "origin": "<build URL>",
    "pairs": [{"id": "saved-filters", "surface": "/projects", "viewport": [1280,800], "scheme": "light", "masked": [".avatar"],
      "before": {"jpg": "saved-filters.before.jpg", "png": "saved-filters.before.png", "sha256": "<PNG hex>", "jpg_sha256": "<JPG hex>", "ref": "<base SHA>", "source": "merge-base", "captured_at": "<RFC 3339>"},
      "after": {"jpg": "saved-filters.after.jpg", "png": "saved-filters.after.png", "sha256": "<PNG hex>", "jpg_sha256": "<JPG hex>", "ref": "<head SHA>", "source": "delivered-build", "captured_at": "<RFC 3339>"}}],
    "unpaired": []}
   ```

   Use each stable `id` as the cell identity on both sides. Entries in `unpaired[]`
   use the same row shape, with just `before` or `after`; each present side must
   name nonempty `png` and `jpg` files with matching `sha256` and `jpg_sha256`,
   resolved relative to, and confined inside, the directory containing `pairs.json`.
   Every `pairs[]` row needs both sides; every `unpaired[]` row needs exactly one.
   Keep every baseline cell, including cells with no AFTER capture. Close compares `pairs[]` and `unpaired[]` together:
   AFTER must contain every BEFORE id, and may add ids. A partial comparison that
   drops BEFORE cells refuses and names all missing ids, even with no AFTER cells.
   Capture the missing cells, or obtain an existing recorded exact-tip `qa_waive`
   or `supervisor_override`; a manifest flag or ledger reason does not waive it.
   BEFORE captures may predate the delivery; AFTER captures and `pairs.json` must
   be fresh for the delivered build. Match `pairs.json` task and head to the bundle,
   including when rebinding evidence after documentation-only commits.

4. **Record the status.** In `bundle.json`, set `files.pairs` to
   `pairs/pairs.json` and `pairs_status` to `complete` (every shot paired),
   `partial` (some shots in `unpaired[]` or capture findings) or `unavailable`
   (no complete pair). A complete status requires at least one pair and no unpaired
   rows or findings. For unavailable, give a one-line `pairs_reason` in `bundle.json`
   and the ledger; omit `files.pairs` when the manifest cannot satisfy validation.

Every consumer renders a pair the same way: only `pairs[]` entries, both
sides labelled Before and After with Before first, from the JPG copies, one
pair per change. With one side or none, it shows no image for that change.
The [release-notes rubric](../../cas-release-notes/references/RUBRIC-template.md)
and the [report contract](../../cas-html-reports/references/technical-contract.md)
add the per-surface limits.

## Affected journeys

For hub-web deliveries, run vitest, tsc and the source-impact-selected journeys
while iterating, with `scripts/journey-eval.sh <task-artifact-dir>` (default
affected, four workers) or `--affected <base>`. Close and QA do not require a
journey receipt.

With a merge queue, the queue owns the full suite; without one, the full suite must be green on the exact SHA reaching the default branch, via CI check-runs on that SHA (preferred) or the supervisor's integrated run recorded on the epic; workers run targeted tests only. Read existing receipts; each check runs once.
Workers produce affected-journey evidence; independent QA consumes matching
receipts before running missing checks (see [independent-pass.md](independent-pass.md)).
The hook denies factory worker and QA full or unfiltered runs. A failure control may rerun the failing
spec at one worker, keeping the original failure.

Documentation or ledger edits do not require a browser rerun of the QA bundle:
set optional `executed_head_sha` in the bundle when rebinding, and keep
`head_sha` as the final reviewed tip. Close proves ancestor history,
documentation-only changes and unchanged hub-web, selector, runner, bundler,
catalog and build inputs. A catalog edit is not exempt documentation.
This bundle rebind does not itself carry an independent QA verdict; use
[correction proof](../../cas-supervisor/references/correction-proof.md) for
the separate source-review, verdict and CI limits, including test-only changes.

## Config

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

const QA = process.env.QA_ARTIFACTS!; // ~/.cas/artifacts/<project-key>/<task-id>/qa

export default defineConfig({
  outputDir: `${QA}/test-results`,
  use: {
    baseURL: process.env.BASE_URL, // the real build under test
    trace: {
      mode: 'on',
      snapshots: { dom: true, aria: true, screen: true },
      screenshots: false, // the filmstrip shares page.screencast and shrinks receipt.webm
      sources: true,
    },
  },
});
```

The evidence trace is always the test runner's own `trace.zip`. Close counts
assertions from its `test.trace`, which records every assertion's outcome and
test-level errors. A library `context.tracing` zip (including one saved with
`saveQaTrace`) has no `test.trace`, so close refuses it.

For an authenticated run, sign in through a setup project with `trace: 'off'` that
saves `storageState`, and give the evidence project `dependencies: ['setup']`, that
`storageState`, and the trace settings above. Evaluate arguments and network headers
can still carry credentials, so scrub the runner's zip before listing it. The scrub
replaces cookie, authorization and token values with `[REDACTED]` and leaves
`test.trace` intact:

```bash
QA_TRACE_SECRETS="$SESSION_VALUE" node <skills-dir>/cas-ui-craft/scripts/visual-qa.mjs \
  --scrub-trace "$QA"/test-results/<test>/trace.zip "$QA/trace.zip"
```

`QA_TRACE_SECRETS` holds extra literal values to remove, one per line; never pass
them as arguments. A failed scrub writes nothing. Delete the raw `test-results`
trace and the storage state after scrubbing: neither may be listed in the bundle.
Use `redactQaText(error, secrets)` at logging boundaries. A library-only diagnostic
harness may still use `saveQaTrace(context, path, { secrets })` and
`closeQaContext(context)`, but its trace is not close evidence.

## Worked example

Task `cas-1234` says: “User filters tasks and sees no matches.” The spec walks
ledger cells M01 and M02. Set `visualChange` to the same value as the bundle's
`visual_change`: only visual changes record chapters and media-mode captures.

```ts
// tests/qa-evidence.spec.ts
import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';

const QA = process.env.QA_ARTIFACTS!;
const visualChange = true; // match bundle.json visual_change for this delivery

test('cas-1234 demo: filter to no matches', async ({ page }) => {
  if (visualChange) {
    await page.screencast.start({ path: `${QA}/receipt.webm`, size: page.viewportSize()! });
    await page.screencast.showActions({ position: 'top-right' });
  }
  await page.goto('/');

  if (visualChange) {
    await page.screencast.showChapter('M01 submit a task filter', {
      description: 'expect: matching rows update', duration: 1000,
    });
  }
  await page.getByRole('searchbox', { name: 'Filter' }).fill('fix');
  await page.getByRole('button', { name: 'Apply' }).click();
  await expect(page.getByRole('status')).toHaveText('1 task');
  await page.screenshot({ path: `${QA}/M01.png` });

  if (visualChange) {
    await page.screencast.showChapter('M02 query with no matches', {
      description: 'expect: clear no-results copy', duration: 1000,
    });
  }
  await page.getByRole('searchbox', { name: 'Filter' }).fill('zzz');
  await page.getByRole('button', { name: 'Apply' }).click();
  await expect(page.getByRole('main')).toMatchAriaSnapshot(`
    - heading "Tasks" [level=1]
    - status: No tasks match
    - list
  `);
  await page.screenshot({ path: `${QA}/M02.png` });

  await writeFile(`${QA}/final.aria.yml`, await page.ariaSnapshot());
  await writeFile(`${QA}/final.aria.json`,
    JSON.stringify(await page.getByRole('main').ariaSnapshotJSON(), null, 2));

  if (visualChange) {
    // Visual change only. emulateMedia keeps omitted features: reset, then prove.
    for (const [name, query, media] of [
      ['forced-colors', '(forced-colors: active)', { forcedColors: 'active' }],
      ['reduced-motion', '(prefers-reduced-motion: reduce)', { reducedMotion: 'reduce' }],
      ['contrast-more', '(prefers-contrast: more)', { contrast: 'more' }],
    ] as const) {
      await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null, ...media });
      expect(await page.evaluate((q) => matchMedia(q).matches, query)).toBe(true);
      await expect(page.getByRole('status')).toHaveText('No tasks match');
      await page.screenshot({ path: `${QA}/a11y-${name}.png` });
    }
    await page.screencast.stop();
  }
});
```

Run and package the bundle against the real build. Run the trace commands from
the bundle directory, because `trace open` extracts into `./.playwright-cli/`
under the current directory. Outside the project, use
`npx --prefix <project> playwright` so that the project's pinned version runs.

```bash
export QA=~/.cas/artifacts/cas-1234/qa BASE_URL=http://127.0.0.1:4173
QA_ARTIFACTS=$QA npx playwright test tests/qa-evidence.spec.ts
cp "$QA"/test-results/*/trace.zip "$QA/trace.zip"
cd "$QA" && npx playwright trace open trace.zip
npx playwright trace actions > trace-actions.txt
npx playwright trace close && rmdir .playwright-cli
npm exec --yes --package=playwright -- node <skills-dir>/cas-ui-craft/scripts/visual-qa.mjs --strict --artifact-dir "$QA/visual-qa" "$BASE_URL/" > "$QA/visual-qa.stdout" 2>&1
```

For polish evidence, also score what the delivery changed with the cas-ui-craft
`references/critique-rubric.md` into `critique.md`, not the page's older
backlog. Then write `bundle.json`.
The floor is distinctiveness, fit, and hierarchy each ≥ 4, and no dimension at
0. A score below the floor is a defect task, not a note. `<skills-dir>` is the
harness skill directory (`.claude/skills`, `.codex/skills` or `.grok/skills`);
the script ships with the cas-ui-craft skill. Only when it is missing (an
install that predates it), take the four renders at 1280×800 and 390×800 in light and dark with
`page.screenshot`, set `visual_qa_status: "unavailable"` and say so in the
ledger's Honesty section. The close gate accepts only `"pass"` or `"scoped"`
without a supervisor override, so ask for one with a `blocker=true` message before
closing. Point `BASE_URL` at your own local serve of
the delivered commit's build, never the deployed site.

Cite the bundle in the ledger rows: the evidence path is `qa/M01.png`. Also
cite it in one typed note,
`task action=notes note_type=platform_proof notes="qa-bundle: <abs>/qa/bundle.json"`,
and repeat that path in the close reason.

## Supervisor review, in about two minutes

1. Open `bundle.json`. Check that `head_sha` is the delivered commit and that
   `critique_score` holds the floor.
2. `cd <bundle> && npx playwright trace open trace.zip`, then:
   - `npx playwright trace actions --errors-only` must print no rows.
   - `npx playwright trace actions --grep Expect` lists the assertions.
3. Drill into a doubtful step:
   - `npx playwright trace action <N>` shows its log and source line.
   - `npx playwright trace snapshot <N> --phase after` prints the aria tree at that moment.
   - Run `npx playwright trace close` when done.
4. For `visual_change: true`, watch `receipt.webm`: every chapter names a
   ledger cell and the action captions show the interaction. For either kind
   of change, skim the four polish renders and `visual-qa/visual-qa.md`.

`cas-playwright-debug` owns diagnosis when a step fails.

## Measured gotchas (1.63.0)

- `trace.screenshots: true` capped the screencast at 800×450 content inside a
  1280×720 frame. With `screenshots: false` it is full size. The per-action
  PNGs from `snapshots.screen` sit in the zip under
  `screenshots/call@<n>-after.png`. `trace screenshot <N>` reads only the
  filmstrip, so it reports “No screenshot found”.
- `page.screencast.start()` without `size` records 800×450. Pass
  `page.viewportSize()`.
- `showChapter` blocks for its `duration` (default 2000 ms). `showActions`
  returns a disposable; the annotations stop when it is disposed.
- `emulateMedia` keeps every feature you omit, so reset all three modes each
  time. A matching `matchMedia` query is the proof that the mode took effect;
  a screenshot identical to the default is not.
