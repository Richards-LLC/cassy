// Receipt ownership: two journey tests never share a receipt directory, so
// neither can overwrite the other's stages, receipt.webm or result.json.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { claimReceiptDirectory, receiptPartName } from './receipt-directory.mjs';

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

test('same-titled light and dark variants cannot reclaim each other as retries (cas-256a)', () => {
  const { root, done } = fresh();
  try {
    const title = 'HUB-J17 phone feedback';
    const directory = join(root, 'HUB-J17', 'parts', 'phone-feedback');
    const identity = (theme) => ({ project: 'journeys', titlePath: ['fleet-ops.journey.ts', `phone fleet feedback ${theme}`, title] });
    claimReceiptDirectory(directory, title, identity('light'));
    writeFileSync(join(directory, 'J01.png'), 'light capture');
    assert.throws(() => claimReceiptDirectory(directory, title, identity('dark')), /already claimed/);
    assert.equal(readFileSync(join(directory, 'J01.png'), 'utf8'), 'light capture');
  } finally { done(); }
});

test('variant captures survive independently and a retry clears only its own stale files (cas-256a)', () => {
  const { root, done } = fresh();
  try {
    const title = 'HUB-J17 phone feedback';
    const variants = ['light', 'dark'].map((theme) => {
      const identity = { project: 'journeys', titlePath: ['fleet-ops.journey.ts', `phone fleet feedback ${theme}`, title] };
      const directory = join(root, 'HUB-J17', 'parts', receiptPartName(title, identity));
      claimReceiptDirectory(directory, title, identity);
      for (const name of ['J01.png', 'receipt.webm', 'result.json']) writeFileSync(join(directory, name), theme);
      return { identity, directory };
    });
    assert.notEqual(variants[0].directory, variants[1].directory);
    claimReceiptDirectory(variants[0].directory, title, { ...variants[0].identity });
    assert.equal(existsSync(join(variants[0].directory, 'J01.png')), false);
    for (const name of ['J01.png', 'receipt.webm', 'result.json']) assert.equal(readFileSync(join(variants[1].directory, name), 'utf8'), 'dark');
    assert.deepEqual(JSON.parse(readFileSync(join(variants[1].directory, 'receipt-owner.json'), 'utf8')), { title, ...variants[1].identity });
  } finally { done(); }
});

test('project names and the untruncated title path distinguish receipt parts (cas-256a)', () => {
  const title = 'HUB-J17 ' + 'long readable prefix '.repeat(10);
  const identity = { project: 'phone-light', titlePath: ['fleet-ops.journey.ts', 'light', title + 'first'] };
  const part = receiptPartName(title, identity);
  assert.equal(receiptPartName(title, { ...identity, titlePath: [...identity.titlePath] }), part);
  assert.notEqual(receiptPartName(title, { ...identity, project: 'phone-dark' }), part);
  assert.notEqual(receiptPartName(title, { ...identity, titlePath: ['fleet-ops.journey.ts', 'light', title + 'second'] }), part);
  assert.ok(part.length <= 96);
});
