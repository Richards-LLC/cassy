import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { once } from "node:events";
import { stat, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { createRealHubHome, createRealHubEnvironment } from "./real-hub-home.mjs";
import { realHubProviderScript } from "./real-hub-provider.mjs";
import { darwinProcessIdentity, darwinOwnedProcesses } from "./real-hub-processes.mjs";

test("native launch cleanup tracks owned descendants after reparenting and rejects reused PIDs", async t => {
  const parent = spawn(process.execPath, ["-e", `
    const { spawn } = require('node:child_process');
    const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore', detached: true });
    console.log(child.pid);
    setInterval(() => {}, 1000);
  `]);
  const exited = once(parent, "exit");
  t.after(async () => { parent.kill(); await exited; });
  const [data] = await once(parent.stdout, "data");
  const descendantPid = Number(data.toString().trim());
  const identity = await darwinProcessIdentity(descendantPid);
  const root = await darwinProcessIdentity(parent.pid);
  t.after(async () => {
    if ((await darwinProcessIdentity(descendantPid))?.starttime === identity.starttime)
      process.kill(descendantPid, "SIGTERM");
  });
  const remembered = new Map();
  assert.deepEqual(await darwinOwnedProcesses({ ...root, starttime: "reused root PID" }, remembered), []);
  assert.deepEqual((await darwinOwnedProcesses(root, remembered)).map(row => row.pid), [descendantPid]);
  parent.kill();
  await exited;
  assert.deepEqual((await darwinOwnedProcesses(root, remembered)).map(row => row.pid), [descendantPid]);
  remembered.set(descendantPid, { ...identity, starttime: "reused PID" });
  assert.deepEqual(await darwinOwnedProcesses(root, remembered), []);
});

test("real-hub provider consumes bracketed CR submissions and exits only on the exit command", { timeout: 5000 }, async t => {
  const home = await createRealHubHome();
  t.after(() => rm(home, { recursive: true, force: true }));
  const cli = join(home, "cas");
  await writeFile(cli, `#!/usr/bin/python3
import os,sys,json
if '--message' in sys.argv:
 with open(os.path.join(os.environ['HOME'],'replies.jsonl'),'a') as out:
  out.write(json.loads(sys.argv[-1])['message']+'\\n')
`, { mode: 0o700 });
  const provider = join(home, "claude");
  await writeFile(provider, realHubProviderScript(cli), { mode: 0o700 });
  const child = spawn(provider, [], { cwd: home, env: { HOME: home, CAS_FACTORY_SESSION: "disposable" } });
  t.after(() => child.kill());
  const ended = once(child, "exit");
  let stderr = "";
  child.stderr.on("data", data => { stderr += data; });
  child.stdout.resume();
  const frame = text => `\x1b[200~${text}\x1b[201~\r`;
  // Match the daemon's paste + submit stream, including a multiline startup
  // that mentions /exit as prose. No terminal line discipline may be assumed.
  child.stdin.end(frame("Startup instructions\nThe /exit command ends a session.") +
    frame("ping launch supervisor") + frame("/exit"));
  assert.deepEqual(await ended, [0, null], stderr);
  assert.equal(await readFile(join(home, "replies.jsonl"), "utf8"), "Supervisor ready\nLaunch supervisor answered\n");
});

test("real-hub login probe resolves only the disposable provider despite host profiles", async t => {
  const home = await createRealHubHome();
  t.after(() => rm(home, { recursive: true, force: true }));
  const bin = join(home, "bin");
  const hostBin = join(home, "host-bin");
  await mkdir(bin);
  await mkdir(hostBin);
  // A hostile login profile models path_helper putting host installs first.
  // Its provider is a sentinel, so this regression never executes a real CLI.
  await writeFile(join(home, ".profile"), `export PATH='${hostBin}:/usr/bin:/bin'\n`);
  await writeFile(join(hostBin, "claude"), "#!/bin/sh\nprintf 'HOST PROVIDER LEAKED'\nexit 91\n", { mode: 0o700 });
  await writeFile(join(bin, "claude"), '#!/bin/sh\nprintf \'{"loggedIn":true}\\n\'\n', { mode: 0o700 });
  const env = await createRealHubEnvironment(home, join(home, "project"));
  const run = promisify(execFile);
  // Same login-shell boundary as hub/launch_env.rs::launch_path.
  const shell = env.SHELL ?? "/bin/sh";
  const { stdout } = await run(shell, ["-lc", "command -v claude"], { env });
  assert.equal(stdout.trim(), join(bin, "claude"), "login-shell lookup must keep the fixture provider first");
  const auth = await run(stdout.trim(), ["auth", "status", "--json"], { env });
  assert.deepEqual(JSON.parse(auth.stdout), { loggedIn: true });
});

test("real-hub candidate assets cannot fall back to external dist", { timeout: 15000 }, async t => {
  // The external bundle really has this stylesheet; this candidate lacks it.
  assert.ok((await stat(new URL("../../dist/app.css", import.meta.url))).isFile());
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
