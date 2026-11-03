import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'node:net';
import { createRealHubHome } from './real-hub-home.mjs';

test('macOS real-hub HOME keeps the native daemon socket within its 103-byte limit', async t => {
  const root = await mkdtemp(join(tmpdir(), 'cas-rh-path-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const longTmp = join(root, 'macos-default-temporary-directory-component'.repeat(2));
  await mkdir(longTmp);
  const home = await createRealHubHome({ platform: 'darwin', temporaryDirectory: longTmp });
  t.after(() => rm(home, { recursive: true, force: true }));
  // validate_daemon_start checks the longer GUI path before the TUI socket.
  const socket = join(home, '.cas', 'factory-real-hub-journey.gui.sock');
  assert.ok(Buffer.byteLength(socket) <= 103, `native socket is ${Buffer.byteLength(socket)} bytes: ${socket}`);
  await mkdir(join(home, '.cas'));
  const server = createServer();
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(socket, resolve);
  });
  await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
});
