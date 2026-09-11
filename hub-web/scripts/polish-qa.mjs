#!/usr/bin/env node
// Controlled catalog / transport against the production bundle. No live credentials.
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
import { launchBrowser } from '../../docs/design/hub-mobile/browser-tools.mjs';
import { installProtocolFixture, SUPERVISOR } from './conversations-qa.mjs';

const origin = process.argv[2] || 'http://127.0.0.1:8436/commander/';
const artifacts = resolve(process.argv[3]);
await mkdir(artifacts, { recursive: true });
const browser = await launchBrowser();
const results = [];
try {
  for (const scheme of ['light', 'dark']) for (const viewport of [{ name: 'phone', width: 390, height: 844 }, { name: 'desktop', width: 1280, height: 800 }, { name: 'landscape', width: 844, height: 390 }]) {
    const page = await browser.newPage({ viewport, colorScheme: scheme, reducedMotion: 'reduce' });
    const errors = []; page.on('pageerror', error => errors.push(error.message));
    const fixture = await installProtocolFixture(page, origin);
    const capture = name => page.screenshot({ path: resolve(artifacts, `${viewport.name}-${scheme}-${name}.png`) });
    const list = page.getByRole('navigation', { name: 'Choose a supervisor' });
    await page.getByRole('button', { name: /2 paired machines Connected/ }).waitFor();
    assert.equal(await list.getByRole('button').count(), 2);
    await capture('list');
    const paired = page.getByRole('button', { name: /2 paired machines/ });
    await paired.focus(); await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: 'Paired machines', exact: true });
    await dialog.waitFor();
    assert.equal(await dialog.locator('.paired-machine').count(), 2);
    assert.match(await dialog.innerText(), /atlas.test/);
    assert.match(await dialog.innerText(), /Last seen/);
    await capture('machines');
    await page.keyboard.press('Escape');
    assert.equal(await paired.evaluate(node => node === document.activeElement), true);
    await list.getByRole('button', { name: /cas-src/ }).click();
    await page.getByRole('textbox', { name: 'Your message' }).fill('Keep this instruction visible while unreachable.');
    await page.getByRole('button', { name: `Send to ${SUPERVISOR}`, exact: true }).click();
    await page.locator('.conversation-turn[data-state="sending"]').waitFor();
    const sent = fixture.sends.at(-1); assert(sent);
    fixture.setSessions('atlas', []);
    await page.getByRole('button', { name: '‹ Conversations', exact: true }).click();
    await list.getByRole('button', { name: /Unreachable/ }).waitFor();
    assert.equal(await list.getByRole('button').count(), 2);
    await capture('unreachable');
    fixture.send(SUPERVISOR, { MessageQueued: { client_ref: sent.client_ref, notification_id: 51, target: SUPERVISOR, stamped: true } });
    assert.equal(await list.getByRole('button', { name: /Unreachable/ }).count(), 1);
    fixture.send(SUPERVISOR, { OperatorReply: { notification_id: 52, reply_to: 51, message: 'Instruction completed.', summary: '', device_id: 'fixture-device' } });
    await list.getByRole('button', { name: /cas-src/ }).waitFor({ state: 'detached' });
    fixture.setSessions('atlas', fixture.liveSessions('atlas'));
    await list.getByRole('button', { name: /cas-src/ }).waitFor();
    await capture('recovered');
    await paired.click();
    await dialog.getByRole('button', { name: 'Remove from this browser', exact: true }).first().click();
    await dialog.getByRole('button', { name: 'Confirm removal', exact: true }).click();
    await dialog.locator('.paired-machine-address').filter({ hasText: 'atlas.test' }).waitFor({ state: 'detached' });
    assert.equal(await dialog.locator('.paired-machine').count(), 1);
    await capture('removed');
    await page.getByRole('button', { name: 'Close paired machines', exact: true }).click();
    await page.reload();
    await page.getByRole('button', { name: /Studio Mac.*Connected/ }).waitFor();
    assert.equal(await list.getByRole('button').count(), 1);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    assert.deepEqual(errors, []);
    results.push({ scheme, viewport, verdict: 'PASS', label: 'fixture', checks: ['dead and unstaffed hidden', 'canonical header mark', 'footer', 'machine register', 'keyboard focus return', 'in-flight absent session retained', 'reply resolves retention', 'catalog recovery', 'durable machine removal'] });
    await page.close();
  }
} finally { await browser.close(); await writeFile(resolve(artifacts, 'results.json'), JSON.stringify(results, null, 2)); }
console.log(`PASS ${results.length} production-bundle fixture journeys`);
