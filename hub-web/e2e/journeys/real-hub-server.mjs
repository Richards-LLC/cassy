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
async function processIdentity(pid) {
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
  await stop(state.hub);
  await stop(state.daemon);
  await stop(state.progressWorker);
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
async function reset({ messageControl = false, newSession = false, progressControl = false } = {}) {
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
import sys,os,json,subprocess,uuid
from datetime import datetime,timezone
if sys.argv[1:3] == ['auth','status']: print('{"loggedIn":true}');sys.exit(0)
if '--version' in sys.argv: print('2.1.47');sys.exit(0)
if '--help' in sys.argv: print('--dangerously-skip-permissions --model --agent --mcp-config');sys.exit(0)
sid=sys.argv[sys.argv.index('--session-id')+1] if '--session-id' in sys.argv else str(uuid.uuid4())
transcript=os.path.join(os.environ['HOME'],'.claude','projects',os.getcwd().replace('/','-').replace('.','-'),sid+'.jsonl')
os.makedirs(os.path.dirname(transcript),exist_ok=True)
open(transcript,'a').close()
def hook(event):
 payload=json.dumps(dict(session_id=sid,cwd=os.getcwd(),hook_event_name=event,transcript_path=transcript))
 subprocess.run([${JSON.stringify(cli)},'hook',event],input=payload,text=True,stdout=subprocess.DEVNULL,check=True)
hook('SessionStart')
def reply(message):
 with open(transcript,'a') as f: f.write(json.dumps(dict(type='system',subtype='turn_duration',sessionId=sid,timestamp=datetime.now(timezone.utc).isoformat()))+'\\n')
 payload=json.dumps(dict(schema_version=2,reply_to=None,message=message,summary=message,device_id='*',kind='status',attachments=[]))
 subprocess.run([${JSON.stringify(cli)},'--json','factory','message','--session',os.environ['CAS_FACTORY_SESSION'],'--project-dir',os.getcwd(),'--target','operator','--from','supervisor','--no-wrap','--message',payload],stdout=subprocess.DEVNULL,check=True)
 hook('Stop')
 print('> ',flush=True)
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
import sys,os,subprocess,json
args=sys.argv[1:]
cwd=next(a.split('=',1)[1] for a in args if a.startswith('--working-directory='))
log=next(a.split('append:',1)[1] for a in args if a.startswith('--property=StandardError=append:'))
with open(log,'ab',buffering=0) as out:
 p=subprocess.Popen(args[args.index('--')+1:],cwd=cwd,stdin=subprocess.DEVNULL,stdout=out,stderr=out,start_new_session=True)
starttime=open('/proc/'+str(p.pid)+'/stat').read().split(') ',1)[1].split()[19]
with open(os.path.join(os.environ['HOME'],'launched-pid'),'w') as f: json.dump(dict(pid=p.pid,starttime=starttime),f)
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
  // A machine hub may inherit another factory's environment. Its selected
  // session's status must remain independent of that ambient identity.
  if (progressControl) state.env.CAS_FACTORY_SESSION = "hub-host-other-session";
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
// Real stores, Git and a live inert harness; the hub still projects all data.
// The fixture never fabricates /status, worker liveness or merge verdicts.
async function progress() {
  state.progressWorker = spawn("bash", ["-c", "exec -a codex sleep 3600"], {
    cwd: state.project, env: state.env, stdio: "ignore",
  });
  const worker = state.progressWorker;
  await new Promise((ok, fail) => { worker.once("spawn", ok); worker.once("error", fail); });
  const { stdout } = await run("python3", ["-c", `
import sqlite3,json,sys,os,subprocess,glob
from datetime import datetime,timezone
home,project,session,pid=sys.argv[1:]
now=datetime.now(timezone.utc).isoformat()
def git(*args): return subprocess.check_output(['git',*args],cwd=project,text=True).strip()
git('config','user.email','fixture@example.test');git('config','user.name','Fixture')
git('checkout','-b','epic/progress')
git('commit','--allow-empty','-m','base')
open(os.path.join(project,'delivery.txt'),'w').write('delivered\\n')
git('add','delivery.txt');git('commit','-m','integrated delivery')
anchor=git('rev-parse','HEAD')
account=os.path.join(home,'codex');rollouts=os.path.join(account,'sessions','2026','10','28')
os.makedirs(rollouts,exist_ok=True)
with open(os.path.join(rollouts,'rollout-progress-worker.jsonl'),'w') as f:
 f.write(json.dumps(dict(type='session_meta',payload=dict(id='progress-worker-id',cwd=project,source='cli')))+'\\n')
 f.write(json.dumps(dict(timestamp=now,type='event_msg',payload=dict(type='task_started',turn_id='progress-turn')))+'\\n')
c=sqlite3.connect(os.path.join(project,'.cas','cas.db'))
meta=dict(worker_cli='codex',clone_path=project,worker_account_dir=account)
start=int(open('/proc/'+pid+'/stat').read().split(') ',1)[1].split()[19])
c.execute('INSERT INTO agents(id,name,agent_type,role,status,pid,pid_starttime,factory_session,cc_session_id,registered_at,last_heartbeat,metadata,startup_confirmed) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,1)',('progress-worker-id','progress-worker','worker','worker','active',int(pid),start,session,'progress-worker-id',now,now,json.dumps(meta)))
for id,title,status,kind,assignee,deliverables in [
 ('cas-progress-epic','Progress fixture','in_progress','epic',None,{}),
 ('cas-progress-live','Live worker task','in_progress','task','progress-worker-id',{}),
 ('cas-progress-merged','Integrated delivery held by close gate','awaiting_merge','task',None,dict(factory_branch_anchor=anchor,merge_commit=anchor)),
 ('cas-progress-ready','Ready task','open','task',None,{})]:
 c.execute('INSERT INTO tasks(id,title,status,task_type,assignee,branch,deliverables,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)',(id,title,status,kind,assignee,'epic/progress' if kind=='epic' else None,json.dumps(deliverables),now,now))
 if kind!='epic': c.execute('INSERT INTO dependencies(from_id,to_id,dep_type,created_at) VALUES(?,?,?,?)',(id,'cas-progress-epic','parent_child',now))
c.commit()
for path in glob.glob(os.path.join(home,'.cas','sessions','*.json')):
 data=json.load(open(path))
 if data.get('name')==session:
  data['workers']=[dict(name='progress-worker',pid=int(pid),worktree_path=project)]
  data['epic_id']='cas-progress-epic'
  json.dump(data,open(path,'w'))
print(json.dumps(dict(worker_pid=int(pid),anchor=anchor,task='cas-progress-live',target='epic/progress',merged='cas-progress-merged')))
`, state.home, state.project, state.session, String(worker.pid)], { timeout: 10000 });
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
        case "/progress":
          return progress();
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
