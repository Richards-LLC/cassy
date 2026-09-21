#!/usr/bin/env node
// Production bundle + a controlled history response. This proves the phone
// reopen/hydration/dedupe flow; daemon/store scope is covered by Rust tests.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
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
try {
  const fixture = await installProtocolFixture(page, served.origin, { history: true });
  const openConversation = async () => {
    await page.getByRole('navigation', { name: 'Choose a supervisor' }).getByRole('button', { name: /cas-src/ }).click();
    await page.getByRole('button', { name: `Send to ${SUPERVISOR}`, exact: true }).waitFor();
    await page.getByText('Earlier question', { exact: true }).waitFor();
    await page.getByText('Earlier answer', { exact: true }).waitFor();
    assert.equal(await page.getByText('Earlier question', { exact: true }).count(), 1);
    assert.equal(await page.getByText('Earlier answer', { exact: true }).count(), 1);
  };

  await openConversation();
  await page.screenshot({ path: resolve(artifactDir, 'conversation-history-phone-reopen.png') });
  fixture.send(SUPERVISOR, { OperatorReply: { notification_id: 18, reply_to: 17, message: 'Earlier answer', summary: '', device_id: 'fixture-device', operator_label: 'Daniel', kind: 'answer', attachments: [] } });
  await page.waitForTimeout(50);
  assert.equal(await page.getByText('Earlier answer', { exact: true }).count(), 1, 'live duplicate is deduplicated');

  await page.getByRole('button', { name: '‹ Conversations', exact: true }).click();
  await openConversation();
  assert.equal(await page.getByText('Earlier question', { exact: true }).count(), 1, 'history survives reopen');
  assert.equal(await page.getByText('Earlier answer', { exact: true }).count(), 1, 'reply survives reopen');
  assert.deepEqual(errors, []);
  console.log('PASS conversation history phone reopen/hydration/dedupe');
} finally {
  await page.close();
  await browser.close();
  await served.close();
}
