import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { existsSync } from 'node:fs';

import { runVisualQa } from './visual-qa.mjs';

const here = fileURLToPath(new URL('.', import.meta.url));
const fixture = (name) => join(here, 'visual-qa-fixtures', name);
const repoRoot = join(here, '..');

/**
 * cas-9ebd: where the historical acceptance runs keep their renders. It was a
 * Linux-only /home/pippenz/.cas/artifacts/cas-f868, which does not exist on
 * macOS or a CI runner. VISUAL_QA_ARTIFACTS_ROOT keeps them somewhere durable
 * (a task's artifacts directory); by default they go under the OS temp dir,
 * like every other run in this file.
 */
async function acceptanceDir(prefix) {
  const root = join(process.env.VISUAL_QA_ARTIFACTS_ROOT || tmpdir(), 'cas-f868');
  await mkdir(root, { recursive: true });
  return mkdtemp(join(root, prefix));
}

async function revisionFixture(revision) {
  const dir = await acceptanceDir('visual-qa-revision-');
  const path = join(dir, `${revision}.html`);
  const html = execFileSync('git', ['show', `${revision}:docs/factory/2026-09-06-model-lane-rubric-review.html`], { cwd: repoRoot, encoding: 'utf8' });
  await writeFile(path, html);
  return path;
}

const acceptanceRender = (url, artifactDir) => runVisualQa({
  urls: [url],
  artifactDir,
  schemes: ['light', 'dark'],
  viewports: [
    { name: 'desktop', width: 1280, height: 800 },
    { name: 'phone', width: 390, height: 800 },
  ],
});

test('reports every planted visual defect and captures a screenshot', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-defects-'));
  const result = await runVisualQa({
    urls: [fixture('defects.html')],
    artifactDir,
    schemes: ['light'],
    viewports: [{ name: 'phone', width: 390, height: 800 }],
  });

  assert.equal(result.status, 'FAIL');
  assert.ok(result.findings.some((finding) => finding.type === 'contrast'));
  assert.ok(result.findings.some((finding) => finding.type === 'clipped-content'));
  assert.ok(result.findings.some((finding) => finding.type === 'overlapping-text'));
  assert.ok(result.findings.some((finding) => finding.type === 'invisible-text'));
  assert.ok(result.findings.some((finding) => finding.type === 'truncated-container'));
  assert.ok(result.screenshots.length === 1);
  assert.match(result.markdown, /PASS\/FAIL|FAIL/);
  assert.match(result.markdown, /contrast|clipped-content|overlapping-text/);
});

test('passes the clean fixture in light and dark at both required widths', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-clean-'));
  const result = await runVisualQa({
    urls: [fixture('clean.html')],
    artifactDir,
    strict: true,
    schemes: ['light', 'dark'],
    viewports: [
      { name: 'desktop', width: 1280, height: 800 },
      { name: 'phone', width: 390, height: 800 },
    ],
  });

  assert.equal(result.status, 'PASS');
  assert.equal(result.findings.length, 0);
  assert.equal(result.screenshots.length, 4);
  const json = JSON.parse(await readFile(join(artifactDir, 'visual-qa.json'), 'utf8'));
  const markdown = await readFile(join(artifactDir, 'visual-qa.md'), 'utf8');
  assert.equal(json.status, 'PASS');
  assert.equal(json.strict, true);
  assert.deepEqual(json.urls, [fixture('clean.html')]);
  assert.match(markdown, /^# Visual QA — PASS\n/);
  assert.deepEqual(json.schemes, ['light', 'dark']);
  assert.deepEqual(json.viewports.map(({ width }) => width), [1280, 390]);
});

test('measures the settled colour of a 100 ms colour transition, identically on every run', async () => {
  const runs = [];
  for (let run = 0; run < 3; run += 1) {
    const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-transition-'));
    const result = await runVisualQa({
      urls: [fixture('transition.html')],
      artifactDir,
      strict: true,
      schemes: ['light', 'dark'],
      viewports: [
        { name: 'desktop', width: 1280, height: 800 },
        { name: 'phone', width: 390, height: 800 },
      ],
    });
    runs.push(result.findings.map(({ type, selector, scheme, viewport }) => `${type} ${selector} ${scheme} ${viewport.width}`));
    assert.deepEqual(runs.at(-1), [], `run ${run + 1} measured a colour before the transition settled`);
    assert.equal(result.status, 'PASS');
  }
  assert.deepEqual(runs[1], runs[0]);
  assert.deepEqual(runs[2], runs[0]);
});

