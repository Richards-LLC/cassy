#!/usr/bin/env node
// Serve a verified production snapshot built from this source tip. The journey
// producer supplies its manifest; standalone named journeys build one here.
import { spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { extname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = resolve(fileURLToPath(new URL('.', import.meta.url)), '../../..');
const manifest = process.env.JOURNEY_BUILD_MANIFEST;
const built = spawnSync('python3', [resolve(repo, 'scripts/journey_dist.py'), manifest ? 'verify' : 'build',
  '--repo', repo, ...(manifest ? ['--manifest', manifest] : [])], { encoding: 'utf8' });
if (built.error || built.status !== 0) {
  console.error(`serve-dist: ${built.error?.message ?? built.stderr}`);
  process.exit(2);
}
const snapshot = JSON.parse(built.stdout);
const dist = snapshot.dist_path;
const port = Number(process.argv[2] ?? process.env.HUB_JOURNEY_PORT);
if (!Number.isInteger(port) || port <= 0) {
  console.error('serve-dist: pass the port (playwright.config.ts picks this checkout\'s own; see e2e/checkout-ports.mjs)');
  process.exit(2);
}
const types = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml', '.wasm': 'application/wasm', '.woff2': 'font/woff2' };

createServer(async (request, response) => {
  const path = new URL(request.url ?? '/', 'http://127.0.0.1').pathname;
  if (!path.startsWith('/commander')) { response.writeHead(302, { location: '/commander/' }); response.end(); return; }
  const inner = path.replace(/^\/commander\/?/, '/');
  const target = resolve(dist, `.${inner === '/' ? '/index.html' : inner}`);
  if (!target.startsWith(dist + sep) || !existsSync(target)) { response.writeHead(404); response.end(); return; }
  response.writeHead(200, { 'content-type': types[extname(target)] ?? 'application/octet-stream', 'cache-control': 'no-store',
    'x-cassy-source-sha': snapshot.source_sha, 'x-cassy-dist-sha256': snapshot.dist_sha256 });
  response.end(await readFile(target));
}).listen(port, '127.0.0.1', () => console.log(`serving ${dist} at http://127.0.0.1:${port}/commander/`));
