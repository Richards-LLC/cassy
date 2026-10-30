import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";

test("real-hub candidate assets cannot fall back to external dist", { timeout: 15000 }, async t => {
  // The external bundle really has this stylesheet; this candidate lacks it.
  assert.ok((await readFile(new URL("../../dist/app.css", import.meta.url))).length);
  const document = '<link rel="stylesheet" href="/commander/app.css">';
  const policy = "default-src 'self'; style-src 'self'";
  const candidate = createServer((req, res) => {
    if (req.url === "/commander/") {
      res.writeHead(200, { "Content-Type": "text/html", "Content-Security-Policy": policy });
      res.end(document);
    } else {
      res.writeHead(404, { "Content-Type": "text/plain" });
      res.end("not embedded");
    }
  });
  candidate.listen(0, "127.0.0.1");
  await once(candidate, "listening");
  t.after(() => new Promise(resolve => candidate.close(resolve)));

  const listener = createServer();
  listener.listen(0, "127.0.0.1");
  await once(listener, "listening");
  const port = listener.address().port;
  await new Promise(resolve => listener.close(resolve));
  const child = spawn(process.execPath, [fileURLToPath(new URL("real-hub-server.mjs", import.meta.url))], {
    env: {
      ...process.env,
      CAS_BIN: process.execPath, // No reset: this route check never launches a CLI.
      REAL_HUB_CONTROL_PORT: String(port),
      REAL_HUB_PORT: String(candidate.address().port),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const exited = once(child, "exit");
  t.after(async () => {
    child.kill("SIGTERM");
    await exited;
  });
  await Promise.race([
    once(child.stdout, "data"),
    exited.then(([code]) => { throw Error(`fixture exited before listening: ${code}`); }),
  ]);

  const origin = `http://127.0.0.1:${port}`;
  const missing = await fetch(`${origin}/commander/app.css`, { signal: AbortSignal.timeout(3000) });
  assert.equal(missing.status, 404, "an external stylesheet must not hide a missing embedded asset");
  assert.equal(await missing.text(), "not embedded");
  const index = await fetch(`${origin}/commander/`, { signal: AbortSignal.timeout(3000) });
  assert.equal(await index.text(), document);
  assert.equal(index.headers.get("content-security-policy"), policy);
});