test('parses computed OKLCH colors without false invisible-text findings', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-oklch-'));
  const oklchPage = join(artifactDir, 'oklch.html');
  await writeFile(oklchPage, `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <style>
    body { margin: 0; font: 16px/1.4 Arial, sans-serif; color: oklch(20% 0 0); background: oklch(100% 0 0); }
    p { margin: 16px; }
    #opaque { color: oklch(20% 0 0); }
    #half { color: oklch(20% 0 0 / 50%); }
    #colored { color: oklch(42% 0.15 250); }
    #colored-background { color: oklch(95% 0.02 250); background: oklch(25% 0.04 250); }
    #transparent { color: oklch(20% 0 0 / 0); }
    #low-contrast { color: oklch(95% 0 0); }
    #unsupported { color: color(display-p3 0 0 0); }
  </style>
</head>
<body>
  <p id="opaque">Opaque OKLCH text is visible.</p>
  <p id="half">Half alpha OKLCH text is visible.</p>
  <p id="colored">Colored OKLCH text is visible.</p>
  <p id="colored-background">OKLCH text on OKLCH background is visible.</p>
  <p id="transparent">Transparent OKLCH text is invisible.</p>
  <p id="low-contrast">Pale OKLCH text has low contrast.</p>
  <p id="unsupported">Other CSS color formats need a contrast check.</p>
</body>
</html>
`);
  const result = await runVisualQa({
    urls: [oklchPage],
    artifactDir,
    strict: true,
    schemes: ['light'],
    viewports: [{ name: 'desktop', width: 1280, height: 800 }],
  });

  const invisible = result.findings.filter((finding) => finding.type === 'invisible-text');
  assert.deepEqual(invisible.map((finding) => finding.selector), ['#transparent']);
  assert.equal(invisible[0].reason, 'color-alpha-0');
  assert.ok(result.findings.some((finding) => finding.type === 'contrast' && finding.selector === '#low-contrast'));
  assert.deepEqual(result.infoFindings.filter((finding) => finding.type === 'unverifiable-contrast').map((finding) => finding.selector), ['#unsupported']);
});

test('allowlist requires a reason and suppresses intentional findings', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-allowlist-'));
  const allowlistPath = join(artifactDir, 'allowlist.json');
  await writeFile(
    allowlistPath,
    JSON.stringify({
      entries: [
        {
          type: 'contrast',
          selector: '#intentional',
          reason: 'Brand mark is an image-backed wordmark reviewed by design.',
        },
      ],
    }),
  );
  const result = await runVisualQa({
    urls: [fixture('allowlisted.html')],
    artifactDir,
    schemes: ['light'],
    viewports: [{ name: 'phone', width: 390, height: 800 }],
    allowlistPath,
  });

  assert.equal(result.status, 'PASS');
  assert.equal(result.findings.length, 0);
  assert.equal(result.suppressed.length, 1);
  assert.match(result.suppressed[0].reason, /Brand mark/);
});

test('aria-hidden drawer content is ignored while unhidden drawer content remains a finding', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-aria-hidden-'));
  const result = await runVisualQa({
    urls: [join(repoRoot, 'scripts', 'visual-qa-fixtures', 'allowlist-aria-hidden.html')],
    artifactDir,
    schemes: ['light'],
    viewports: [{ name: 'phone', width: 390, height: 800 }],
    strict: true,
  });

  const invisibleFindings = result.findings.filter((finding) => finding.type === 'invisible-text');
  assert.equal(result.status, 'FAIL');
  assert.equal(invisibleFindings.length, 1);
  assert.equal(result.suppressed.length, 0);
  assert.match(invisibleFindings[0].elementPath, /machine-drawer/);
});

test('invalid allowlist selectors are informational failures of configuration', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-invalid-allowlist-'));
  const allowlistPath = join(artifactDir, 'allowlist.json');
  await writeFile(allowlistPath, JSON.stringify({ entries: [{ type: 'contrast', selector: '[', reason: 'Invalid test selector.' }] }));
  const result = await runVisualQa({
    urls: [fixture('clean.html')],
    artifactDir,
    schemes: ['light'],
    viewports: [{ name: 'phone', width: 390, height: 800 }],
    allowlistPath,
  });

  assert.equal(result.status, 'PASS');
  assert.equal(result.infoFindings.filter((finding) => finding.type === 'invalid-allowlist-selector').length, 1);
});

