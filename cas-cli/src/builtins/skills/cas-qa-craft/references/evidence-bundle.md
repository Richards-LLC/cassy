# Playwright evidence bundle

Every web or hub cell of a `demo_statement` task produces this bundle. CLI-only
cells keep terminal captures in the ledger instead. The APIs below were checked
against `@playwright/test` 1.63.0 on 2026-09-23 and arrived between 1.59 and
1.63; pin `^1.63`. On an older Playwright, the bundle row is `NOT EXERCISED`:
never substitute weaker evidence and call it PASS.

## Layout

The bundle lives at `~/.cas/artifacts/<task-id>/qa/`. `LEDGER.md` stays one
level up. An independent QA round uses `independent-qa/round-<n>/` and a
journey uses `journeys/<journey-id>/`, each with the same shape and its own
`bundle.json`. Extra files are allowed; a validator reads only what
`bundle.json` lists.

| `files` key | name | required |
| --- | --- | --- |
| `trace` | `trace.zip` | always |
| `trace_actions` | `trace-actions.txt` (`npx playwright trace actions`, ≥1 `Expect` row) | always |
| `receipt` | `receipt.webm` (screencast with action annotations and one chapter per cell) | always |
| `aria_yaml`, `aria_json` | `final.aria.yml`, `final.aria.json` (final state) | always |
| `cells` | `M01.png`, `M02.png`, … one per run cell, named by ledger id | always |
| `polish_screenshots` | `visual-qa/<slug>-{light,dark}-{desktop,phone}.png` | always |
| `visual_qa`, `visual_qa_json`, `visual_qa_stdout` | `visual-qa/visual-qa.md`, `visual-qa/visual-qa.json`, `visual-qa.stdout` | always |
| `critique` | `critique.md` (the cas-ui-craft rubric table) | always |
| `a11y` | `a11y-forced-colors.png`, `a11y-reduced-motion.png`, `a11y-contrast-more.png` | visual change |

Polish evidence is required for every web bundle, not only for visual changes.
An unpolished delivery is a defect, the same as a bug. The one exception is a
`journey` bundle ([journeys.md](journeys.md)). It may set
`visual_qa_status: "unavailable"` and omit the polish keys, because the release
journey evaluation scores polish for it.

`bundle.json` is the manifest. It records:

- `schema: 1`, `task_id`, and `producer` (`cas-qa-craft`, `independent-qa`, or `journey`)
- `head_sha`: the full SHA of the build under test
- `build_url`, `playwright_version`, `created_at` (RFC 3339)
- `visual_change` (boolean) and `visual_qa_status` (`pass`, `fail`, or `unavailable`).
  `pass` is a claim that close checks against the run's own report,
  `visual-qa/visual-qa.json`. The report must say `"status": "PASS"`, and
  its `generatedAt` must be later than the delivered commit. Every URL in
  `urls` must be a local build: loopback, `*.localhost`, or a file. The run
  must not record `"strict": false`. A run against a production or other
  remote origin never counts, because it checks what is deployed there,
  not this commit. With no run, write `unavailable`; never `pass`.
- `files`: the keys above, with paths relative to the bundle
- `critique_score`: `distinctiveness`, `fit`, `hierarchy`, `craft`, and `accessibility`, each 0–5, matching `critique.md`

## Config

```ts
// playwright.config.ts
import { defineConfig } from '@playwright/test';

const QA = process.env.QA_ARTIFACTS!; // ~/.cas/artifacts/<task-id>/qa

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

The `snapshots` keys do the following:

- `dom` keeps the DOM and network for every action.
- `aria` writes an aria tree before and after each action.
- `screen` writes a PNG before and after each action.

The same object works in library code:
`context.tracing.start({ snapshots: { dom: true, aria: true, screen: true }, sources: true })`.

## Worked example

Task `cas-1234` says: “User filters tasks and sees no matches.” The spec walks
ledger cells M01 and M02, with one chapter per cell.

```ts
// tests/qa-evidence.spec.ts
import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';

const QA = process.env.QA_ARTIFACTS!;

test('cas-1234 demo: filter to no matches', async ({ page }) => {
  await page.screencast.start({ path: `${QA}/receipt.webm`, size: page.viewportSize()! });
  await page.screencast.showActions({ position: 'top-right' });
  await page.goto('/');

  await page.screencast.showChapter('M01 submit a task filter', {
    description: 'expect: matching rows update', duration: 1000,
  });
  await page.getByRole('searchbox', { name: 'Filter' }).fill('fix');
  await page.getByRole('button', { name: 'Apply' }).click();
  await expect(page.getByRole('status')).toHaveText('1 task');
  await page.screenshot({ path: `${QA}/M01.png` });

  await page.screencast.showChapter('M02 query with no matches', {
    description: 'expect: clear no-results copy', duration: 1000,
  });
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
node <repo>/scripts/visual-qa.mjs --strict --artifact-dir "$QA/visual-qa" "$BASE_URL/" > "$QA/visual-qa.stdout" 2>&1
```

For polish evidence, also score the surface with the cas-ui-craft
`references/critique-rubric.md` into `critique.md`. Then write `bundle.json`.
The floor is distinctiveness, fit, and hierarchy each ≥ 4, and no dimension at
0. A score below the floor is a defect task, not a note. If the project has no
`scripts/visual-qa.mjs`, run the copy that ships with the cas-ui-craft skill:
`cas-ui-craft/scripts/visual-qa.mjs` in the harness skill directory
(`.claude/skills/`, `.codex/skills/` or `.grok/skills/`). Only when neither
exists, take the four renders at 1280×800 and 390×800 in light and dark with
`page.screenshot`, set `visual_qa_status: "unavailable"` and say so in the
ledger's Honesty section. The close gate accepts only `"pass"` without a
supervisor override, so ask for one with a `blocker=true` message before
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
4. Watch `receipt.webm`. Every chapter title names a ledger cell, and the
   action captions show what was clicked or typed. Then skim the four polish
   renders and `visual-qa/visual-qa.md`.

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
