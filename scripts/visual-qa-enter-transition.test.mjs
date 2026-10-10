// GH #1158: --strict must not report invisible-text for an element that is
// mid-way through a post-load enter transition.
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { runVisualQa } from './visual-qa.mjs';

const fixture = fileURLToPath(new URL('./visual-qa-fixtures/enter-transition.html', import.meta.url));
const matrix = { strict: true, schemes: ['light'], viewports: [{ name: 'phone', width: 390, height: 800 }] };
const artifacts = () => mkdtemp(join(process.env.VISUAL_QA_ARTIFACTS_ROOT || tmpdir(), 'enter-transition-'));

// Where the race lands depends on how long load-to-measure takes on the host:
// 25-45 ms after load on a fast local fixture, 212-407 ms on the staging app
// GH #1158 measured. Ten runs sweep 0-90 ms and ten sweep the measured band.
const DELAYS = [
  ...Array.from({ length: 10 }, (_, index) => index * 10),
  ...Array.from({ length: 10 }, (_, index) => 212 + Math.round((index * 195) / 9)),
];

test('a post-load enter transition passes --strict on 20 of 20 runs', async () => {
  assert.equal(DELAYS.length, 20);
  const failures = [];
  for (const delay of DELAYS) {
    const url = `${pathToFileURL(fixture).href}?delay=${delay}`;
    const result = await runVisualQa({ ...matrix, urls: [url], artifactDir: await artifacts() });
    if (result.status !== 'PASS') {
      failures.push(`delay ${delay} ms: ${result.findings.map(({ type, reason }) => `${type} (${reason})`).join(', ')}`);
    }
  }
  assert.deepEqual(failures, [], `${failures.length}/20 runs measured the message mid-transition`);
});

test('text that stays invisible after its transition still fails --strict', async () => {
  const artifactDir = await artifacts();
  const path = join(artifactDir, 'page.html');
  await writeFile(path, `<!doctype html><html lang="en"><meta charset="utf-8">
    <meta name="visual-qa:requires-javascript" content="Fixture inserts content after load.">
    <style>body { font: 16px sans-serif; color: #172033; background: #fff; }
      .stuck { opacity: 0; transition: opacity 200ms ease; }</style>
    <main><h1>Sign in</h1><div id="field"></div></main>
    <script>addEventListener('load', () => setTimeout(() => {
      const message = document.createElement('div');
      message.className = 'stuck';
      message.id = 'stuck';
      message.textContent = 'This message never becomes visible.';
      document.querySelector('#field').append(message);
    }, 250));</script></html>`);
  const result = await runVisualQa({ ...matrix, urls: [path], artifactDir });
  assert.equal(result.status, 'FAIL');
  assert.ok(result.findings.some(({ type, selector }) => type === 'invisible-text' && /stuck/.test(selector)), JSON.stringify(result.findings));
});