test('strict mode exposes a non-zero exit code for any finding', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-strict-'));
  const result = await runVisualQa({
    urls: [fixture('defects.html')],
    artifactDir,
    schemes: ['light'],
    viewports: [{ name: 'phone', width: 390, height: 800 }],
    strict: true,
  });

  assert.equal(result.status, 'FAIL');
  assert.equal(result.exitCode, 1);
  // The report records the strict run, which is what the close gate counts (cas-a6a3).
  const report = JSON.parse(await readFile(join(artifactDir, 'visual-qa.json'), 'utf8'));
  assert.equal(report.strict, true);
  assert.ok(Date.parse(report.generatedAt) > 0);
  assert.deepEqual(report.urls, [fixture('defects.html')]);
});

test('reports content lost when JavaScript is disabled or print media applies', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-media-'));
  const result = await runVisualQa({
    urls: [fixture('media-loss.html')],
    artifactDir,
    schemes: ['light'],
    viewports: [{ name: 'phone', width: 390, height: 800 }],
  });

  assert.ok(result.findings.some((finding) => finding.type === 'javascript-disabled-loss'));
  assert.ok(result.findings.some((finding) => finding.type === 'print-loss'));
});

test('acceptance surfaces pass and the historical Figure 3 defect fails', async () => {
  const artifactDir = await acceptanceDir('visual-qa-acceptance-');
  const exemplarNames = ['product-page.html', 'report.html', 'dashboard.html', 'before-after.html'];
  for (const name of exemplarNames) {
    const result = await acceptanceRender(
      join(repoRoot, 'cas-cli/src/builtins/skills/cas-ui-craft/references/exemplars', name),
      join(artifactDir, `exemplar-${name}`),
    );
    assert.equal(result.status, 'PASS', `${name} should pass: ${JSON.stringify(result.counts)}`);
  }

  const cleanReview = await acceptanceRender(await revisionFixture('2420a246'), join(artifactDir, 'review-2420a246'));
  assert.equal(cleanReview.status, 'PASS', `2420a246 should pass: ${JSON.stringify(cleanReview.counts)}`);

  const defectiveReview = await acceptanceRender(await revisionFixture('cbac967b'), join(artifactDir, 'review-cbac967b'));
  assert.equal(defectiveReview.status, 'FAIL');
  assert.ok(defectiveReview.findings.some((finding) => finding.elementPath.includes('fig3cap') && finding.otherElementPath.includes('figure')));
});

// cas-9178 (GH #1023 finding 7): declared journeys render the states a user
// reaches after interacting, not just the resting page.
const journeyFile = fixture('journey-start.json');
const both = {
  schemes: ['light', 'dark'],
  viewports: [
    { name: 'desktop', width: 1280, height: 800 },
    { name: 'phone', width: 390, height: 800 },
  ],
};

test('a resting-page run misses the planted post-interaction defect', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-journey-rest-'));
  const result = await runVisualQa({ urls: [fixture('journey-start.html')], artifactDir, strict: true, ...both });
  assert.equal(result.status, 'PASS', JSON.stringify(result.counts));
  assert.equal(result.journeyRuns, undefined);
});

