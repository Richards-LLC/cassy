#!/usr/bin/env node
// Production bundle + a controlled history response. This proves the phone
// reopen/hydration/dedupe flow; daemon/store scope is covered by Rust tests.
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { launchBrowser } from '../../docs/design/hub-mobile/browser-tools.mjs';
import { installProtocolFixture, serveDist, SUPERVISOR } from './conversations-qa.mjs';

const artifactDir = resolve(process.argv[2] || '/home/pippenz/.cas/artifacts/cas-56cc');
await mkdir(artifactDir, { recursive: true });
const served = await serveDist();
const browser = await launchBrowser();
const page = await browser.newPage({ viewport: { width: 390, height: 844 }, colorScheme: 'light', reducedMotion: 'reduce' });
const errors = [];
page.on('pageerror', (error) => errors.push(error.message));
const results = [];
const capture = async (target, name) => {
  await target.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await target.screenshot({ path: resolve(artifactDir, `${name}.png`) });
};
try {
  const fixture = await installProtocolFixture(page, served.origin, { history: true });
  const openConversation = async (target) => {
    await target.getByRole('navigation', { name: 'Choose a supervisor' }).getByRole('button', { name: /cas-src/ }).click();
    await target.getByRole('button', { name: `Send to ${SUPERVISOR}`, exact: true }).waitFor();
    const thread = target.locator('.conversation-reading.thread');
    await thread.getByText('Earlier question', { exact: true }).waitFor();
    await thread.getByText('Earlier answer', { exact: true }).waitFor();
    assert.equal(await thread.getByText('Earlier question', { exact: true }).count(), 1);
    assert.equal(await thread.getByText('Earlier answer', { exact: true }).count(), 1);
  };

  await openConversation(page);
  await capture(page, 'conversation-history-phone-reopen');
  results.push({ id: 'M01', status: 'PASS', label: 'fixture', evidence: 'conversation-history-phone-reopen.png' });
  fixture.send(SUPERVISOR, { OperatorReply: { notification_id: 18, reply_to: 17, message: 'Earlier answer', summary: '', device_id: 'fixture-device', operator_label: 'Daniel', kind: 'answer', attachments: [] } });
  await page.waitForTimeout(50);
  assert.equal(await page.locator('.conversation-reading.thread').getByText('Earlier answer', { exact: true }).count(), 1, 'live duplicate is deduplicated');
  await capture(page, 'conversation-history-live-dedupe');
  results.push({ id: 'M02', status: 'PASS', label: 'fixture', evidence: 'conversation-history-live-dedupe.png' });

  await page.getByRole('button', { name: '‹ Conversations', exact: true }).click();
  await openConversation(page);
  assert.equal(await page.locator('.conversation-reading.thread').getByText('Earlier question', { exact: true }).count(), 1, 'history survives reopen');
  assert.equal(await page.locator('.conversation-reading.thread').getByText('Earlier answer', { exact: true }).count(), 1, 'reply survives reopen');
  await capture(page, 'conversation-history-phone-revisit');
  results.push({ id: 'M03', status: 'PASS', label: 'fixture', evidence: 'conversation-history-phone-revisit.png' });

  const emptyPage = await browser.newPage({ viewport: { width: 390, height: 844 }, colorScheme: 'light', reducedMotion: 'reduce' });
  await installProtocolFixture(emptyPage, served.origin);
  await emptyPage.getByRole('navigation', { name: 'Choose a supervisor' }).getByRole('button', { name: /cas-src/ }).click();
  await emptyPage.getByText('Nothing waiting on you.', { exact: false }).waitFor();
  await capture(emptyPage, 'conversation-history-empty-phone');
  results.push({ id: 'M04', status: 'PASS', label: 'fixture', evidence: 'conversation-history-empty-phone.png' });
  await emptyPage.close();

  const desktopPage = await browser.newPage({ viewport: { width: 1280, height: 800 }, colorScheme: 'dark', reducedMotion: 'reduce' });
  await installProtocolFixture(desktopPage, served.origin, { history: true });
  await openConversation(desktopPage);
  await capture(desktopPage, 'conversation-history-desktop-reopen');
  results.push({ id: 'M05', status: 'PASS', label: 'fixture', evidence: 'conversation-history-desktop-reopen.png' });
  await desktopPage.close();

  assert.deepEqual(errors, []);
  await writeFile(resolve(artifactDir, 'conversation-history-qa-results.json'), JSON.stringify(results, null, 2));
  console.log('PASS conversation history phone reopen/hydration/dedupe');
} finally {
  await page.close();
  await browser.close();
  await served.close();
}
