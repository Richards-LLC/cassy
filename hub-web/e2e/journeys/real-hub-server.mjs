#!/usr/bin/env node
// A registry-owned test server. All hub/daemon children live under this one
// process and use a private HOME/project. No live factory or hosted relay.
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { createServer, request as httpRequest } from "node:http";
import { createServer as createTlsServer } from "node:https";
import { connect } from "node:net";
import {
  mkdir,
  writeFile,
  readFile,
  rm,
  readdir,
} from "node:fs/promises";
import { existsSync, createWriteStream } from "node:fs";
import { createRealHubHome, createRealHubEnvironment } from "./real-hub-home.mjs";
import { realHubProviderScript } from "./real-hub-provider.mjs";
import { darwinProcessIdentity, darwinOwnedProcesses } from "./real-hub-processes.mjs";
import { resolve, join } from "node:path";
import { redactQaText, redactQaValue } from "../../../scripts/visual-qa.mjs";
const run = promisify(execFile);
if (!process.env.CAS_BIN)
  throw Error("CAS_BIN must name the copied supervisor-built binary");
const cli = resolve(process.env.CAS_BIN);
const port = Number(process.env.REAL_HUB_CONTROL_PORT ?? 29925);
const hubPort = Number(process.env.REAL_HUB_PORT ?? 29926);
const tlsPort = Number(process.env.REAL_HUB_TLS_PORT ?? 29927);
if (process.env.REAL_HUB_DIST !== undefined)
  throw Error("REAL_HUB_DIST is unsupported: real-hub QA serves only candidate-embedded assets; use a different CAS_BIN for a baseline");
