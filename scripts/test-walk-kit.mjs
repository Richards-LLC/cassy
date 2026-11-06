// Shared walk contracts: visible outcomes, measured replies and deduplicated evidence.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import {
  WALK_KIT_VERSION, createLedger, requireBuild, waitForReply,
  frameMessage, openDraft, shouldRun,
} from '../cas-cli/src/builtins/skills/cas-qa-craft/scripts/walk-kit.mjs';

function clock() {
  let time = 0;
  return { now: () => time, sleep: async ms => { time += ms; } };
}
const directory = t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'cas-walk-kit-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return root;
};

test('build checks require actual served identities for every requested surface', async () => {
  await assert.rejects(requireBuild({}, async () => ({})), /no fallback/);
  const expected = { backend: 'abcdef0', frontend: '1234567' };
  await assert.rejects(requireBuild(expected, async () => ({ backend: 'abcdef0', frontend: '7654321' })), /HOLD/);
  assert.deepEqual(await requireBuild(expected, async () => expected), expected);
});

test('reply waits ignore typing, blank and hidden bubbles and measure first visible text', async () => {
  const time = clock();
  const row = { id: 'reply', user: false, working: false, visible: true, text: 'Revised hook' };
  const result = await waitForReply({}, {
    ...time, before: new Set(['old']), sentAt: 0, pollMs: 1_000, settleMs: 2_000,
    readThread: async () => time.now() < 7_000
      ? [{ ...row, visible: false }, { ...row, text: '', working: true }]
      : [row],
  });
  assert.equal(result.firstReplyMs, 7_000);
  assert.deepEqual(result.replies, [row]);
  assert.equal(time.now(), 9_000);
  const timed = await waitForReply({}, {
    ...time, before: new Set(), sentAt: time.now(), timeoutMs: 1, pollMs: 1,
    readThread: async () => [{ ...row, working: true }],
  });
  assert.equal(timed.timedOut, true);
  assert.equal(timed.firstReplyMs, null);
});

test('frame capture refuses an attached offscreen message before taking a screenshot', async t => {
  const file = path.join(directory(t), 'frame.png');
  let y = 900, captures = 0;
  const page = { viewportSize: () => ({ width: 390, height: 844 }),
    screenshot: async ({ path: target }) => { captures++; fs.writeFileSync(target, 'visible frame bytes'); } };
  const adapters = { readVisibleText: async () => 'Revised hook', locateMessage: () => ({
    scrollIntoViewIfNeeded: async () => {}, boundingBox: async () => ({ x: 0, y, width: 300, height: 40 }),
  }) };
  await assert.rejects(frameMessage(page, 'Revised hook', file, adapters), /outside the viewport/);
  assert.equal(captures, 0);
  y = 100;
  const frame = await frameMessage(page, 'Revised hook', file, adapters);
  assert.equal(frame.visibleText, 'Revised hook');
  assert.equal(frame.hash, createHash('sha256').update(fs.readFileSync(file)).digest('hex'));
  assert.equal(captures, 1);
});

test('delivered artifacts wait for real visible content and compare the opened draft', async () => {
  const time = clock();
  let opened;
  const page = { goto: async url => { opened = url; }, url: () => opened };
  const draft = await openDraft(page, 'https://candidate.test/draft/42', ['revised hook', 'caption'], {
    ...time, pollMs: 1,
    readDraft: async () => ({ itemId: '42', visible: true, working: time.now() < 3,
      text: time.now() < 3 ? '' : 'The revised hook' }),
  });
  assert.equal(time.now(), 3);
  assert.equal(draft.ready, true);
  assert.equal(draft.itemId, '42');
  assert.deepEqual(draft.missing, ['caption']);
});

test('scripted answers run only for the question actually open', () => {
  for (const [ifAsked, open, run] of [
    ['target', null, false], ['target', { kind: 'details' }, false],
    ['target', { kind: 'target' }, true], ['any', { kind: 'details' }, true], [undefined, null, true],
  ]) assert.equal(shouldRun({ ifAsked }, open), run);
});