test('a declared journey renders loading, submit-error and offline states and fails strict on the planted defect', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-journey-'));
  const result = await runVisualQa({ journey: journeyFile, artifactDir, strict: true, ...both });
  assert.equal(result.status, 'FAIL');
  assert.equal(result.exitCode, 1);
  // The page at rest is still checked (and is clean), then every state on every scheme and viewport.
  assert.deepEqual(result.urls, [pathToFileURL(fixture('journey-start.html')).href]);
  assert.equal(result.screenshots.length, 4 + 3 * 4);
  assert.equal(result.journeyRuns.length, 3 * 4);
  assert.deepEqual([...new Set(result.journeyRuns.map((run) => run.state))], ['loading', 'submit-error', 'offline']);
  // Findings come only from the states the defect lives in, never the resting page or the loading state.
  assert.ok(result.findings.every((finding) => finding.state === 'submit-error' || finding.state === 'offline'), JSON.stringify(result.findings.map((finding) => [finding.state, finding.type])));
  const errorFindings = result.findings.filter((finding) => finding.state === 'submit-error');
  assert.ok(errorFindings.some((finding) => finding.type === 'clipped-content' || finding.type === 'truncated-container'), JSON.stringify(result.counts));
  assert.ok(errorFindings.some((finding) => finding.type === 'contrast'), JSON.stringify(result.counts));
  // Focus dropped on the page body after the error is an unmet expectation.
  const focus = errorFindings.find((finding) => finding.type === 'journey-expectation');
  assert.match(focus.reason, /#email is not focused \(focus is on the page body\)/);
  // Every run records its steps, a screenshot and a trace.
  for (const run of result.journeyRuns) {
    assert.ok(existsSync(join(artifactDir, run.screenshot)), run.screenshot);
    assert.ok(run.trace && existsSync(join(artifactDir, run.trace)), `${run.state} trace`);
    assert.ok(run.steps.length >= 3);
    assert.ok(run.steps.slice(0, 2).every((step) => step.status === 'ok'), JSON.stringify(run.steps));
  }
  const loading = result.journeyRuns.find((run) => run.state === 'loading');
  assert.deepEqual(loading.steps.map((step) => step.status), ['ok', 'ok', 'ok']);
  const markdown = await readFile(join(artifactDir, 'visual-qa.md'), 'utf8');
  assert.match(markdown, /## Journey states — start/);
  assert.match(markdown, /\*\*submit-error\*\* · light · phone — screenshot/);
  assert.match(markdown, /UNMET — `expect #email focused=true`/);
  assert.match(markdown, /state submit-error · light/);
  const report = JSON.parse(await readFile(join(artifactDir, 'visual-qa.json'), 'utf8'));
  assert.deepEqual(report.journey.states, ['loading', 'submit-error', 'offline']);
  assert.equal(report.strict, true);
});

test('the same journey passes once the post-interaction defect is fixed', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-journey-fixed-'));
  const journey = JSON.parse(await readFile(journeyFile, 'utf8'));
  journey.url = `${pathToFileURL(fixture('journey-start.html')).href}?fixed=1`;
  const result = await runVisualQa({ journey, artifactDir, strict: true, ...both });
  assert.equal(result.status, 'PASS', JSON.stringify(result.findings.map((finding) => [finding.state, finding.type, finding.reason])));
  assert.ok(result.journeyRuns.every((run) => run.steps.every((step) => step.status === 'ok')));
});

