#!/usr/bin/env node
// Pebble assembly receipts (cas-590b). Two halves:
//  1. Fixture-site receipts: the built fixture harness, every Pebble state,
//     behaviour asserted in the DOM at 390 and 1280.
//  2. Protocol receipts: the production bundle (hub-web/dist) served at
//     /commander/ with the controlled protocol fixture — the quick-reply
//     payload on the wire, the pinned ask unpinning on the supervisor's
//     answer, the terminal alternate view, and the absence of the pane
//     article in the default conversation.
// Usage: node scripts/pebble-qa.mjs <artifact-dir>
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import { launchBrowser } from '../../docs/design/hub-mobile/browser-tools.mjs';
import { buildFixtureSite, closeServer, serveDirectory } from './visual-qa.mjs';
import { installProtocolFixture, serveDist, SUPERVISOR } from './conversations-qa.mjs';

const here = fileURLToPath(new URL('.', import.meta.url));
const repoRoot = resolve(here, '../..');
const artifacts = resolve(process.argv[2] || join(repoRoot, '.cas', 'pebble-qa'));
await mkdir(artifacts, { recursive: true });
const PHONE = { name: 'phone', width: 390, height: 844 };
const DESKTOP = { name: 'desktop', width: 1280, height: 800 };
const STATES = ['conversations-list', 'conversation', 'conversation-replied', 'conversation-error', 'conversation-thread', 'conversation-evidence', 'conversation-ask', 'conversation-ask-answered', 'conversation-blocker', 'conversation-pairs', 'conversation-attachment', 'conversation-empty', 'conversation-composer', 'conversation-keyboard', 'paired-machines'];

const receipts = [];
const receipt = (name, ok, detail) => { receipts.push({ name, status: ok ? 'PASS' : 'FAIL', detail }); if (!ok) throw new Error(`${name}: ${detail}`); };