test('ledger writes versioned live rows, merges one defect and refuses unsupported verdicts', t => {
  const root = directory(t), file = path.join(root, 'LEDGER.md');
  const ledger = createLedger({ file, task: 'cas-fixture', build: 'abcdef0', scope: 'revise', surface: 'web', label: 'fixture' });
  for (const id of ['W1', 'W2', 'W3']) ledger.expect(id, {
    cell: 'revise draft', expected: 'revised hook', cite: 'docs/guide.md:17', quote: 'show the revised draft',
  });
  const frame = (name, text) => {
    const file = path.join(root, `${name}.png`);
    fs.writeFileSync(file, `${name}:${text}`);
    return { file, hash: createHash('sha256').update(fs.readFileSync(file)).digest('hex'), visibleText: text };
  };
  const step = { channel: 'web', view: '390', sent: { text: 'revise the hook', url: 'https://candidate.test/chat' },
    verdict: 'FAIL', defect: 'cas-fixture', observed: 'old hook', claims: ['old hook'] };
  ledger.record({ ...step, id: 'W1', frame: frame('a', 'old hook') });
  ledger.record({ ...step, id: 'W2', view: '1440', frame: frame('b', 'old hook') });
  assert.equal(ledger.rows.size, 1);
  const saved = fs.readFileSync(file, 'utf8');
  assert.match(saved, /schema: 2/);
  assert.match(saved, /cells=1; PASS=0; FAIL=1/);
  assert.ok(saved.includes(`[kit:${WALK_KIT_VERSION}; web/390, web/1440]`));
  assert.match(saved, /source: docs\/guide.md:17/);
  assert.throws(() => ledger.record({ ...step, id: 'W1', verdict: 'PASS', defect: undefined }), /conflicting verdicts/);
  assert.throws(() => ledger.record({ ...step, id: 'W3', frame: frame('c', 'different text') }), /claim does not appear/);
  const used = frame('d', 'old hook');
  ledger.record({ ...step, id: 'W3', frame: used });
  assert.throws(() => ledger.record({ ...step, id: 'W3', frame: used }), /already cited/);
  ledger.expect('PASS', { cell: 'saved', expected: 'visible draft', cite: 'docs/guide.md:17', quote: 'show the draft' });
  const pass = { ...step, id: 'PASS', verdict: 'PASS', defect: undefined };
  assert.throws(() => ledger.record(pass), /DB-only evidence/);
  assert.throws(() => ledger.record({ ...pass, frame: frame('e', 'old hook'), draft: { ready: true, missing: ['new hook'] } }), /delivered draft/);
  ledger.record({ ...pass, frame: frame('f', 'old hook'), reply: { firstReplyMs: 8_036, timedOut: false } });
  assert.match(fs.readFileSync(file, 'utf8'), /reply_ms=8036/);
  ledger.expect('TIMEOUT', { cell: 'reply', expected: 'reply arrives', cite: 'docs/guide.md:17', quote: 'show the reply' });
  const timeout = { ...step, id: 'TIMEOUT', defect: 'cas-timeout', frame: frame('timeout', 'old hook'),
    reply: { firstReplyMs: null, timedOut: true } };
  assert.throws(() => ledger.record({ ...timeout, verdict: 'PASS', defect: undefined }), /reply timed out/);
  assert.equal(ledger.record(timeout).verdict, 'FAIL');
  ledger.expect('UNCITED', { cell: 'invented offer', expected: 'make it mine' });
  const question = ledger.record({ ...step, id: 'UNCITED', frame: undefined });
  assert.match(question.verdict, /^NOT EXERCISED: spec question/);
  assert.ok(fs.readFileSync(file, 'utf8').split('\n').filter(line => /^\| (W1|PASS|TIMEOUT|UNCITED) \|/.test(line))
    .every(line => line.includes(`kit:${WALK_KIT_VERSION}`)));
});
