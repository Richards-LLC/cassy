#!/usr/bin/env node
// Production bundle + controlled protocol data. This is fixture evidence, not
// proof of operator identity stamping or durable delivery by a running daemon.
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { launchBrowser } from '../../docs/design/hub-mobile/browser-tools.mjs';

export const PANE_TEXT = 'The supervisor conversation is ready for review.\r\n\r\nI kept the project visible while you read and reply.\r\n\r\n    const project = "cas-src";\r\n    const instruction = "Keep the real supervisor text.";\r\n\r\nThe focused checks passed. I am waiting for your direction.';
export const SUPERVISOR = 'patient-pelican-9';
export async function installProtocolFixture(page, origin) {
  const sockets = new Map();
  const sends = [];
  const liveSessions = id => {
    const second = id === 'studio'; const supervisor = second ? 'calm-otter-4' : SUPERVISOR;
    return [{ name: supervisor, supervisor, project_dir: second ? '/projects/gabber-studio' : '/projects/cas-src', workers: ['fixture-worker'], liveness: 'live' }];
  };
  const catalogs = new Map(['atlas', 'studio'].map(id => [id, [...liveSessions(id),
    { ...liveSessions(id)[0], name: 'dead-supervisor', supervisor: 'dead-supervisor', dormant: true },
    { ...liveSessions(id)[0], name: 'empty-supervisor', supervisor: 'empty-supervisor', workers: [] },
  ]]));
  await page.addInitScript(() => {
    const original = window.fetch;
    window.fetch = (input, init) => {
      const url = new URL(String(input), location.href);
      if (url.hostname.endsWith('.test') && url.pathname === '/v1/events') return Promise.resolve(new Response(new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode(': fixture connected\n\n')); } }), { headers: { 'content-type': 'text/event-stream' } }));
      return original(input, init);
    };
  });
  await page.route('https://*.test/**', route => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const second = url.hostname === 'studio.test';
    const session = second ? 'calm-otter-4' : SUPERVISOR;
    let body = {};
    if (path === '/v1/machine') body = { schema_version: 1, version: 'fixture', capabilities: ['session_index', 'daemon_attach', 'machine_events'] };
    if (path === '/v1/sessions') body = { freshness_threshold_secs: 30, sessions: catalogs.get(second ? 'studio' : 'atlas') };
    if (path.endsWith('/lease')) body = { held_by_me: true, controller_label: 'Fixture device' };
    if (path.endsWith('/status')) body = { tasks_in_progress: [{ id: 'task-fixture', title: 'Supervisor conversations', status: 'in_progress' }], tasks_ready: [], agents: [] };
    if (path.endsWith('/websocket-ticket')) body = { ticket: 'fixture-only' };
    return route.fulfill({ json: body });
  });
  await page.routeWebSocket(/\.test\/v1\//, ws => {
    const session = decodeURIComponent(new URL(ws.url()).pathname.split('/')[3]);
    sockets.set(session, ws);
    ws.onMessage(data => { const message = JSON.parse(String(data)); if (message.SendMessage) sends.push({ session, ...message.SendMessage }); });
    ws.send(JSON.stringify({ Welcome: { state: { panes: [{ id: 'supervisor', kind: 'Supervisor', title: session, focused: true, exited: false }], cols: 100, rows: 28 }, scrollback: { supervisor: [[...Buffer.from(PANE_TEXT)]] } } }));
  });
  await page.goto(origin);
  await page.evaluate(async () => {
    localStorage.clear();
    const pair = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
    const publicKey = await crypto.subtle.exportKey('jwk', pair.publicKey);
    const db = await new Promise((resolve, reject) => { const req = indexedDB.open('cas-commander-v1', 1); req.onsuccess = () => resolve(req.result); req.onerror = () => reject(req.error); });
    await new Promise((resolve, reject) => {
      const tx = db.transaction('machines', 'readwrite');
      for (const [id, label] of [['atlas', 'Atlas · Linux'], ['studio', 'Studio Mac · macOS']]) tx.objectStore('machines').put({ id, label, baseUrl: `https://${id}.test`, deviceId: 'fixture-device', credentialId: 'fixture-credential', credential: 'fixture-only', expiresAt: '2099-01-01T00:00:00Z', scopes: ['machine-read','session-read','pane-read','pane-input','message-send','pane-interrupt'], privateKey: pair.privateKey, publicKey });
      tx.oncomplete = resolve; tx.onerror = () => reject(tx.error);
    });
    db.close();
  });
  await page.reload();
  await page.getByRole('navigation', { name: 'Choose a supervisor' }).getByRole('button').first().waitFor();
  return { sends, sockets, liveSessions, setSessions(id, rows) { catalogs.set(id, rows); }, send(session, message) { assert(sockets.has(session), `No fixture socket for ${session}`); sockets.get(session).send(JSON.stringify(message)); } };
}

