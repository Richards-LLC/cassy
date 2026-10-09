import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { existsSync } from 'node:fs';

import { runVisualQa } from './visual-qa.mjs';
import { runVisualQa as runBuiltinVisualQa } from '../cas-cli/src/builtins/skills/cas-ui-craft/scripts/visual-qa.mjs';

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
  const clipped = result.findings.find((finding) => finding.type === 'clipped-content');
  assert.ok(clipped);
  assert.ok(clipped.textSample, 'renamed elements retain text identity');
  assert.ok(['x', 'y', 'width', 'height'].every((field) => Number.isFinite(clipped.textBounds[field])));
  assert.ok(clipped.textBounds.width > 0 && clipped.textBounds.height > 0);
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
    #wide-gamut { color: color(display-p3 0 0 0); }
  </style>
</head>
<body>
  <p id="opaque">Opaque OKLCH text is visible.</p>
  <p id="half">Half alpha OKLCH text is visible.</p>
  <p id="colored">Colored OKLCH text is visible.</p>
  <p id="colored-background">OKLCH text on OKLCH background is visible.</p>
  <p id="transparent">Transparent OKLCH text is invisible.</p>
  <p id="low-contrast">Pale OKLCH text has low contrast.</p>
  <p id="wide-gamut">Wide gamut text is also checked.</p>
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
  assert.deepEqual(result.infoFindings.filter((finding) => finding.type === 'unverifiable-contrast'), []);
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

for (const [name, inspect] of [['repository', runVisualQa], ['builtin', runBuiltinVisualQa]]) {
  test(`${name} ignores only text a loading marker hides behind a painted placeholder (cas-3791)`, async () => {
    const dir = await mkdtemp(join(tmpdir(), `visual-qa-pending-prices-${name}-`));
    const original = await readFile(fixture('pending-prices.html'), 'utf8');
    let runId = 0;
    const run = async (html) => {
      const id = runId++;
      const url = join(dir, `case-${id}.html`);
      const artifactDir = join(dir, `artifacts-${id}`);
      await mkdir(artifactDir, { recursive: true });
      await writeFile(url, html);
      return inspect({
        urls: [url],
        artifactDir,
        strict: true,
        schemes: ['light'],
        viewports: [{ name: 'phone', width: 390, height: 844 }],
      });
    };

    const ariaBusy = original.replaceAll('[data-pricing-client-pending]', '[aria-busy="true"]').replace(' data-pricing-client-pending', ' aria-busy="true"');
    for (const [label, html] of [['#1150 repro', original], ['aria-busy region', ariaBusy]]) {
      const positive = await run(html);
      assert.equal(positive.status, 'PASS', `${label}: ${JSON.stringify(positive.findings, null, 2)}`);
      assert.deepEqual(positive.findings.filter(({ type }) => type === 'invisible-text'), []);
    }

    const unrelated = original.replace('</section>', '<p class="unrelated-hidden" style="visibility:hidden">Unrelated hidden card text</p></section>');
    const hiddenCachesWithoutMarkers = (html) => html.replace('</style>', '.plan-price > :not(.pricing-price-placeholder), .plan-highlight { visibility:hidden !important; }</style>');
    const negatives = [
      ['blank placeholder', original.replaceAll('—', ''), '$7.00*'],
      ['one card blank, the other painted', original.replace(/—(?![\s\S]*—)/, ''), '$99.00'],
      ['missing JavaScript marker', hiddenCachesWithoutMarkers(original.replace(' data-pricing-js', '')), '$7.00*'],
      ['missing pending marker', hiddenCachesWithoutMarkers(original.replace(' data-pricing-client-pending', '')), '$7.00*'],
      ['hidden placeholder', original.replace('</style>', '.pricing-price-placeholder { visibility: hidden !important; }</style>'), '$7.00*'],
      ['transparent placeholder', original.replace('</style>', '.pricing-price-placeholder { color: transparent !important; }</style>'), '$7.00*'],
      ['zero-size placeholder', original.replace('</style>', '.pricing-price-placeholder { display:inline-block!important; width:0!important; height:0!important; font-size:0!important; overflow:hidden!important; }</style>'), '$7.00*'],
      ['unrelated hidden card text', unrelated, 'Unrelated hidden card text'],
    ];
    for (const [label, html, expectedText] of negatives) {
      const result = await run(html);
      const invisible = result.findings.filter(({ type }) => type === 'invisible-text');
      assert.equal(result.status, 'FAIL', `${label} unexpectedly passed strict QA`);
      assert.ok(invisible.some(({ textSample }) => textSample.includes(expectedText)), `${label}: ${JSON.stringify(invisible, null, 2)}`);
    }
  });
}

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