const browser = await launchBrowser();
const fixtureBuild = join(repoRoot, '.cas', 'pebble-qa-fixture-build');
let fixtureServer, distServer;
try {
  // ---- 1. fixture-site receipts ---------------------------------------
  await rm(fixtureBuild, { recursive: true, force: true }); await mkdir(fixtureBuild, { recursive: true });
  await buildFixtureSite(fixtureBuild);
  fixtureServer = await serveDirectory(fixtureBuild);
  for (const viewport of [PHONE, DESKTOP]) for (const scheme of ['light', 'dark']) {
    const page = await browser.newPage({ viewport, colorScheme: scheme, reducedMotion: 'reduce' });
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    const open = async state => { await page.goto(`${fixtureServer.origin}/?fixture=${state}`); await page.locator('#app > *').first().waitFor(); };
    const shot = name => page.screenshot({ path: resolve(artifacts, `${viewport.name}-${scheme}-${name}.png`), fullPage: true });
    // no horizontal overflow on any state
    for (const state of STATES) {
      await open(state);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
      receipt(`no-horizontal-overflow ${state} ${viewport.name} ${scheme}`, !overflow, `scrollWidth ${await page.evaluate(() => document.documentElement.scrollWidth)} vs clientWidth ${viewport.width}`);
    }
    // list affordances: waiting vs unread
    await open('conversations-list');
    const rows = page.locator('#conversation-list .conversation-row');
    receipt(`list rows ${viewport.name} ${scheme}`, await rows.count() === 6, `${await rows.count()} rows`);
    const waiting = rows.filter({ has: page.locator('.conversation-flag') });
    const unread = rows.filter({ has: page.locator('.conversation-unread') });
    receipt(`list waiting-vs-unread ${viewport.name} ${scheme}`, await waiting.count() === 1 && await unread.count() === 1 && await waiting.locator('.conversation-when.hot').count() === 1 && await unread.locator('.conversation-unread').textContent() === '2' && await waiting.locator('.conversation-unread').count() === 0, 'exactly one waiting row (ochre dot + hot time, no pill) and one unread row (count pill)');
    const machines = await rows.locator('.conversation-machine').allTextContents();
    receipt(`list machine named as text ${viewport.name} ${scheme}`, machines.length === 6 && machines.every(Boolean) && new Set(machines).size === 3, machines.join(','));
    const footer = await page.locator('#paired-machines-toggle').textContent();
    receipt(`list footer count matches rows ${viewport.name} ${scheme}`, /6 conversations|3 paired machines/.test(footer ?? '') && !/3 conversations/.test(footer ?? ''), footer ?? '');
    await shot('list');
    // thread grouping and coalescing; pane article absent
    await open('conversation-thread');
    const groups = page.locator('.thread .msgs > .turn');
    const firstSup = page.locator('.thread .turn.sup').first();
    receipt(`thread grouped corners ${viewport.name} ${scheme}`, await firstSup.locator('.bub.group-first').count() === 1 && await firstSup.locator('.bub.group-last').count() === 1 && await firstSup.locator('.bub').count() === 2 && await firstSup.locator('time').count() === 1, 'first supervisor group: two bubbles, one group-first, one group-last, one timestamp');
    const coalesce = page.locator('.thread .coalesce');
    receipt(`thread coalesced statuses ${viewport.name} ${scheme}`, await coalesce.count() === 1 && await coalesce.getAttribute('data-count') === '4' && /3 more updates · gate 11 of 14 targets green/.test(await coalesce.textContent()), `${await coalesce.count()} lines; text ${await coalesce.first().textContent()}`);
    receipt(`thread operator pebbles right ${viewport.name} ${scheme}`, await page.locator('.thread .turn.you .bub').count() === 2, 'two operator turns');
    receipt(`no pane article in default view ${viewport.name} ${scheme}`, await page.locator('.conversation-pane, .conversation-pane-text').count() === 0 && !(await page.content()).includes('Live pane text'), 'no .conversation-pane and no "Live pane text"');
    receipt(`working indicator ${viewport.name} ${scheme}`, await page.locator('.thread .working').count() === 1, 'one working line');
    receipt(`receipt tick ${viewport.name} ${scheme}`, await page.locator('.thread .bub.receipt .tick').count() === 1, 'one ticked receipt');
    await shot('thread');
    // evidence table fits
    await open('conversation-evidence');
    const evi = page.locator('.thread .evi');
    receipt(`evidence table fits ${viewport.name} ${scheme}`, await evi.count() === 1 && await evi.evaluate(n => n.scrollWidth <= n.clientWidth && n.getBoundingClientRect().right <= document.documentElement.clientWidth) && await evi.locator('.evi-row').count() === 9, 'eight rows plus header inside the bubble, no scroller');
    // pinned ask + quick reply + unpin
    await open('conversation-ask');
    const pinned = page.locator('.pinned-ask');
    receipt(`ask pinned above composer ${viewport.name} ${scheme}`, await pinned.isVisible() && await pinned.locator('.chip').count() === 2 && await page.locator('#conversation-composer-slot > .pinned-ask + .message').count() === 1, 'pinned region visible with two chips directly above the composer');
    await shot('ask-pinned');
    await pinned.locator('.chip', { hasText: 'Fix in-train' }).click();
    await page.locator('.pinned-ask[hidden]').waitFor({ state: 'attached' });
    const answer = page.locator('.thread .turn.you .bub').last();
    receipt(`ask unpins on quick reply ${viewport.name} ${scheme}`, await pinned.isHidden() && (await answer.textContent() ?? '').includes('Fix in-train'), 'pinned region hidden, chip text recorded as an operator turn');
    await open('conversation-ask-answered');
    receipt(`answered ask stays unpinned ${viewport.name} ${scheme}`, await pinned.isHidden() && await page.locator('.thread [data-kind="ask"] .chip.sent').count() === 1, 'tray shows the sent reply, nothing pinned');
    await shot('ask-answered');
    // blocker evidence window
    await open('conversation-blocker');
    receipt(`blocker inset window ${viewport.name} ${scheme}`, await page.locator('.thread [data-kind="blocker"] .window').count() === 1 && await pinned.isHidden(), 'blocker renders its evidence window; nothing pinned');
    await shot('blocker');
    // attachment sheet opens the artifact path
    await open('conversation-attachment');
    const sheets = page.locator('.thread a.sheet');
    const hrefs = await sheets.evaluateAll(nodes => nodes.map(n => n.getAttribute('href')));
    receipt(`attachment link opens artifact path ${viewport.name} ${scheme}`, hrefs.length === 2 && hrefs.every(h => h?.startsWith('#artifact:')), hrefs.join(' '));
    await sheets.first().click();
    receipt(`attachment tap navigates ${viewport.name} ${scheme}`, (await page.evaluate(() => location.hash)) === hrefs[0], await page.evaluate(() => location.hash));
    await shot('attachment');
    await open('conversation-empty'); await shot('empty');
    await open('conversation-composer'); await shot('composer');
    // Phone keyboard (cas-edc9). Two paths: the browser honours
    // interactive-widget=resizes-content and the viewport itself shrinks to
    // 544 tall; or it ignores the meta and only the visual viewport shrinks,
    // which the conversation-keyboard fixture reproduces through the
    // visualViewport fallback. Either way the header, the pinned ask and the
    // last turn stay inside the viewport with the composer at its bottom.
    const keyboardReceipt = async (label, viewportHeight) => {
      const composer = page.locator('.conversation-composer');
      await composer.locator('textarea').focus();
      await page.waitForTimeout(120);
      const box = await page.evaluate(() => {
        const rect = selector => document.querySelector(selector)?.getBoundingClientRect();
        const header = rect('.conversation-heading'), composer = rect('.conversation-composer'), pinned = rect('.pinned-ask:not([hidden])');
        const turns = document.querySelectorAll('.thread .msgs > .turn');
        const last = turns[turns.length - 1]?.getBoundingClientRect();
        const shell = rect('.conversation-shell');
        const thread = document.querySelector('.conversation-reading.thread');
        const tailGap = thread ? thread.scrollHeight - thread.scrollTop - thread.clientHeight : NaN;
        return { scrollY: window.scrollY, tailGap, header: header && { top: header.top, bottom: header.bottom }, composer: composer && { top: composer.top, bottom: composer.bottom }, pinned: pinned && { top: pinned.top, bottom: pinned.bottom }, last: last && { top: last.top, bottom: last.bottom }, shell: shell && { height: shell.height } };
      });
      const inside = (r, limit) => r && r.top >= -0.5 && r.bottom <= limit + 0.5;
      receipt(`keyboard ${label}: header at the top ${viewport.name} ${scheme}`, box.scrollY === 0 && inside(box.header, viewportHeight) && box.header.top < 1, JSON.stringify({ scrollY: box.scrollY, header: box.header, viewportHeight }));
      receipt(`keyboard ${label}: composer above the keys ${viewport.name} ${scheme}`, inside(box.composer, viewportHeight) && box.composer.bottom > viewportHeight - 40, JSON.stringify({ composer: box.composer, viewportHeight }));
      // The last turn here is the ask itself, taller than the room the keyboard
      // leaves for the thread, so its tail — not its whole body — is what must
      // show: the thread is scrolled to its end and the turn ends above the
      // pinned copy, which is entirely in view.
      receipt(`keyboard ${label}: pinned ask and thread tail visible ${viewport.name} ${scheme}`, inside(box.pinned, viewportHeight) && box.tailGap < 2 && box.last && box.last.bottom > box.header.bottom && box.last.bottom <= box.pinned.top + 0.5, JSON.stringify({ pinned: box.pinned, last: box.last, tailGap: box.tailGap, viewportHeight }));
      receipt(`keyboard ${label}: shell fits the viewport ${viewport.name} ${scheme}`, Math.round(box.shell.height) === viewportHeight, JSON.stringify({ shell: box.shell, viewportHeight }));
      await shot(`keyboard-${label}`);
    };
    if (viewport.name === 'phone') {
      await open('conversation-ask');
      await page.setViewportSize({ width: viewport.width, height: 544 });
      await keyboardReceipt('resizes-content', 544);
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      await open('conversation-keyboard');
      await keyboardReceipt('visual-viewport-fallback', 544);
    }
    receipt(`no page errors ${viewport.name} ${scheme}`, errors.length === 0, errors.join('; '));
    await page.close();
  }

  // ---- 2. protocol receipts against the production bundle --------------
  distServer = await serveDist();
  for (const viewport of [PHONE, DESKTOP]) {
    const page = await browser.newPage({ viewport, colorScheme: 'light', reducedMotion: 'reduce' });
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    const fixture = await installProtocolFixture(page, distServer.origin);
    const list = page.getByRole('navigation', { name: 'Choose a supervisor' });
    await list.getByRole('button', { name: /cas-src/ }).click();
    await page.getByRole('button', { name: `Send to ${SUPERVISOR}`, exact: true }).waitFor();
    await page.locator('.conversation-reading.thread').waitFor();
    receipt(`bundle: viewport meta resizes content for the keyboard ${viewport.name}`, /interactive-widget=resizes-content/.test(await page.evaluate(() => document.querySelector('meta[name="viewport"]')?.getAttribute('content') ?? '')), await page.evaluate(() => document.querySelector('meta[name="viewport"]')?.getAttribute('content') ?? 'no viewport meta'));
    receipt(`bundle: default view has no pane article ${viewport.name}`, await page.locator('.conversation-pane, .conversation-pane-text').count() === 0 && !(await page.content()).includes('Live pane text') && !(await page.content()).includes('The supervisor conversation is ready for review.'), 'pane text from the Welcome scrollback is not mirrored into the thread');
    fixture.send(SUPERVISOR, { OperatorReply: { notification_id: 90, reply_to: null, message: 'Gate run 33512 failed on one warning. Fix it in-train or ship with it allowlisted?', summary: 'Ask', device_id: 'fixture-device', operator_label: 'Daniel', kind: 'ask', options: ['Fix in-train', 'Ship with allowlist'] } });
    const pinned = page.locator('.pinned-ask');
    await pinned.waitFor({ state: 'visible' });
    await page.screenshot({ path: resolve(artifacts, `bundle-${viewport.name}-ask-pinned.png`) });
    await pinned.locator('.chip', { hasText: 'Fix in-train' }).click();
    await page.locator('.conversation-turn[data-state="sending"]').waitFor();
    const sent = fixture.sends.at(-1);
    receipt(`bundle: quick-reply payload carries in_reply_to ${viewport.name}`, sent?.in_reply_to === 90 && sent.text === 'Fix in-train' && sent.target === SUPERVISOR, JSON.stringify({ in_reply_to: sent?.in_reply_to, text: sent?.text, target: sent?.target }));
    fixture.send(SUPERVISOR, { MessageQueued: { client_ref: sent.client_ref, notification_id: 91, target: SUPERVISOR, stamped: true } });
    await page.locator('.conversation-turn[data-state="acknowledged"]').waitFor();
    fixture.send(SUPERVISOR, { OperatorReply: { notification_id: 92, reply_to: 91, message: 'Spawning one worker on the warning now.', summary: '', device_id: 'fixture-device', operator_label: 'Daniel' } });
    await page.locator('.thread .bub p', { hasText: 'Spawning one worker on the warning now.' }).waitFor();
    receipt(`bundle: pinned ask unpins on answer ${viewport.name}`, await pinned.isHidden(), 'pinned region hidden after the reply to the quick reply');
    await page.screenshot({ path: resolve(artifacts, `bundle-${viewport.name}-ask-answered.png`) });
    await page.getByRole('button', { name: 'Terminal view', exact: true }).click();
    await page.locator('.t3-ghostty-canvas').first().waitFor();
    receipt(`bundle: terminal alternate view reachable ${viewport.name}`, await page.locator('.t3-ghostty-canvas').first().isVisible(), 'ghostty canvas visible after Terminal view');
    await page.screenshot({ path: resolve(artifacts, `bundle-${viewport.name}-terminal.png`) });
    await page.getByRole('button', { name: 'Conversations', exact: true }).click();
    await page.locator('.conversation-reading.thread').waitFor();
    receipt(`bundle: no page errors ${viewport.name}`, errors.length === 0, errors.join('; '));
    await page.close();
  }
} catch (error) {
  receipts.push({ name: 'run', status: 'FAIL', detail: error instanceof Error ? error.message : String(error) });
  process.exitCode = 1;
} finally {
  await browser.close();
  if (fixtureServer) await closeServer(fixtureServer.server);
  if (distServer) await distServer.close();
  await rm(fixtureBuild, { recursive: true, force: true });
  await writeFile(resolve(artifacts, 'pebble-qa.json'), JSON.stringify({ generatedAt: new Date().toISOString(), receipts }, null, 2));
  const failed = receipts.filter(r => r.status === 'FAIL');
  console.log(failed.length ? `FAIL ${failed.length}/${receipts.length} receipts` : `PASS ${receipts.length} receipts`);
  for (const r of failed) console.log(`FAIL ${r.name}: ${r.detail}`);
}
