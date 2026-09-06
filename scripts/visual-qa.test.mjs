import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { runVisualQa } from './visual-qa.mjs';

const here = fileURLToPath(new URL('.', import.meta.url));
const fixture = (name) => join(here, 'visual-qa-fixtures', name);

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
  assert.deepEqual(json.schemes, ['light', 'dark']);
  assert.deepEqual(json.viewports.map(({ width }) => width), [1280, 390]);
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