for (const [name, inspect] of [['repository', runVisualQa], ['builtin', runBuiltinVisualQa]]) {
  test(`${name} honours a reasoned JavaScript requirement while undeclared pages still fail`, async () => {
    const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-js-required-'));
    const reason = 'This application needs JavaScript to reach machines and pair devices.';
    const html = (declaration) => `<!doctype html><html lang="en"><head>
      <meta charset="utf-8">${declaration}
      <style>body { margin: 24px; color: #172033; background: #fff; font: 16px/1.4 Arial, sans-serif; }</style>
      </head><body><noscript>Enable JavaScript to reach your machines.</noscript><main></main>
      <script>document.querySelector('main').textContent = ${JSON.stringify('This application shows live machine status, pairs devices and lets operators respond to their workers. '.repeat(4))};</script>
      </body></html>`;
    const declared = join(artifactDir, 'declared.html');
    const undeclared = join(artifactDir, 'undeclared.html');
    await writeFile(declared, html(`<meta name="visual-qa:requires-javascript" content="${reason}">`));
    await writeFile(undeclared, html(''));
    const options = { strict: true, schemes: ['light'], viewports: [{ name: 'phone', width: 390, height: 800 }] };
    const accepted = await inspect({ ...options, urls: [declared], artifactDir: join(artifactDir, 'accepted') });
    assert.equal(accepted.status, 'PASS', JSON.stringify(accepted.findings));
    assert.equal(accepted.exitCode, 0);
    assert.equal(accepted.findings.some(({ type }) => type === 'javascript-disabled-loss'), false);
    assert.deepEqual(accepted.pageDeclarations, [{ url: declared, requiresJavaScript: true, reason }]);
    assert.match(accepted.markdown, /JavaScript required/);
    assert.ok(accepted.markdown.includes(reason));
    const rejected = await inspect({ ...options, urls: [undeclared],
      allowlistPath: join(repoRoot, 'hub-web/visual-qa-allowlist.json'), artifactDir: join(artifactDir, 'rejected') });
    assert.equal(rejected.status, 'FAIL');
    assert.equal(rejected.exitCode, 1);
    assert.ok(rejected.findings.some(({ type }) => type === 'javascript-disabled-loss'));
    const mixed = await inspect({ ...options, urls: [declared, undeclared], artifactDir: join(artifactDir, 'mixed') });
    assert.equal(mixed.exitCode, 1, 'one declared page must not exempt its undeclared neighbour');
    assert.deepEqual(mixed.pageDeclarations, accepted.pageDeclarations);
    assert.deepEqual(mixed.findings.filter(({ type }) => type === 'javascript-disabled-loss').map(({ url }) => url), [undeclared]);
  });
}