export async function runConversationQa(origin, artifactDir) {
  await mkdir(artifactDir, { recursive: true });
  const browser = await launchBrowser();
  const results = [];
  try {
    for (const scheme of ['light', 'dark']) for (const viewport of [{ name: 'phone', width: 390, height: 844 }, { name: 'desktop', width: 1280, height: 800 }, { name: 'landscape', width: 844, height: 390 }]) {
      const page = await browser.newPage({ viewport, colorScheme: scheme, reducedMotion: 'reduce' });
      const errors = []; page.on('pageerror', e => errors.push(e.message));
      const fixture = await installProtocolFixture(page, origin);
      const capture = async name => { await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))); return page.screenshot({ path: resolve(artifactDir, `${viewport.name}-${scheme}-${name}.png`) }); };
      await capture('list');
      const list = page.getByRole('navigation', { name: 'Choose a supervisor' });
      assert.equal(await list.locator('.project-badge').allTextContents().then(x => x.sort()).then(x => x.join(',')), 'cas-src,gabber-studio');
      await list.getByRole('button', { name: /cas-src/ }).click();
      await page.getByRole('button', { name: `Send to ${SUPERVISOR}`, exact: true }).waitFor();
      await page.getByText('The supervisor conversation is ready for review.', { exact: true }).waitFor();
      await capture('thread');
      const composer = page.getByRole('textbox', { name: 'Your message' });
      await composer.fill('Please keep the project badge prominent.');
      await page.getByRole('button', { name: `Send to ${SUPERVISOR}`, exact: true }).click();
      await page.locator('.conversation-turn[data-state="sending"]').waitFor();
      const sent = fixture.sends.at(-1); assert(sent); assert.equal(sent.target, SUPERVISOR); assert.equal(sent.attribution.operator_label, null); assert(sent.client_ref);
      await capture('sending');
      fixture.send(SUPERVISOR, { MessageQueued: { client_ref: sent.client_ref, notification_id: 41, target: SUPERVISOR, stamped: true } });
      await page.getByText('Acknowledged · sent from you', { exact: true }).waitFor();
      await capture('acknowledged');
      fixture.send(SUPERVISOR, { OperatorReply: { notification_id: 42, reply_to: 41, message: 'The project badge stays visible in the list and conversation header.', summary: 'Badge confirmed', device_id: 'fixture-device', operator_label: 'Daniel' } });
      await page.getByText('The project badge stays visible in the list and conversation header.', { exact: true }).waitFor();
      await page.locator('.conversation-turn[data-state="replied"]').waitFor();
      await capture('replied');
      await composer.fill('Keep my selection across a heartbeat');
      await composer.evaluate(node => { node.focus(); node.setSelectionRange(5, 9); });
      fixture.send(SUPERVISOR, { StateUpdate: { state: { panes: [{ id: 'supervisor', kind: 'Supervisor', title: SUPERVISOR, focused: true, exited: false }], cols: 100, rows: 28 } } });
      await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      assert.deepEqual(await composer.evaluate(node => [node === document.activeElement, node.selectionStart, node.selectionEnd]), [true, 5, 9]);
      await composer.fill('Test a refused instruction');
      await page.getByRole('button', { name: `Send to ${SUPERVISOR}`, exact: true }).click();
      const refused = fixture.sends.at(-1);
      fixture.send(SUPERVISOR, { Error: { client_ref: refused.client_ref, message: 'Permission refused' } });
      await page.locator('.conversation-turn[data-state="error"]').waitFor();
      assert.equal(await page.locator('#message-delivery').isVisible(), false);
      await capture('rejected');
      await page.getByRole('button', { name: 'Edit message', exact: true }).click();
      assert.equal(await composer.inputValue(), 'Test a refused instruction');
      await composer.fill('Draft for this project');
      await page.getByRole('button', { name: '‹ Conversations', exact: true }).click();
      await list.getByRole('button', { name: /gabber-studio/ }).click();
      assert.equal(await page.getByRole('textbox', { name: 'Your message' }).inputValue(), '');
      await page.getByRole('button', { name: '‹ Conversations', exact: true }).click();
      await list.getByRole('button', { name: /cas-src/ }).click();
      assert.equal(await page.getByRole('textbox', { name: 'Your message' }).inputValue(), 'Draft for this project');
      await page.getByRole('button', { name: 'Terminal view', exact: true }).click();
      await page.locator('.t3-ghostty-canvas').first().waitFor();
      await capture('terminal');
      await page.getByRole('button', { name: 'Conversations', exact: true }).click();
      await page.getByRole('button', { name: `Send to ${SUPERVISOR}`, exact: true }).waitFor();
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'page overflow');
      assert.deepEqual(errors, []);
      results.push({ viewport, scheme, status: 'PASS', evidence: 'fixture protocol against production bundle', checks: ['project badges', 'exact pane text', 'addressed send', 'client correlation', 'acknowledged', 'replied', 'draft isolation', 'terminal alternate'], errors });
      await page.close();
    }
  } finally { await browser.close(); await writeFile(resolve(artifactDir, 'results.json'), JSON.stringify(results, null, 2)); }
  return results;
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const result = await runConversationQa(process.argv[2] || 'http://127.0.0.1:8421/commander/', resolve(process.argv[3]));
  console.log(`PASS ${result.length} viewport/scheme combinations`);
}