test('a step that cannot run fails the state and skips the rest', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-journey-broken-'));
  const result = await runVisualQa({
    urls: [fixture('clean.html')],
    journey: { name: 'broken', timeoutMs: 300, states: [{ name: 'missing', steps: [{ click: '#nope' }, { wait: 10 }] }] },
    artifactDir,
    strict: true,
    schemes: ['light'],
    viewports: [{ name: 'phone', width: 390, height: 800 }],
  });
  assert.equal(result.status, 'FAIL');
  const [run] = result.journeyRuns;
  assert.deepEqual(run.steps.map((step) => step.status), ['failed', 'skipped']);
  assert.ok(result.findings.some((finding) => finding.type === 'journey-step-failed' && /click #nope/.test(finding.reason)));
});

test('journey files are checked before anything renders', async () => {
  const { loadJourney } = await import('./visual-qa.mjs');
  await assert.rejects(loadJourney({ states: [] }), /non-empty states/);
  await assert.rejects(loadJourney({ states: [{ name: 'a', steps: [{ click: '#x', fill: '#y' }] }] }), /exactly one action/);
  await assert.rejects(loadJourney({ states: [{ name: 'a' }, { name: 'a' }] }), /used twice/);
  await assert.rejects(loadJourney({ states: [{ name: 'a', routes: [{ status: 500 }] }] }), /url pattern/);
  const loaded = await loadJourney(journeyFile);
  assert.equal(loaded.url, pathToFileURL(fixture('journey-start.html')).href);
  await assert.rejects(runVisualQa({ urls: [fixture('clean.html'), fixture('defects.html')], journey: { states: [{ name: 'a' }] } }), /exactly one URL/);
});

test('the command line takes --journey', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-journey-cli-'));
  let status = 0;
  let output = '';
  try {
    output = execFileSync(process.execPath, [join(here, 'visual-qa.mjs'), '--strict', '--scheme', 'light', '--viewport', '390x800', '--artifact-dir', artifactDir, '--journey', journeyFile], { encoding: 'utf8', env: process.env });
  } catch (error) {
    status = error.status;
    output = `${error.stdout}`;
  }
  assert.equal(status, 1);
  assert.match(output, /FAIL journey-expectation \[submit-error\] #email/);
  const report = JSON.parse(await readFile(join(artifactDir, 'visual-qa.json'), 'utf8'));
  assert.equal(report.journeyRuns.length, 3);
});

test('text a vertical scroller reaches is not clipped; a fixed-height hidden box still is (cas-0d16)', async () => {
  const run = (name) => mkdtemp(join(tmpdir(), `visual-qa-${name}-`)).then((artifactDir) => runVisualQa({
    urls: [fixture(`${name}.html`)],
    artifactDir,
    strict: true,
    schemes: ['light', 'dark'],
    viewports: [
      { name: 'desktop', width: 1280, height: 800 },
      { name: 'phone', width: 390, height: 800 },
    ],
  }));

  // overflow-x: hidden; overflow-y: auto inside an overflow: hidden panel:
  // the keys below the fold are reachable by scrolling.
  const reachable = await run('scroller-reachable');
  assert.deepEqual(reachable.findings.filter((finding) => finding.type === 'clipped-content'), []);
  assert.equal(reachable.status, 'PASS', JSON.stringify(reachable.findings, null, 2));

  const clipped = await run('clip-box');
  assert.equal(clipped.status, 'FAIL');
  assert.ok(
    clipped.findings.some((finding) => finding.type === 'clipped-content' && finding.reason === 'text-bounds-exceed-overflow-ancestor'),
    JSON.stringify(clipped.findings, null, 2),
  );
});

test('text folded inside a closed <details> is not clipped; an open disclosure that clips still is (cas-b7f2)', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-details-'));
  const result = await runVisualQa({
    urls: [fixture('details-disclosure.html')],
    artifactDir,
    strict: true,
    schemes: ['light', 'dark'],
    viewports: [
      { name: 'desktop', width: 1280, height: 800 },
      { name: 'phone', width: 390, height: 800 },
    ],
  });
  const clipped = result.findings.filter((finding) => finding.type === 'clipped-content');
  // Folded content has a layout box past the scroller's range but is not
  // drawn; opening its summary brings it into the range (cas-6b75 QA F01).
  assert.deepEqual(clipped.filter((finding) => /Folded turn|machine:read/.test(finding.textSample ?? '')), [], JSON.stringify(clipped, null, 2));
  // The summaries are drawn and still checked: none is clipped.
  assert.deepEqual(clipped.filter((finding) => /Earlier session|Technical details/.test(finding.textSample ?? '')), []);
  // A real clip inside an open disclosure still fails.
  assert.equal(result.status, 'FAIL');
  assert.ok(clipped.some((finding) => /lost below the edge/.test(finding.textSample ?? '')), JSON.stringify(result.findings, null, 2));
  // Every clip found is that real one: its lost line or the box that loses it.
  assert.deepEqual(clipped.filter((finding) => !/lost below the edge/.test(finding.textSample ?? '') && !finding.elementPath.endsWith('div.clip')), [], JSON.stringify(clipped, null, 2));
});

test('visually hidden helpers, an intentional ellipsis and closed drawers pass strict; real defects still fail (GH #1081)', async () => {
  const run = (name) => mkdtemp(join(tmpdir(), `visual-qa-${name}-`)).then((artifactDir) => runVisualQa({
    urls: [fixture(`${name}.html`)],
    artifactDir,
    strict: true,
    schemes: ['light', 'dark'],
    viewports: [
      { name: 'desktop', width: 1280, height: 800 },
      { name: 'phone', width: 390, height: 800 },
    ],
  }));

  const hidden = await run('hidden-helpers');
  assert.equal(hidden.status, 'PASS', JSON.stringify(hidden.findings, null, 2));
  assert.equal(hidden.findings.length, 0);

  const real = await run('real-defects');
  assert.equal(real.status, 'FAIL');
  const has = (type, selector) => real.findings.some((finding) => finding.type === type && finding.elementPath.includes(selector));
  assert.ok(has('content-overflow', 'div:nth-of-type(1)') || real.findings.some((finding) => finding.type === 'content-overflow' && finding.selector === '#box'), JSON.stringify(real.findings, null, 2));
  assert.ok(real.findings.some((finding) => finding.type === 'clipped-content' && finding.elementPath.includes('div.alert')), JSON.stringify(real.findings, null, 2));
  assert.ok(real.findings.some((finding) => finding.type === 'outside-viewport' && finding.selector === '#lost'), JSON.stringify(real.findings, null, 2));
});