test('JavaScript requirement needs a reason and cannot waive print loss', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-js-required-controls-'));
  const original = await readFile(fixture('media-loss.html'), 'utf8');
  const options = { strict: true, schemes: ['light'], viewports: [{ name: 'phone', width: 390, height: 800 }] };
  for (const [name, metadata, expected] of [
    ['empty', '<meta name="visual-qa:requires-javascript" content=" ">', 'invalid-javascript-requirement'],
    ['duplicate', '<meta name="visual-qa:requires-javascript" content="A reason"><meta name="visual-qa:requires-javascript" content="Another reason">', 'invalid-javascript-requirement'],
    ['print', '<meta name="visual-qa:requires-javascript" content="This application renders live data.">', 'print-loss'],
  ]) {
    const page = join(artifactDir, `${name}.html`);
    await writeFile(page, original.replace('<head>', `<head>${metadata}`));
    const result = await runVisualQa({ ...options, urls: [page], artifactDir: join(artifactDir, name) });
    assert.equal(result.exitCode, 1);
    assert.ok(result.findings.some(({ type }) => type === expected), JSON.stringify(result.findings));
    if (name === 'print') {
      assert.equal(result.findings.some(({ type }) => type === 'javascript-disabled-loss'), false);
      assert.equal(result.pageDeclarations.length, 1);
    } else {
      assert.ok(result.findings.some(({ type }) => type === 'javascript-disabled-loss'));
      assert.deepEqual(result.pageDeclarations, []);
    }
  }
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

test('CSS Color 4 hover keeps the opaque review button above its gold tray (cas-3dac)', async () => {
  const artifactDir = await acceptanceDir('visual-qa-color4-hover-');
  const result = await runVisualQa({
    urls: [fixture('color4-hover.html')], artifactDir, strict: true,
    schemes: ['light'], viewports: both.viewports,
    journey: { name: 'review', states: [
      { name: 'hover', steps: [{ hover: '#review' }] },
      { name: 'focus', steps: [{ focus: '#review' }] },
    ] },
  });
  const review = result.findings.filter((finding) => finding.selector === '#review');
  assert.deepEqual(review, [], `opaque hover must not use the tray: ${JSON.stringify(review)}`);
  const controls = result.findings.filter((finding) => finding.type === 'contrast' && finding.selector === '#low-contrast');
  assert.equal(controls.length, 6, 'real low contrast survives in rest/hover/focus at both widths');
  for (const control of controls) {
    assert.deepEqual(control.background, [221, 218, 214]);
    assert.equal(control.ratio, 1);
    // The production inspector's background, with the captured button's ink.
    const luminance = (rgb) => rgb.reduce((sum, c, i) => {
      const s = c / 255;
      return sum + (s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4) * [0.2126, 0.7152, 0.0722][i];
    }, 0);
    const ratio = (luminance(control.background) + 0.05) / (luminance([27, 29, 36]) + 0.05);
    assert.ok(ratio >= 12, `hover must measure at least 12:1, not the old 2.57:1: ${ratio}`);
  }
});

test('browser CSS Color 4 spaces and alpha keep both visible and truly defective text checked (cas-3dac)', async () => {
  const artifactDir = await acceptanceDir('visual-qa-color4-spaces-');
  const path = join(artifactDir, 'colors.html');
  const dark = [
    'color(srgb 0.2 0.2 0.2)', 'color(srgb 20% 20% 20%)', 'color(srgb-linear 0.03 0.03 0.03)',
    'color(display-p3 0.2 0.2 0.2)', 'color(a98-rgb 0.2 0.2 0.2)',
    'color(prophoto-rgb 0.2 0.2 0.2)', 'color(rec2020 0.2 0.2 0.2)',
    'color(xyz 0.03 0.03 0.03)', 'color(xyz-d50 0.03 0.03 0.03)', 'color(xyz-d65 0.03 0.03 0.03)',
    'lab(20% 0 0)', 'lch(20% 0 120deg)', 'oklab(20% 0 0)', 'oklch(20% 0 0)',
    'hsl(120deg 0% 20%)', 'hwb(120deg 20% 80%)', 'color-mix(in srgb, black 80%, white)',
  ];
  await writeFile(path, `<!doctype html><html lang="en"><meta charset="utf-8">
    <style>body{margin:0;font:16px/1.4 Arial;background:white;color:black}p{margin:4px;padding:2px}</style>
    ${dark.map((color, i) => `<p id="space-${i}" style="color:${color}">Visible CSS color ${i}</p>`).join('')}
    <p id="transparent" style="color:color(srgb 0 0 0 / 0)">Invisible text</p>
    <p id="half" style="color:color(srgb 0 0 0 / 50%)">Genuinely low contrast alpha</p>
    <p id="pale" style="color:lab(90% 0 0)">Genuinely low contrast Lab</p></html>`);
  const result = await runVisualQa({ urls: [path], artifactDir, strict: true,
    schemes: ['light'], viewports: [{ name: 'desktop', width: 1280, height: 800 }] });
  assert.deepEqual(result.infoFindings.filter((finding) => finding.type === 'unverifiable-contrast'), []);
  assert.deepEqual(result.findings.filter((finding) => finding.type === 'invisible-text').map((finding) => finding.selector), ['#transparent']);
  assert.deepEqual(result.findings.filter((finding) => finding.type === 'contrast').map((finding) => finding.selector), ['#half', '#pale']);
  assert.ok(result.findings.find((finding) => finding.selector === '#half').ratio > 3.9);
});

test('opaque CSS Color 4 layers hide ancestor images, while own images and translucent layers remain unverifiable (cas-3dac)', async () => {
  const artifactDir = await acceptanceDir('visual-qa-color4-layers-');
  const path = join(artifactDir, 'layers.html');
  await writeFile(path, `<!doctype html><html lang="en"><meta charset="utf-8">
    <style>body{margin:0;font:16px/1.5 Arial;background:white;color:black}
    .gradient{padding:16px;background:linear-gradient(90deg,black,white)}
    p{padding:12px;margin:12px;background:color(srgb 1 1 1)}
    #exposed{background:transparent}#partial{background:color(srgb 1 1 1 / .999)}
    #own{background-image:linear-gradient(90deg,white,black)}</style>
    <div class="gradient"><p id="covered">Opaque layer over gradient</p>
    <p id="exposed">Exposed ancestor image</p><p id="partial">Translucent layer over image</p>
    <p id="own">Its own image still paints above its opaque color</p></div></html>`);
  const result = await runVisualQa({ urls: [path], artifactDir, strict: true,
    schemes: ['light'], viewports: [{ name: 'phone', width: 390, height: 800 }] });
  assert.deepEqual(result.findings, []);
  assert.deepEqual(result.infoFindings.filter((finding) => finding.type === 'unverifiable-contrast').map((finding) => finding.selector), ['#exposed', '#partial', '#own']);
});

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

test('an intentional multi-line clamp with an ellipsis is not clipped; an unclamped hidden overflow still is (cas-272d)', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-clamp-'));
  const result = await runVisualQa({
    urls: [fixture('line-clamp.html')],
    artifactDir,
    strict: true,
    schemes: ['light', 'dark'],
    viewports: [
      { name: 'desktop', width: 1280, height: 800 },
      { name: 'phone', width: 390, height: 844 },
    ],
  });
  const on = (id) => result.findings.filter((finding) => (finding.selector ?? '').includes(id) || (finding.elementPath ?? '').includes(id));
  // The clamp hides its later lines on purpose and shows an ellipsis: no finding.
  assert.deepEqual(on('#clamped'), [], JSON.stringify(result.findings, null, 2));
  // A -webkit-box clamp draws its own ellipsis with text-overflow left at
  // its default, as an audit summary with an expand toggle does (GH #1109).
  assert.deepEqual(on('#engine-ellipsis'), [], JSON.stringify(result.findings, null, 2));
  // Negative controls: hidden lines with no clamp and no ellipsis still fail,
  // and so does a line count on a plain block, which clamps nothing.
  assert.equal(result.status, 'FAIL');
  assert.ok(on('#unclamped').some((finding) => finding.type === 'clipped-content'), JSON.stringify(result.findings, null, 2));
  assert.ok(on('#stray').some((finding) => finding.type === 'clipped-content'), JSON.stringify(result.findings, null, 2));
});

