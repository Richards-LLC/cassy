#!/usr/bin/env node
// Registry-owned, disposable real CLI/daemon/hub. Only this run's principals
// may be revoked. Cloud APIs are live; the local control has no bearer output.
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { createServer } from "node:http";
import { createWriteStream } from "node:fs";
import { mkdir, writeFile, readFile, readdir, rm } from "node:fs/promises";
import { join, resolve } from "node:path";
import { liveOptions, loadSandboxProfile, assertSandboxIdentity, isolatedCasEnv } from "./operator-inbox-live-safety.mjs";
import { redactQaText } from "../../scripts/visual-qa.mjs";

const options = liveOptions(process.env); // Before credentials or HTTP.
assert(process.env.CAS_BIN, "CAS_BIN must name the supervisor-built binary");
const cli = resolve(process.env.CAS_BIN);
const controlPort = Number(process.env.CAS_2077_CONTROL_PORT ?? 29935);
const hubPort = Number(process.env.CAS_2077_HUB_PORT ?? 29936);
const run = promisify(execFile);
const delay = (ms) => new Promise((ok) => setTimeout(ok, ms));
let profile, state, cleaned = false;
const secret = () => profile ? [profile.token] : [];
async function authority(path, body) {
  const response = await fetch(`${profile.cloudOrigin}${path}`, {
    method: body ? "POST" : "GET", headers: { Authorization: `Bearer ${profile.token}`, ...(body ? { "Content-Type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(20_000),
  });
  assert(response.ok, `sandbox account request refused: ${response.status} ${path}`);
  return response.json();
}
async function cas(args, extra = {}) {
  try {
    const output = await run(cli, args, { cwd: state.project, env: { ...state.env, ...extra }, timeout: 45_000, maxBuffer: 512 * 1024 });
    return output.stdout;
  } catch (error) {
    // Do not print command arguments (approval codes) or execFile's env.
    throw Error(`disposable CLI failed: ${redactQaText(error.stderr ?? String(error.code), secret())}`);
  }
}
async function jsonCas(args, extra) { return JSON.parse(await cas(["--json", ...args], extra)); }
function launch(args, name) {
  const stream = createWriteStream(join(options.root, name), { flags: "a", mode: 0o600 });
  const child = spawn(cli, args, { cwd: state.project, env: state.env, stdio: ["ignore", "pipe", "pipe"] });
  child.stdout.pipe(stream, { end: false }); child.stderr.pipe(stream, { end: false });
  child.once("exit", () => stream.end()); child.once("error", () => stream.end("owned child launch failed\n"));
  return child;
}
async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const ended = new Promise((ok) => child.once("exit", ok));
  child.kill("SIGTERM"); await Promise.race([ended, delay(5000)]);
  if (child.exitCode === null && child.signalCode === null) { child.kill("SIGKILL"); await ended; }
}
async function inventory() {
  const all = await authority("/api/operator/principals");
  return {
    devices: (all.devices ?? []).filter((x) => x.label?.startsWith(`${state.label}-`)),
    machines: (all.machines ?? []).filter((x) => x.machine_id === state.machine?.machine_id || x.label === state.label),
  };
}
async function cleanup() {
  if (!state || cleaned) return { cleaned };
  await stop(state.hub); await stop(state.daemon);
  const receipt = { created_at: new Date().toISOString(), account_id: profile.accountId, devices: [], machines: [], failures: [] };
  try {
    const owned = await inventory();
    for (const [kind, rows, idField] of [["devices", owned.devices, "device_id"], ["machines", owned.machines, "machine_id"]]) {
      for (const principal of rows) {
        const id = principal[idField];
        try {
          const reply = await authority(`/api/operator/${kind}/${encodeURIComponent(id)}/revoke`, { wire_version: 1 });
          receipt[kind].push({ id, reply });
        } catch { receipt.failures.push({ kind, id }); }
      }
    }
    const remaining = await inventory();
    receipt.remaining_active = [...remaining.devices, ...remaining.machines].filter((x) => x.status === "active").map((x) => x.device_id ?? x.machine_id);
  } catch { receipt.failures.push({ kind: "inventory" }); }
  await writeFile(join(options.artifacts, "cleanup.json"), `${JSON.stringify(receipt, null, 2)}\n`);
  assert.equal(receipt.failures.length, 0, "principal cleanup failed; see cleanup.json");
  assert.equal(receipt.remaining_active.length, 0, "run principals still active");
  // Secret-bearing HOME/logs/DB are never artifacts. Keep only scrubbed receipts.
  // The test runner still owns its private trace directory until it exits.
  for (const name of ["native", "daemon.log", "hub.log"]) await rm(join(options.root, name), { recursive: true, force: true });
  cleaned = true;
  return { cleaned: true, receipt };
}
async function prepare() {
  assert(!state, "one disposable run per registry entry");
  profile = await loadSandboxProfile(process.env.CAS_2077_CREDENTIAL_FILE ?? join(process.env.HOME, ".config/cas/cloud-test-profiles.env"), options.profile);
  assertSandboxIdentity(await authority("/api/me"), options.profile);
  await mkdir(options.root, { recursive: true, mode: 0o700 });
  const nativeRoot = join(options.root, "native");
  await mkdir(nativeRoot, { mode: 0o700 }); // Refuse existing native runs.
  await mkdir(options.artifacts, { recursive: true });
  const env = isolatedCasEnv(nativeRoot, process.env), project = join(nativeRoot, "project"), bin = join(nativeRoot, "bin");
  state = { env, project, label: `cassy-2077-${Date.now()}`, session: "inbox-live-2077" };
  await mkdir(project); await mkdir(bin); await mkdir(join(env.HOME, ".cas"), { recursive: true });
  env.PATH = `${bin}:${env.PATH}`;
  await writeFile(join(bin, "claude"), `#!/usr/bin/python3\nimport sys,time\nif '--version' in sys.argv: print('2.1.47');sys.exit(0)\nif '--help' in sys.argv: print('--dangerously-skip-permissions --model --agent --mcp-config');sys.exit(0)\nprint('Disposable inbox test supervisor\\n> ',flush=True)\nwhile True:\n sys.stdin.readline()\n time.sleep(.1)\n`, { mode: 0o700 });
  await run("git", ["init", "-q", project], { env });
  await cas(["init", "--yes", "--no-integrations"]);
  await writeFile(join(project, ".cas/config.toml"), `[factory.ai_enrichment]\nenabled = false\n[project]\ncanonical_id = "${profile.projectId}"\n`);
  await writeFile(env.CAS_USER_CLOUD_JSON, JSON.stringify({ endpoint: profile.cloudOrigin, token: profile.token, email: profile.email }), { mode: 0o600 });
  // Pinning precedes discovery registration; this is only the private host registry.
  await cas(["init", "--yes", "--no-integrations"]);
  state.machine = await jsonCas(["hub", "operator", "enroll", "--label", state.label, "--project", profile.projectId]);
  assert.equal(state.machine.account_id, profile.accountId);
  await jsonCas(["hub", "operator", "bind"]);
  state.daemon = launch(["factory", "daemon", "--session", state.session, "--cwd", project, "--workers", "0", "--no-worktrees", "--no-phone-home", "--foreground", "--supervisor-name", "inbox-supervisor"], "daemon.log");
  const until = Date.now() + 30_000;
  for (;;) {
    let ready = false;
    try { for (const file of await readdir(join(env.HOME, ".cas/sessions"))) { const m = JSON.parse(await readFile(join(env.HOME, ".cas/sessions", file), "utf8")); if (m.name === state.session && m.ws_port) ready = true; } } catch {}
    if (ready) break;
    assert(Date.now() < until && state.daemon.exitCode === null, "disposable factory startup failed");
    await delay(100);
  }
  return { ...state.machine, label: state.label, session: state.session, account_id: profile.accountId, binary: (await cas(["--version"])).trim() };
}
async function queue(message) {
  assert(typeof message === "string" && message.startsWith(state.label) && message.length < 1000, "bounded run-owned message required");
  const payload = { schema_version: 2, reply_to: null, message, summary: message, device_id: "*", kind: "status", attachments: [] };
  const queued = await jsonCas(["factory", "message", "--session", state.session, "--project-dir", state.project, "--target", "operator", "--from", "supervisor", "--no-wrap", "--message", JSON.stringify(payload)], { CAS_FACTORY_SESSION: state.session });
  const drained = await jsonCas(["hub", "operator", "drain"]);
  return { queued, drained };
}
async function startHub() {
  assert(!state.hub || state.hub.exitCode !== null || state.hub.signalCode !== null, "hub already running");
  state.hub = launch(["hub", "serve", "--port", String(hubPort)], "hub.log");
  const until = Date.now() + 20_000;
  while (Date.now() < until) {
    try { if ((await fetch(`http://127.0.0.1:${hubPort}/v1/health`, { signal: AbortSignal.timeout(300) })).ok) return { started: true }; } catch {}
    assert(state.hub.exitCode === null, "disposable hub exited"); await delay(100);
  }
  throw Error("disposable hub did not become healthy");
}
async function evidence() {
  const code = `import sqlite3,json,sys\nc=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)\nc.row_factory=sqlite3.Row\ndef rows(q): return [dict(r) for r in c.execute(q)]\nprint(json.dumps({'admissions':rows('SELECT command_id,history_event_id,device_id,prompt_id,receipt_id,receipt_sent_at FROM operator_command_admissions'),'outbox':rows('SELECT event_id,key_epoch,sequence,receipt_kind,seal_count FROM operator_cloud_outbox'),'prompts':rows("SELECT id,source,target,prompt FROM prompt_queue WHERE source LIKE 'commander:%'")}))`;
  const output = await run("python3", ["-c", code, join(state.project, ".cas/cas.db")], { env: state.env, timeout: 5000 });
  return JSON.parse(output.stdout);
}
let chain = Promise.resolve();
const server = createServer(async (req, res) => {
  try {
    if (req.method === "GET" && req.url === "/health") { res.end("ready"); return; }
    assert(req.method === "POST" && req.headers["content-type"] === "application/json" && !req.headers.origin, "local CLI JSON control required");
    let body = ""; for await (const chunk of req) { body += chunk; assert(body.length < 4096); }
    const data = JSON.parse(body || "{}");
    const job = chain.then(async () => {
      if (req.url === "/prepare") return prepare();
      if (req.url === "/cleanup") return cleanup();
      assert(state, "prepare required");
      switch (req.url) {
        case "/approve":
          assert(/^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/.test(data.code));
          await jsonCas(["hub", "operator", "approve", data.code, ...(data.reply ? ["--command-scope", profile.projectId] : []), ...(data.manage === true ? ["--manage"] : [])]); return { approved: true };
        case "/queue": return queue(data.message);
        case "/devices": return inventory();
        case "/start": return startHub();
        case "/stop": await stop(state.hub); return { stopped: true, stopped_at: new Date().toISOString() };
        case "/evidence": return evidence();
        case "/revoke": {
          const { devices } = await inventory(); assert(devices.some((x) => x.device_id === data.id), "only run-created device can be revoked");
          return jsonCas(["hub", "operator", "revoke-device", data.id]);
        }
        default: throw Error("unknown control");
      }
    });
    chain = job.catch(() => undefined);
    const result = await job; res.writeHead(200, { "Content-Type": "application/json" }); res.end(JSON.stringify(result));
  } catch (error) { res.writeHead(500, { "Content-Type": "application/json" }); res.end(JSON.stringify({ error: redactQaText(String(error), secret()) })); }
});
for (const signal of ["SIGTERM", "SIGINT"]) process.once(signal, () => {
  chain.finally(cleanup).then(() => server.close(() => process.exit(0))).catch(() => server.close(() => process.exit(1)));
});
server.listen(controlPort, "127.0.0.1", () => console.log(`cas-2077 native control at 127.0.0.1:${controlPort}; Cloud profile ${options.profile}; root ${options.root}`));