const origin = `http://127.0.0.1:${port}`;
const hubUrl = `https://journey-hub.ts.net:${tlsPort}`;
let state;
const delay = (ms) => new Promise((ok) => setTimeout(ok, ms));
async function cas(args, extra = {}) {
  return run(cli, args, {
    cwd: state.project,
    env: { ...state.env, ...extra },
    timeout: 15000,
    maxBuffer: 256 * 1024,
  });
}
function launch(args, log) {
  const stream = createWriteStream(join(state.home, log), {
    flags: "a",
    mode: 0o600,
  });
  const p = spawn(cli, args, {
    cwd: state.project,
    env: state.env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  p.stdout.pipe(stream, { end: false });
  p.stderr.pipe(stream, { end: false });
  p.once("exit", () => stream.end());
  p.once("error", (e) => stream.end(redactQaText(e)));
  return p;
}
async function stop(p) {
  if (!p || p.exitCode !== null || p.signalCode !== null) return;
  let exited = false;
  const ended = new Promise((ok) =>
    p.once("exit", () => {
      exited = true;
      ok();
    }),
  );
  p.kill("SIGTERM");
  await Promise.race([ended, delay(3000)]);
  if (!exited) {
    p.kill("SIGKILL");
    await Promise.race([ended, delay(3000)]);
    if (!exited) throw Error("owned child did not exit");
  }
}
async function healthy() {
  try {
    return (
      await fetch(`http://127.0.0.1:${hubPort}/v1/health`, {
        signal: AbortSignal.timeout(300),
      })
    ).ok;
  } catch {
    return false;
  }
}
async function startHub() {
  state.hub = launch(
    ["hub", "serve", "--no-tailscale-serve", "--port", String(hubPort)],
    "hub-process.log",
  );
  if (process.platform === "darwin") state.hubIdentity = await processIdentity(state.hub.pid);
  const end = performance.now() + 15000;
  while (performance.now() < end) {
    if (await healthy()) return;
    if (state.hub.exitCode !== null) break;
    await delay(100);
  }
  throw Error(
    `hub startup failed: ${redactQaText(await readFile(join(state.home, "hub-process.log"), "utf8"))}`,
  );
}
async function processIdentity(pid) {
  if (process.platform === "darwin") return darwinProcessIdentity(pid);
  try {
    const stat = await readFile(`/proc/${pid}/stat`, "utf8");
    const fields = stat.slice(stat.lastIndexOf(") ") + 2).split(/\s+/);
    return fields[0] === "Z" ? undefined
      : { pid: Number(pid), parent: Number(fields[1]), starttime: fields[19] };
  } catch (error) {
    if (["ENOENT", "ESRCH"].includes(error.code)) return;
    throw error;
  }
}
async function privateProcesses(home) {
  if (process.platform === "darwin")
    return darwinOwnedProcesses(state.hubIdentity, state.launchedProcesses);
  // PTY children have their own session/group, and can be reparented when
  // the reaper exits. This marked fixture's unique HOME follows those children.
  const expected = `HOME=${home}`;
  const processes = await Promise.all((await readdir("/proc"))
    .filter(pid => /^\d+$/.test(pid)).map(async pid => {
      try {
        const identity = await processIdentity(pid);
        if (!identity) return;
        const env = await readFile(`/proc/${pid}/environ`);
        if (!env.toString().split("\0").includes(expected)) return;
        if ((await processIdentity(pid))?.starttime === identity.starttime) return identity;
      } catch (error) {
        if (["ENOENT", "ESRCH", "EACCES"].includes(error.code)) return;
        throw error;
      }
    }));
  return processes.filter(Boolean);
}
async function signalOwned(identity, signal) {
  if ((await processIdentity(identity.pid))?.starttime !== identity.starttime) return;
  try { process.kill(identity.pid, signal); }
  catch (error) { if (error.code !== "ESRCH") throw error; }
}
async function stopLaunchedDaemon(home, owned) {
  // Stop the actual daemon before its reaper. The reaper must be allowed to
  // wait for the daemon and finish writing its exit receipt before rm.
  let processes = await privateProcesses(home);
  for (const child of processes.filter(p => p.parent === owned.pid))
    await signalOwned(child, "SIGTERM");
  const waitForExit = async () => {
    const deadline = performance.now() + 3000;
    do {
      processes = await privateProcesses(home);
      if (!processes.length) return true;
      await delay(50);
    } while (performance.now() < deadline);
    return false;
  };
  if (await waitForExit()) return;
  // Each signal is bounded to a PID with this private HOME and the same start
  // fingerprint, including a provider detached into its own process group.
  for (const signal of ["SIGTERM", "SIGKILL"]) {
    for (const child of processes) await signalOwned(child, signal);
    if (await waitForExit()) return;
  }
  throw Error("owned launch processes did not exit; fixture retained");
}
async function cleanup() {
  if (!state) return;
  if (state.paused) state.hub.kill("SIGCONT");
  // The native macOS daemon is a direct hub child, not the Linux shim's
  // recorded reaper. Capture its tree while the owned hub is still alive.
  if (state.newSession && process.platform === "darwin")
    await stopLaunchedDaemon(state.home, { pid: state.hub.pid });
  await stop(state.hub);
  await stop(state.daemon);
  for (const daemon of state.extraDaemons ?? []) await stop(daemon);
  const ownedLaunch = join(state.home, "launched-pid");
  if (existsSync(ownedLaunch)) {
    const owned = JSON.parse(await readFile(ownedLaunch, "utf8"));
    if (!Number.isSafeInteger(owned.pid) || owned.pid <= 1)
      throw Error("invalid owned launch PID; fixture retained");
    const leader = await processIdentity(owned.pid);
    if (leader && leader.starttime !== owned.starttime)
      throw Error("owned launch PID was reused; fixture retained");
    await stopLaunchedDaemon(state.home, owned);
  }
  if (state.tls) {
    for (const socket of state.sockets) socket.destroy();
    state.tls.closeAllConnections();
    await new Promise((ok) => state.tls.close(ok));
  }
  const root = state.home;
  if (existsSync(root)) {
    // rm may remove the marker before failing elsewhere. Retain ownership as
    // well as state so a later explicit cleanup can finish that partial delete.
    state.rootOwned ||= existsSync(join(root, ".real-hub-fixture"));
    if (!state.rootOwned) throw Error("fixture ownership marker missing");
    await rm(root, { recursive: true, force: true });
  }
  state = undefined;
}
function startDaemon(session = state.session) {
  const daemon = launch(
    [
      "factory",
      "daemon",
      "--session",
      session,
      "--cwd",
      state.project,
      "--workers",
      "0",
      "--no-worktrees",
      "--no-phone-home",
      "--foreground",
      "--supervisor-name",
      session === state.session ? "journey-supervisor" : session,
    ],
    `${session}-daemon-process.log`,
  );
  if (session === state.session) state.daemon = daemon;
  else {
    state.extraDaemons ??= [];
    state.extraDaemons.push(daemon);
    state.namedDaemons ??= new Map();
    state.namedDaemons.set(session, daemon);
  }
  return daemon;
}
// Real daemon processes create both stale metadata and an end receipt; no
// catalog payloads are fabricated for the stopped-session regression.
async function stoppedSessions() {
  const result = [];
  state.extraDaemons ??= [];
  for (const kind of ["crashed", "ended"]) {
    const name = `${kind}-journey`;
    const daemon = launch(["factory", "daemon", "--session", name, "--cwd", state.project,
      "--workers", "0", "--no-worktrees", "--no-phone-home", "--foreground",
      "--supervisor-name", `${kind}-supervisor`], `${kind}-process.log`);
    state.extraDaemons.push(daemon);
    const dir = join(state.home, ".cas", "sessions");
    const deadline = performance.now() + 20000;
    let metadata;
    while (performance.now() < deadline) {
      for (const file of await readdir(dir)) {
        try { const value = JSON.parse(await readFile(join(dir, file), "utf8"));
          if (value.name === name && value.ws_port) metadata = value;
        } catch { /* The writer may be replacing metadata. */ }
      }
      if (metadata) break;
      if (daemon.exitCode !== null) throw Error(`${kind} fixture daemon exited early`);
      await delay(100);
    }
    if (!metadata) throw Error(`${kind} fixture daemon metadata missing`);
    if (kind === "crashed") {
      const exited = new Promise(resolve => daemon.once("exit", resolve));
      daemon.kill("SIGKILL");
      await exited;
    } else await stop(daemon);
    result.push({ name, pid: daemon.pid, signal: kind === "crashed" ? "SIGKILL" : "SIGTERM" });
  }
  return result;
}
async function reset({ messageControl = false, newSession = false, nativeCommander = false } = {}) {
  await cleanup();
  const home = await createRealHubHome();
  await writeFile(join(home, ".real-hub-fixture"), String(process.pid));
  const project = join(home, "project"),
    bin = join(home, "bin");
  await mkdir(project);
  await mkdir(bin);
  await writeFile(
    join(bin, "claude"),
    `#!/usr/bin/python3\nimport sys,time\nif sys.argv[1:3] == ['auth','status']: print('{\"loggedIn\":true}');sys.exit(0)\nif '--version' in sys.argv: print('2.1.47');sys.exit(0)\nif '--help' in sys.argv: print('--dangerously-skip-permissions --model --agent --mcp-config');sys.exit(0)\nprint('Claude Code disposable test supervisor\\n> ',flush=True)\nwhile True:\n sys.stdin.readline()\n time.sleep(.1)\n`,
    { mode: 0o700 },
  );
  if (newSession) await writeFile(join(bin, "claude"), realHubProviderScript(cli), { mode: 0o700 });
  await mkdir(join(home, ".claude"));
  // The user-service manager is the sole host boundary: retain this fixture's
  // HOME and containment instead of asking the real login manager to launch
  // into the operator's HOME. The hub's actual reaper, daemon and PTY execute.
  if (newSession) await writeFile(join(bin, "systemd-run"), `#!/usr/bin/python3
import sys,os,subprocess,json
args=sys.argv[1:]
cwd=next(a.split('=',1)[1] for a in args if a.startswith('--working-directory='))
log=next(a.split('append:',1)[1] for a in args if a.startswith('--property=StandardError=append:'))
with open(log,'ab',buffering=0) as out:
 p=subprocess.Popen(args[args.index('--')+1:],cwd=cwd,stdin=subprocess.DEVNULL,stdout=out,stderr=out,start_new_session=True)
starttime=open('/proc/'+str(p.pid)+'/stat').read().split(') ',1)[1].split()[19]
with open(os.path.join(os.environ['HOME'],'launched-pid'),'w') as f: json.dump(dict(pid=p.pid,starttime=starttime),f)
`, { mode: 0o700 });
  const env = await createRealHubEnvironment(home, project);
  state = { home, project, env, session: "real-hub-journey", newSession, launchedProcesses: new Map() };
  await run("git", ["init", "-q", project], { env, timeout: 5000 });
  await cas(["init", "--yes", "--no-integrations"]);
  await writeFile(
    join(project, ".cas", "config.toml"),
    "[factory.ai_enrichment]\nenabled = false\n",
  );
  if (newSession) {
    // Match the production user service: a machine hub has no CAS_ROOT.
    delete state.env.CAS_ROOT;
    await writeFile(join(home, ".cas", "config.toml"), `[hub]\nlaunch_roots = [${JSON.stringify(home)}]\n`);
  }
  if (!newSession) startDaemon();
  const deadline = performance.now() + 20000;
  let meta;
  while (!newSession && performance.now() < deadline) {
    const dir = join(home, ".cas", "sessions");
    if (existsSync(dir)) {
      for (const f of await readdir(dir)) {
        try {
          const m = JSON.parse(await readFile(join(dir, f), "utf8"));
          if (m.name === state.session && m.ws_port) meta = m;
        } catch {}
      }
    }
    if (meta) break;
    if (state.daemon.exitCode !== null) break;
    await delay(100);
  }
  if (!newSession && !meta)
    throw Error(
      `real daemon startup failed: ${redactQaText(await readFile(join(home, "daemon-process.log"), "utf8"))}`,
    );
  await run(
    "openssl",
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-keyout",
      join(home, "tls.key"),
      "-out",
      join(home, "tls.crt"),
      "-days",
      "1",
      "-subj",
      "/CN=journey-hub.ts.net",
    ],
    { timeout: 10000 },
  );
  const tls = createTlsServer(
    {
      key: await readFile(join(home, "tls.key")),
      cert: await readFile(join(home, "tls.crt")),
    },
    proxyToHub,
  );
  // A transparent TLS transport to the actual hub. Application frames are
  // neither fabricated nor decoded; actual upstream shutdown drops sockets.
  state.sockets = new Set();
  tls.on("connection", (socket) => {
    state.sockets.add(socket);
    socket.on("close", () => state?.sockets.delete(socket));
  });
  tls.on("upgrade", (req, socket, head) => {
    const upstream = connect(hubPort, "127.0.0.1", () => {
      upstream.write(
        `${req.method} ${req.url} HTTP/${req.httpVersion}\r\n${Object.entries(
          req.headers,
        )
          .map(([k, v]) => `${k}: ${v}`)
          .join("\r\n")}\r\n\r\n`,
      );
      if (head.length) upstream.write(head);
      socket.pipe(upstream);
      upstream.pipe(socket);
    });
    upstream.on("error", () => socket.destroy());
    socket.on("error", () => upstream.destroy());
    socket.on("close", () => upstream.destroy());
    upstream.on("close", () => socket.destroy());
  });
  state.tls = tls;
  await new Promise((ok, fail) => {
    tls.once("error", fail);
    tls.listen(tlsPort, "127.0.0.1", ok);
  });
  await startHub();
  // Native cells load the hub's embedded document and its headers directly.
  const commanderOrigin = nativeCommander ? `http://127.0.0.1:${hubPort}` : origin;
  const pairingHubUrl = nativeCommander ? commanderOrigin : hubUrl;
  const pair = JSON.parse(
    (
      await cas([
        "--json",
        "hub",
        "pair",
        ...(newSession ? ["--scopes", "machine:read,session:read,pane:read,pane:input,message:send,session-launch"] : messageControl ? ["--scopes", "machine:read,session:read,pane:read,pane:input,message:send"] : []),
        "--origin",
        commanderOrigin,
        "--hub-url",
        pairingHubUrl,
      ])
    ).stdout,
  );
  return {
    pairUrl: pair.url,
    session: state.session,
    hubUrl,
    daemonPid: state.daemon?.pid,
    project,
    hubPid: state.hub.pid,
    version: (await cas(["--version"])).stdout.trim(),
  };
}
async function queue(message) {
  if (typeof message !== "string" || message.length > 1000)
    throw Error("bounded fixture message required");
  const payload = {
    schema_version: 2,
    reply_to: null,
    message,
    summary: message,
    device_id: "*",
    kind: "status",
    attachments: [],
  };
  const out = await cas(
    [
      "--json",
      "factory",
      "message",
      "--session",
      state.session,
      "--project-dir",
      state.project,
      "--target",
      "operator",
      "--from",
      "supervisor",
      "--no-wrap",
      "--message",
      JSON.stringify(payload),
    ],
    { CAS_FACTORY_SESSION: state.session },
  );
  return JSON.parse(out.stdout);
}
async function ask() {
  const out = await cas([
    "--json", "factory", "message", "--session", state.session,
    "--project-dir", state.project, "--target", "operator", "--from", "supervisor",
    "--kind", "ask", "--message", "Keep testing or stop?",
    "--option", "Keep testing", "--option", "Stop testing",
  ], { CAS_FACTORY_SESSION: state.session });
  return JSON.parse(out.stdout);
}
async function answers(notificationId) {
  if (!Number.isSafeInteger(notificationId) || notificationId <= 0)
    throw Error("a positive ask notification id is required");
  // Independent read-back from the actual store; no fixture-authored answers.
  const { stdout } = await run("python3", ["-c", `
import sqlite3,json,sys
c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
c.row_factory=sqlite3.Row
marker='[CAS reply: explicitly acknowledges notification_id='+sys.argv[2]+']\\n'
rows=c.execute("select id,target,prompt,operator_verified from prompt_queue where source like 'commander:%' and prompt like ?",(marker+'%',))
print(json.dumps([dict(row) for row in rows]))
`, join(state.project, ".cas", "cas.db"), String(notificationId)], { timeout: 5000 });
  return JSON.parse(stdout);
}
async function readLogs(directory) {
  if (!existsSync(directory)) return "";
  const files = (await readdir(directory))
    .filter((name) => name.endsWith(".log"))
    .sort();
  return (
    await Promise.all(
      files.map((name) => readFile(join(directory, name), "utf8")),
    )
  ).join("\n");
}
async function evidence() {
  const auditPath = join(state.home, ".cas", "hub", "audit.jsonl");
  const audit = existsSync(auditPath)
    ? (await readFile(auditPath, "utf8"))
        .trim()
        .split("\n")
        .filter(Boolean)
        .map((x) => JSON.parse(x))
    : [];
  return {
    audit: redactQaValue(audit),
    hubLog: redactQaText(await readLogs(join(state.home, ".cas", "hub"))),
    hubTrace: redactQaText(
      await readLogs(join(state.home, ".cas", "hub", "logs")),
    ),
    daemonLog: redactQaText(
      await readLogs(join(state.project, ".cas", "logs")),
    ),
    launchLogs: existsSync(join(state.home, ".cas", "logs", "factory"))
      ? await Promise.all((await readdir(join(state.home, ".cas", "logs", "factory"))).map(async (name) => ({ name, log: redactQaText(await readLogs(join(state.home, ".cas", "logs", "factory", name))) })))
      : [],
    hubPid: state.hub?.pid,
    daemonPid: state.daemon?.pid,
  };
}
function proxyToHub(req, res) {
  const upstream = httpRequest(
    {
      host: "127.0.0.1",
      port: hubPort,
      path: req.url,
      method: req.method,
      headers: req.headers,
    },
    (r) => {
      res.writeHead(r.statusCode ?? 502, r.headers);
      r.pipe(res);
    },
  );
  upstream.on("error", () => {
    if (res.headersSent) res.destroy();
    else {
      res.writeHead(502);
      res.end();
    }
  });
  res.on("close", () => upstream.destroy());
  req.pipe(upstream);
}
let chain = Promise.resolve();
const server = createServer(async (req, res) => {
  try {
    const path = new URL(req.url ?? "/", origin).pathname;
    if (path === "/commander" || path.startsWith("/commander/")) {
      proxyToHub(req, res);
      return;
    }
    if (req.method === "GET" && path === "/health") {
      res.end("ready");
      return;
    }
    if (req.method === "GET" && path === "/") {
      res.writeHead(302, { Location: "/commander/" });
      res.end();
      return;
    }
    if (
      req.method !== "POST" ||
      req.headers["content-type"] !== "application/json" ||
      (req.headers.origin && req.headers.origin !== origin)
    )
      throw Error("local JSON control required");
    let body = "";
    for await (const chunk of req) {
      body += chunk;
      if (body.length > 4096) throw Error("control body too large");
    }
    const data = JSON.parse(body || "{}");
    const job = chain.then(async () => {
      switch (path) {
        case "/reset":
          return reset(data);
        case "/stop":
          if (state.paused) {
            state.hub.kill("SIGCONT");
            state.paused = false;
          }
          await stop(state.hub);
          return { stopped: true };
        case "/pause":
          if (!state.hub.kill("SIGSTOP"))
            throw Error("owned hub could not pause");
          state.paused = true;
          return { paused: true };
        case "/resume":
          if (!state.hub.kill("SIGCONT"))
            throw Error("owned hub could not resume");
          state.paused = false;
          return { resumed: true };
        case "/start":
          await startHub();
          return { started: true };
        case "/stopped-sessions":
          return stoppedSessions();
        case "/daemon-stop": {
          const daemon = data.session ? state.namedDaemons?.get(data.session) : state.daemon;
          if (!daemon) throw Error("unknown owned fixture daemon");
          await stop(daemon);
          return { daemonPid: daemon.pid };
        }
        case "/daemon-start": {
          const daemon = startDaemon(data.session);
          return { daemonPid: daemon.pid };
        }
        case "/attachment": {
          const path = join(state.project, ".cas", "factory-daemon", state.session + ".loop.json");
          const loop = existsSync(path) ? JSON.parse(await readFile(path, "utf8")) : {};
          return { daemonPid: state.daemon.pid, loopPid: loop.pid,
            commander_clients: loop.commander_clients, passes: loop.passes,
            written_at: loop.written_at, last_pass_at: loop.last_pass_at };
        }
        case "/ask":
          return ask();
        case "/answers":
          return answers(data.notificationId);
        case "/queue":
          return queue(data.message);
        case "/evidence":
          return evidence();
        case "/cleanup":
          await cleanup();
          return { cleaned: true };
        default:
          throw Error("unknown fixture control");
      }
    });
    chain = job.catch(() => undefined);
    const result = await job;
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(result));
  } catch (e) {
    res.writeHead(500, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ error: redactQaText(e) }));
  }
});
for (const sig of ["SIGTERM", "SIGINT"])
  process.once(sig, () => {
    cleanup().finally(() => server.close(() => process.exit(0)));
  });
server.listen(port, "127.0.0.1", () =>
  console.log(`real-hub fixture control and candidate-embedded assets at ${origin}; binary ${cli}`),
);
