#!/usr/bin/env node
// A registry-owned test server. All hub/daemon children live under this one
// process and use a private HOME/project. No live factory or hosted relay.
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { createServer, request as httpRequest } from "node:http";
import { createServer as createTlsServer } from "node:https";
import { connect } from "node:net";
import {
  mkdtemp,
  mkdir,
  writeFile,
  readFile,
  rm,
  realpath,
  readdir,
} from "node:fs/promises";
import { existsSync, createWriteStream } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join, extname } from "node:path";
import { fileURLToPath } from "node:url";
import { redactQaText, redactQaValue } from "../../../scripts/visual-qa.mjs";
const run = promisify(execFile);
if (!process.env.CAS_BIN)
  throw Error("CAS_BIN must name the copied supervisor-built binary");
const cli = resolve(process.env.CAS_BIN);
const port = Number(process.env.REAL_HUB_CONTROL_PORT ?? 29925);
const hubPort = Number(process.env.REAL_HUB_PORT ?? 29926);
const tlsPort = Number(process.env.REAL_HUB_TLS_PORT ?? 29927);
const dist = resolve(
  process.env.REAL_HUB_DIST ??
    fileURLToPath(new URL("../../dist", import.meta.url)),
);
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
async function cleanup() {
  if (!state) return;
  if (state.paused) state.hub.kill("SIGCONT");
  await stop(state.hub);
  await stop(state.daemon);
  const ownedLaunch = join(state.home, "launched-pid");
  if (existsSync(ownedLaunch)) {
    const pid = Number(await readFile(ownedLaunch, "utf8"));
    if (Number.isSafeInteger(pid) && pid > 1) {
      try { process.kill(-pid, "SIGTERM"); } catch (e) { if (e.code !== "ESRCH") throw e; }
    }
  }
  if (state.tls) {
    for (const socket of state.sockets) socket.destroy();
    state.tls.closeAllConnections();
    await new Promise((ok) => state.tls.close(ok));
  }
  const root = state.home;
  state = undefined;
  if (existsSync(join(root, ".real-hub-fixture")))
    await rm(root, { recursive: true, force: true });
}
function startDaemon() {
  state.daemon = launch(
    [
      "factory",
      "daemon",
      "--session",
      state.session,
      "--cwd",
      state.project,
      "--workers",
      "0",
      "--no-worktrees",
      "--no-phone-home",
      "--foreground",
      "--supervisor-name",
      "journey-supervisor",
    ],
    "daemon-process.log",
  );
}
async function reset({ messageControl = false, newSession = false } = {}) {
  await cleanup();
  const home = await mkdtemp(join(await realpath(tmpdir()), "cas-real-hub-"));
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
  if (newSession) await writeFile(join(bin, "claude"), `#!/usr/bin/python3
import sys,os,json,subprocess
if sys.argv[1:3] == ['auth','status']: print('{"loggedIn":true}');sys.exit(0)
if '--version' in sys.argv: print('2.1.47');sys.exit(0)
if '--help' in sys.argv: print('--dangerously-skip-permissions --model --agent --mcp-config');sys.exit(0)
def reply(message):
 payload=json.dumps(dict(schema_version=2,reply_to=None,message=message,summary=message,device_id='*',kind='status',attachments=[]))
 subprocess.run([${JSON.stringify(cli)},'--json','factory','message','--session',os.environ['CAS_FACTORY_SESSION'],'--project-dir',os.getcwd(),'--target','operator','--from','supervisor','--no-wrap','--message',payload],stdout=subprocess.DEVNULL,check=True)
print('Claude Code disposable test supervisor\\n> ',flush=True)
started=False
for line in sys.stdin:
 if not started:
  reply('Supervisor ready');started=True
 if 'ping launch supervisor' in line: reply('Launch supervisor answered')
`, { mode: 0o700 });
  await mkdir(join(home, ".claude"));
  // The user-service manager is the sole host boundary: retain this fixture's
  // HOME and containment instead of asking the real login manager to launch
  // into the operator's HOME. The hub's actual reaper, daemon and PTY execute.
  if (newSession) await writeFile(join(bin, "systemd-run"), `#!/usr/bin/python3
import sys,os,subprocess
args=sys.argv[1:]
cwd=next(a.split('=',1)[1] for a in args if a.startswith('--working-directory='))
log=next(a.split('append:',1)[1] for a in args if a.startswith('--property=StandardError=append:'))
with open(log,'ab',buffering=0) as out:
 p=subprocess.Popen(args[args.index('--')+1:],cwd=cwd,stdin=subprocess.DEVNULL,stdout=out,stderr=out,start_new_session=True)
with open(os.path.join(os.environ['HOME'],'launched-pid'),'w') as f: f.write(str(p.pid))
`, { mode: 0o700 });
  const env = {
    HOME: home,
    XDG_CONFIG_HOME: join(home, "config"),
    XDG_RUNTIME_DIR: join(home, "runtime"),
    CAS_ROOT: join(project, ".cas"),
    CAS_AGENT_NAME: process.env.CAS_AGENT_NAME ?? "real-hub-qa",
    PATH: `${bin}:/usr/bin:/bin`,
    TERM: "xterm-256color",
    LANG: "C.UTF-8",
    CAS_SKIP_FACTORY_TOOLING: "1",
    CAS_SUPPRESS_DUPLICATE_WARNING: "1",
    RUST_LOG: "cas=debug",
  };
  state = { home, project, env, session: "real-hub-journey" };
  await run("git", ["init", "-q", project], { env, timeout: 5000 });
  await cas(["init", "--yes", "--no-integrations"]);
  await writeFile(
    join(project, ".cas", "config.toml"),
    "[factory]\nai_enrichment = false\n",
  );
  if (newSession) {
    // A machine hub commonly inherits the project that originally started it.
    const launcher = join(home, "launcher");
    await mkdir(launcher);
    await run("git", ["init", "-q", launcher], { env, timeout: 5000 });
    await run(cli, ["init", "--yes", "--no-integrations"], {
      cwd: launcher, env: { ...env, CAS_ROOT: join(launcher, ".cas") }, timeout: 15000,
    });
    state.env.CAS_ROOT = join(launcher, ".cas");
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
    (req, res) => {
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
    },
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
  const pair = JSON.parse(
    (
      await cas([
        "--json",
        "hub",
        "pair",
        ...(newSession ? ["--scopes", "machine:read,session:read,pane:read,pane:input,message:send,session-launch"] : messageControl ? ["--scopes", "machine:read,session:read,pane:read,pane:input,message:send"] : []),
        "--origin",
        origin,
        "--hub-url",
        hubUrl,
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
const types = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
};
let chain = Promise.resolve();
const server = createServer(async (req, res) => {
  try {
    const path = new URL(req.url ?? "/", origin).pathname;
    if (path.startsWith("/commander")) {
      const inner = path.replace(/^\/commander\/?/, "/");
      const file = resolve(dist, `.${inner === "/" ? "/index.html" : inner}`);
      if (!file.startsWith(`${dist}/`)) throw Error("invalid asset");
      if (!existsSync(file)) {
        res.writeHead(404);
        res.end();
        return;
      }
      const bytes = await readFile(file);
      res.writeHead(200, {
        "Content-Type": types[extname(file)] ?? "application/octet-stream",
        "Cache-Control": "no-store",
      });
      res.end(bytes);
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
        case "/daemon-stop":
          await stop(state.daemon);
          return { daemonPid: state.daemon.pid };
        case "/daemon-start":
          startDaemon();
          return { daemonPid: state.daemon.pid };
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
  console.log(`real-hub fixture control and dist at ${origin}; binary ${cli}`),
);
