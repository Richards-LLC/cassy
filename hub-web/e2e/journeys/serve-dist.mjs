#!/usr/bin/env node
// Serve the committed production bundle (hub-web/dist) at /commander/, the way
// `cas hub` embeds it. Used as the journeys project's webServer.
import { createServer } from 'node:http';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { extname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const dist = resolve(fileURLToPath(new URL('.', import.meta.url)), '../../dist');
const port = Number(process.argv[2] ?? process.env.HUB_JOURNEY_PORT ?? 4792);
const types = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml', '.wasm': 'application/wasm', '.woff2': 'font/woff2' };

createServer(async (request, response) => {
  const path = new URL(request.url ?? '/', 'http://127.0.0.1').pathname;
  if (!path.startsWith('/commander')) { response.writeHead(302, { location: '/commander/' }); response.end(); return; }
  const inner = path.replace(/^\/commander\/?/, '/');
  const target = resolve(dist, `.${inner === '/' ? '/index.html' : inner}`);
  if (!target.startsWith(dist) || !existsSync(target)) { response.writeHead(404); response.end(); return; }
  response.writeHead(200, { 'content-type': types[extname(target)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
  response.end(await readFile(target));
}).listen(port, '127.0.0.1', () => console.log(`serving ${dist} at http://127.0.0.1:${port}/commander/`));