test('cards in a horizontal scroll-snap carousel are reachable, not clipped; a row that cannot scroll still is (GH #1109)', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-carousel-'));
  const result = await runVisualQa({
    urls: [fixture('carousel.html')],
    artifactDir,
    strict: true,
    schemes: ['light', 'dark'],
    viewports: [
      { name: 'desktop', width: 1280, height: 800 },
      { name: 'phone', width: 390, height: 844 },
    ],
  });
  const on = (id) => result.findings.filter((finding) => [finding.selector, finding.elementPath, finding.ancestorPath].some((value) => (value ?? '').includes(id)));
  // Off-screen cards are reached by scrolling the rail: plain, inside a
  // section that hides its bleed, and inside a narrow shell with overflow
  // hidden, as the audit page's carousel sits.
  for (const id of ['#rail', '#bleed-rail', '#screens', 'ul.rail', 'div.scroller']) assert.deepEqual(on(id), [], `${id}: ${JSON.stringify(on(id), null, 2)}`);
  // Negative control: the same cards in a row that does not scroll are cut off.
  assert.equal(result.status, 'FAIL');
  assert.ok(on('stuck').some((finding) => finding.type === 'clipped-content' && finding.reason === 'text-bounds-exceed-overflow-ancestor'), JSON.stringify(result.findings, null, 2));
});

test('a long value scrolling inside an editable field is not clipped; a box that clips the field still is (cas-000c)', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-editable-'));
  const result = await runVisualQa({
    urls: [fixture('editable-input.html')],
    artifactDir,
    strict: true,
    schemes: ['light', 'dark'],
    viewports: [
      { name: 'desktop', width: 1280, height: 800 },
      { name: 'phone', width: 390, height: 844 },
    ],
  });
  // The long input value and the unwrapped textarea line scroll while editing: no finding on either field.
  assert.deepEqual(result.findings.filter((finding) => /#long-name|#long-notes|label:nth-of-type\((1|2)\) > (input|textarea)/.test(finding.elementPath ?? finding.selector ?? '')), [], JSON.stringify(result.findings, null, 2));
  // Negative control: the box that clips its input is still flagged.
  assert.equal(result.status, 'FAIL');
  assert.ok(result.findings.some((finding) => finding.type === 'clipped-content' && /tight-box/.test(finding.selector ?? finding.elementPath ?? '')), JSON.stringify(result.findings, null, 2));
});

test('text the engine skips (content-visibility: hidden) is not clipped; a visible clip still is (cas-861c)', async () => {
  const artifactDir = await mkdtemp(join(tmpdir(), 'visual-qa-content-visibility-'));
  const result = await runVisualQa({
    urls: [fixture('content-visibility.html')],
    artifactDir,
    strict: true,
    schemes: ['light', 'dark'],
    viewports: [
      { name: 'desktop', width: 1280, height: 800 },
      { name: 'phone', width: 390, height: 800 },
    ],
  });
  // Skipped text keeps a box past the scroller's range but is not drawn:
  // neither clipped nor invisible-text.
  assert.deepEqual(result.findings.filter((finding) => /Skipped line/.test(finding.textSample ?? '')), [], JSON.stringify(result.findings, null, 2));
  // The drawn text around it is still checked, and the real clip still fails.
  assert.equal(result.status, 'FAIL');
  const clipped = result.findings.filter((finding) => finding.type === 'clipped-content');
  assert.ok(clipped.some((finding) => /lost below the edge/.test(finding.textSample ?? '')), JSON.stringify(result.findings, null, 2));
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
