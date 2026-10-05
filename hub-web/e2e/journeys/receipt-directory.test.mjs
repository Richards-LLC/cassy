// Receipt ownership: two journey tests never share a receipt directory, so
// neither can overwrite the other's stages, receipt.webm or result.json.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { claimReceiptDirectory } from './receipt-directory.mjs';

const fresh = () => {
  const root = mkdtempSync(join(process.cwd(), '.receipt-claims-'));
  return { root, done: () => rmSync(root, { recursive: true, force: true }) };
};

test('the first test claims its directory and records its title', () => {
  const { root, done } = fresh();
  try {
    const directory = join(root, 'HUB-J13');
    claimReceiptDirectory(directory, 'HUB-J13 start a new session from Commander');
    assert.deepEqual(JSON.parse(readFileSync(join(directory, 'receipt-owner.json'), 'utf8')), { title: 'HUB-J13 start a new session from Commander' });
  } finally { done(); }
});

test('a second test without journeyPart is refused before it records anything', () => {
  const { root, done } = fresh();
  try {
    const directory = join(root, 'HUB-J13');
    claimReceiptDirectory(directory, 'HUB-J13 start a new session from Commander');
    assert.throws(() => claimReceiptDirectory(directory, 'HUB-J13 New session says a reconnecting machine is reconnecting'), /already claimed: .*HUB-J13; give extra tests journeyPart/);
    // The first owner is kept.
    assert.match(readFileSync(join(directory, 'receipt-owner.json'), 'utf8'), /start a new session/);
  } finally { done(); }
});

test('a journeyPart test claims its own parts directory beside the main test', () => {
  const { root, done } = fresh();
  try {
    claimReceiptDirectory(join(root, 'HUB-J13'), 'HUB-J13 start a new session from Commander');
    claimReceiptDirectory(join(root, 'HUB-J13', 'parts', 'reconnecting'), 'HUB-J13 New session says a reconnecting machine is reconnecting');
    assert.match(readFileSync(join(root, 'HUB-J13', 'parts', 'reconnecting', 'receipt-owner.json'), 'utf8'), /reconnecting/);
  } finally { done(); }
});

test('the same test claims its directory again (a retry or --repeat-each) and its stale captures are cleared', () => {
  const { root, done } = fresh();
  try {
    const directory = join(root, 'HUB-J13');
    const title = 'HUB-J13 start a new session from Commander';
    claimReceiptDirectory(directory, title);
    for (const name of ['J01.png', 'J02.png', 'result.json']) writeFileSync(join(directory, name), 'earlier attempt');
    mkdirSync(join(directory, 'parts', 'reconnecting'), { recursive: true });
    writeFileSync(join(directory, 'parts', 'reconnecting', 'J01.png'), 'another test');
    claimReceiptDirectory(directory, title);
    assert.equal(existsSync(join(directory, 'J02.png')), false);
    assert.equal(existsSync(join(directory, 'result.json')), false);
    assert.equal(readFileSync(join(directory, 'parts', 'reconnecting', 'J01.png'), 'utf8'), 'another test');
    assert.match(readFileSync(join(directory, 'receipt-owner.json'), 'utf8'), /start a new session/);
  } finally { done(); }
});
