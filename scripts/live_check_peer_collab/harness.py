#!/usr/bin/env python3
"""Scripted stand-in for the supervisor/worker harness of a factory pane (cas-daa0).

The live check starts a real `cas factory daemon` with `--supervisor-cli claude`
and puts a `claude` stub on PATH that runs this file. In the pane (stdin is a
TTY) it does what Claude Code does at startup, without an LLM:

- spawn `cas serve` (the `.mcp.json` server) and `cas hook SessionStart`
  as sibling children, so the MCP daemon adopts the factory identity
  (CAS_AGENT_NAME, CAS_AGENT_ROLE, CAS_SESSION_ID, CAS_FACTORY_SESSION);
- log everything the factory injects into the pane (delivered messages land
  there) to `<run dir>/<agent>.pane.log`;
- serve a Unix control socket at `<run dir>/<agent>.sock` that the runner uses
  to call MCP tools as this agent.

Outside a TTY (the factory's `claude --version` / `--help` probes, `cas init`)
it exits 0 at once.

Control protocol: one JSON request line, one JSON response line.
  {"op": "tool", "name": "coordination", "arguments": {...}}
      -> {"ok": true, "is_error": false, "text": "..."}
  {"op": "pane"} -> {"ok": true, "text": "<pane input so far>"}
  {"op": "ping"} -> {"ok": true, "agent": ..., "pid": ...}
  {"op": "exit"} -> stops `cas serve` and exits the pane.

`python3 harness.py ctl <socket> '<json>'` is the client the runner uses
locally and over ssh.
"""

import json
import os
import re
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path


def ctl(sock_path, request, timeout=120.0):
    deadline = time.monotonic() + 30
    while True:
        try:
            client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            client.settimeout(timeout)
            client.connect(sock_path)
            break
        except OSError:
            if time.monotonic() > deadline:
                raise
            time.sleep(0.5)
    with client:
        client.sendall(request.encode() + b"\n")
        data = b""
        while not data.endswith(b"\n"):
            chunk = client.recv(65536)
            if not chunk:
                break
            data += chunk
    return data.decode()


class McpClient:
    """Newline-delimited JSON-RPC over `cas serve`'s stdio."""

    def __init__(self, argv, log_path, env=None):
        self.log = open(log_path, "a", buffering=1)
        self.proc = subprocess.Popen(
            argv,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.log,
            text=True,
            bufsize=1,
        )
        self.next_id = 0
        self.pending = {}
        self.lock = threading.Lock()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                message = json.loads(line)
            except ValueError:
                self.log.write(f"[harness] non-JSON from cas serve: {line[:200]}\n")
                continue
            waiter = self.pending.get(message.get("id"))
            if waiter is not None and ("result" in message or "error" in message):
                waiter["reply"] = message
                waiter["event"].set()

    def request(self, method, params, timeout=120.0):
        with self.lock:
            self.next_id += 1
            request_id = self.next_id
            waiter = {"event": threading.Event(), "reply": None}
            self.pending[request_id] = waiter
            self.proc.stdin.write(
                json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
                + "\n"
            )
            self.proc.stdin.flush()
        if not waiter["event"].wait(timeout):
            raise TimeoutError(f"{method} timed out after {timeout}s")
        del self.pending[request_id]
        return waiter["reply"]

    def notify(self, method, params=None):
        with self.lock:
            message = {"jsonrpc": "2.0", "method": method}
            if params is not None:
                message["params"] = params
            self.proc.stdin.write(json.dumps(message) + "\n")
            self.proc.stdin.flush()

    def initialize(self):
        reply = self.request(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cas-live-check-harness", "version": "1"},
            },
        )
        if "error" in reply:
            raise RuntimeError(f"initialize failed: {reply['error']}")
        self.notify("notifications/initialized")

    def call_tool(self, name, arguments):
        reply = self.request("tools/call", {"name": name, "arguments": arguments})
        if "error" in reply:
            return {"ok": False, "is_error": True, "text": json.dumps(reply["error"])}
        result = reply.get("result", {})
        text = "\n".join(
            part.get("text", "") for part in result.get("content", []) if part.get("type") == "text"
        )
        return {"ok": True, "is_error": bool(result.get("isError")), "text": text}

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()


def run_pane():
    agent = os.environ.get("CAS_AGENT_NAME", "agent")
    run_dir = Path(os.environ.get("LIVE_CHECK_RUN_DIR") or Path(os.environ["HOME"]).parent)
    log_path = run_dir / f"{agent}.harness.log"
    pane_path = run_dir / f"{agent}.pane.log"
    sock_path = run_dir / f"{agent}.sock"
    cas = os.environ.get("LIVE_CHECK_CAS", "cas")
    log = open(log_path, "a", buffering=1)
    log.write(f"[harness] {time.strftime('%H:%M:%S')} pid={os.getpid()} argv={sys.argv[1:]}\n")

    # Everything the factory types into the pane: prompts and delivered messages.
    pane = open(pane_path, "a", buffering=1)

    def drain_stdin():
        fd = sys.stdin.fileno()
        while True:
            try:
                chunk = os.read(fd, 4096)
            except OSError:
                return
            if not chunk:
                return
            pane.write(chunk.decode("utf-8", "replace"))

    threading.Thread(target=drain_stdin, daemon=True).start()

    serve_env = dict(os.environ)
    if os.environ.get("LIVE_CHECK_SERVE_WITHOUT_SESSION_ID") == "1":
        # Diagnostic only (runner --workaround-eager-registration): without
        # CAS_SESSION_ID `cas serve` skips eager registration, so SessionStart
        # creates the agent and registers it in the cloud. A real harness
        # passes CAS_SESSION_ID to its MCP servers.
        serve_env.pop("CAS_SESSION_ID", None)
        log.write("[harness] DIAGNOSTIC: cas serve started without CAS_SESSION_ID\n")
    mcp = McpClient([cas, "serve"], log_path, env=serve_env)
    mcp.initialize()
    # Give `cas serve` time to bind its daemon socket so the hook notifies it.
    time.sleep(2)
    hook_input = {
        "session_id": os.environ.get("CAS_SESSION_ID", ""),
        "cwd": os.getcwd(),
        "hook_event_name": "SessionStart",
        "source": "startup",
        "transcript_path": str(run_dir / f"{agent}.transcript.jsonl"),
    }
    hook = subprocess.run(
        [cas, "hook", "SessionStart"],
        input=json.dumps(hook_input),
        capture_output=True,
        text=True,
        timeout=60,
    )
    log.write(f"[harness] SessionStart exit={hook.returncode}\n{hook.stderr[-2000:]}\n")

    if sock_path.exists():
        sock_path.unlink()
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(str(sock_path))
    server.listen(8)
    log.write(f"[harness] control socket {sock_path}\n")
    # The factory injects into a pane only once it has printed something
    # (cas-mux Pane::ready_for_injection: total_bytes_received > 0), as
    # Claude Code does when it draws its UI. Say we are ready.
    sys.stdout.write(f"cas live-check harness {agent} ready\r\n")
    sys.stdout.flush()
    stop = threading.Event()

    def handle(conn):
        with conn:
            data = b""
            while not data.endswith(b"\n"):
                chunk = conn.recv(65536)
                if not chunk:
                    return
                data += chunk
            try:
                request = json.loads(data)
                op = request.get("op")
                if op == "tool":
                    reply = mcp.call_tool(request["name"], request.get("arguments", {}))
                elif op == "pane":
                    pane.flush()
                    reply = {"ok": True, "text": pane_path.read_text(errors="replace")}
                elif op == "ping":
                    reply = {"ok": True, "agent": agent, "pid": os.getpid(), "serve_pid": mcp.proc.pid}
                elif op == "exit":
                    reply = {"ok": True}
                    stop.set()
                else:
                    reply = {"ok": False, "text": f"unknown op {op!r}"}
            except Exception as error:  # reported to the runner, never fatal
                reply = {"ok": False, "is_error": True, "text": f"{type(error).__name__}: {error}"}
            log.write(f"[harness] {request.get('op') if isinstance(request, dict) else '?'} -> {str(reply)[:300]}\n")
            conn.sendall(json.dumps(reply).encode() + b"\n")

    server.settimeout(1.0)
    while not stop.is_set():
        try:
            conn, _ = server.accept()
        except socket.timeout:
            if mcp.proc.poll() is not None:
                log.write(f"[harness] cas serve exited {mcp.proc.returncode}\n")
                break
            continue
        threading.Thread(target=handle, args=(conn,), daemon=True).start()
    time.sleep(0.5)
    mcp.close()
    server.close()
    if sock_path.exists():
        sock_path.unlink()
    log.write("[harness] exit\n")


#
# Machine-side subcommands the runner calls (locally or over ssh). Every path
# they touch is under the run directory; nothing reads or writes the real
# ~/.cas or the installed cas.
#


def scratch_env(run_dir):
    """The clean environment every scratch `cas` runs with: no inherited CAS_*
    identity, HOME inside the run directory."""
    run_dir = Path(run_dir)
    python_dir = str(Path(sys.executable).parent)
    return {
        "HOME": str(run_dir / "home"),
        "PATH": ":".join([str(run_dir / "bin"), python_dir, "/usr/bin", "/bin"]),
        "TERM": "xterm-256color",
        "LIVE_CHECK_RUN_DIR": str(run_dir),
        "LIVE_CHECK_CAS": str(run_dir / "bin" / "cas"),
        "TMPDIR": str(run_dir / "tmp"),
    }


def cmd_setup(args):
    """Create the scratch home, bin, repo and Cassy project for one machine.
    The device token arrives on stdin and is written 0600; it is never echoed."""
    run_dir = Path(args.run_dir)
    token = sys.stdin.read().strip()
    if not token:
        raise SystemExit("setup: no device token on stdin")
    for sub in ("home", "bin", "repo", "tmp"):
        (run_dir / sub).mkdir(parents=True, exist_ok=True)
    cas_link = run_dir / "bin" / "cas"
    if cas_link.is_symlink() or cas_link.exists():
        cas_link.unlink()
    cas_link.symlink_to(args.binary)
    claude = run_dir / "bin" / "claude"
    claude.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{Path(__file__).resolve()}" "$@"\n')
    claude.chmod(0o755)
    env = scratch_env(run_dir)
    repo = run_dir / "repo"

    def run(argv, **kw):
        result = subprocess.run(argv, cwd=repo, env=env, capture_output=True, text=True, **kw)
        if result.returncode != 0:
            raise SystemExit(f"setup: {argv[:3]} exited {result.returncode}: {result.stderr[-800:]}")
        return result

    run(["git", "init", "-q", "-b", "main", "."])
    run(["git", "-c", "user.email=live-check@invalid", "-c", "user.name=live-check",
         "commit", "-q", "--allow-empty", "-m", "live-check scratch repo"])
    run(["git", "remote", "add", "origin", args.remote])
    run([str(cas_link), "init", "--yes"], stdin=subprocess.DEVNULL, timeout=120)
    with open(repo / ".cas" / "config.toml", "a") as config:
        config.write(f'\n[project]\ncanonical_id = "{args.canonical_id}"\n')
    cloud = repo / ".cas" / "cloud.json"
    fd = os.open(cloud, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as handle:
        json.dump({"endpoint": args.endpoint, "token": token}, handle)
    version = run([str(cas_link), "--version"]).stdout.strip()
    print(json.dumps({"ok": True, "run_dir": str(run_dir), "binary_version": version}))


def cmd_cloud_token(args):
    """Rewrite the scratch project's cloud.json token (0600), keeping the
    endpoint. The token arrives on stdin; `--invalid` writes a random bad
    token instead (to take this machine's mailbox and presence offline)."""
    import secrets as _secrets
    cloud = Path(args.run_dir) / "repo" / ".cas" / "cloud.json"
    config = json.loads(cloud.read_text())
    config["token"] = ("invalid-" + _secrets.token_hex(16)) if args.invalid else sys.stdin.read().strip()
    if not config["token"]:
        raise SystemExit("cloud-token: no token on stdin")
    fd = os.open(cloud, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as handle:
        json.dump(config, handle)
    print(json.dumps({"ok": True, "valid": not args.invalid}))


def cmd_agent_ids(args):
    try:
        db = _db(args.run_dir)
        rows = db.execute("SELECT id, name, role, status FROM agents").fetchall()
    except Exception as error:
        print(json.dumps({"ok": False, "error": str(error), "agents": []}))
        return
    print(json.dumps({"ok": True, "agents": [dict(zip(("id", "name", "role", "status"), r)) for r in rows]}))


def cmd_launch(args):
    """Start `cas factory daemon` detached in its own session; print its pid."""
    run_dir = Path(args.run_dir)
    env = scratch_env(run_dir)
    if args.serve_without_session_id:
        env["LIVE_CHECK_SERVE_WITHOUT_SESSION_ID"] = "1"
    argv = [
        str(run_dir / "bin" / "cas"), "factory", "daemon",
        "--session", args.session,
        "--cwd", str(run_dir / "repo"),
        "--workers", str(len(args.worker_name)),
        "--no-worktrees",
        "--supervisor-cli", "claude",
        "--worker-cli", "claude",
        "--foreground",
        "--no-phone-home",
        "--supervisor-name", args.supervisor_name,
    ]
    for name in args.worker_name:
        argv += ["--worker-name", name]
    log = open(run_dir / f"daemon-{args.session}.log", "ab")
    proc = subprocess.Popen(
        argv, cwd=run_dir / "repo", env=env, stdin=subprocess.DEVNULL,
        stdout=log, stderr=log, start_new_session=True,
    )
    (run_dir / f"daemon-{args.session}.pid").write_text(str(proc.pid))
    print(json.dumps({"ok": True, "pid": proc.pid}))


def process_table():
    """[(pid, cwd, command)] for every process this user can see."""
    rows = []
    if Path("/proc").is_dir():
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                cwd = os.readlink(entry / "cwd")
                command = (entry / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
            except OSError:
                continue
            rows.append((int(entry.name), cwd, command))
        return rows
    commands = {}
    listing = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True, text=True).stdout
    for line in listing.splitlines():
        pid, _, command = line.strip().partition(" ")
        if pid.isdigit():
            commands[int(pid)] = command
    cwds = {}
    lsof = subprocess.run(["lsof", "-a", "-d", "cwd", "-Fpn"], capture_output=True, text=True).stdout
    current = None
    for line in lsof.splitlines():
        if line.startswith("p"):
            current = int(line[1:])
        elif line.startswith("n") and current is not None:
            cwds[current] = line[1:]
    for pid, command in commands.items():
        rows.append((pid, cwds.get(pid, ""), command))
    return rows


def run_dir_processes(run_dir):
    run_dir = str(Path(run_dir).resolve())
    me = os.getpid()
    found = []
    for pid, cwd, command in process_table():
        if pid == me or pid == os.getppid():
            continue
        cwd = cwd.removesuffix(" (deleted)")
        if cwd == run_dir or cwd.startswith(run_dir + "/") or run_dir in command:
            if "harness.py sweep" in command:
                continue
            found.append({"pid": pid, "cwd": cwd, "command": command[:200]})
    return found


def cmd_sweep(args):
    """List (and with --kill, terminate) every process whose cwd or command
    line is inside the run directory."""
    import signal

    found = run_dir_processes(args.run_dir)
    if args.kill and found:
        for row in found:
            try:
                os.kill(row["pid"], signal.SIGTERM)
            except OSError:
                pass
        time.sleep(3)
        for row in run_dir_processes(args.run_dir):
            try:
                os.kill(row["pid"], signal.SIGKILL)
            except OSError:
                pass
        time.sleep(1)
    left = run_dir_processes(args.run_dir)
    print(json.dumps({"ok": not left, "found": found, "left": left}))


def cmd_remove(args):
    """Delete the run directory (after a sweep) and report whether it is gone."""
    import shutil

    run_dir = Path(args.run_dir).resolve()
    if not any(part.startswith("live-check-") for part in run_dir.parts):
        raise SystemExit(f"remove: refusing {run_dir}: not a live-check scratch directory")
    if run_dir.exists():
        shutil.rmtree(run_dir, ignore_errors=True)
    print(json.dumps({"ok": not run_dir.exists(), "path": str(run_dir)}))


def _db(run_dir, readonly=True):
    import sqlite3

    path = Path(run_dir) / "repo" / ".cas" / "cas.db"
    if readonly:
        return sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=30)
    return sqlite3.connect(path, timeout=30)


def cmd_task_export(args):
    """Task rows (and the dependency rows among them) for --task-id ids."""
    import sqlite3
    db = _db(args.run_dir)
    db.row_factory = sqlite3.Row
    ids = [i for i in args.task_id.split(",") if i]
    marks = ",".join("?" for _ in ids)
    tasks = [dict(r) for r in db.execute(f"SELECT * FROM tasks WHERE id IN ({marks})", ids)]
    deps = [dict(r) for r in db.execute(
        f"SELECT * FROM dependencies WHERE from_id IN ({marks}) AND to_id IN ({marks})", ids + ids)]
    found = {t["id"] for t in tasks}
    print(json.dumps({"ok": found == set(ids), "missing": sorted(set(ids) - found),
                      "tasks": tasks, "dependencies": deps,
                      # Back-compat: a single exported task.
                      "task": tasks[0] if len(tasks) == 1 else None}))


def _insert_rows(db, table, rows):
    columns = [row[1] for row in db.execute(f"PRAGMA table_info({table})")]
    for row in rows:
        keep = [column for column in columns if column in row]
        db.execute(
            f"INSERT OR REPLACE INTO {table} ({', '.join(keep)}) VALUES ({', '.join('?' for _ in keep)})",
            [row[column] for column in keep],
        )
    return len(rows)


def cmd_task_import(args):
    """Insert task rows (and their dependency rows) exported from the other
    machine: the stand-in cloud has no sync, so the runner replicates the
    shared tasks. Accepts {"tasks": [...], "dependencies": [...]} or one task."""
    payload = json.loads(sys.stdin.read())
    tasks = payload.get("tasks") if isinstance(payload, dict) and "tasks" in payload else [payload]
    deps = payload.get("dependencies", []) if isinstance(payload, dict) else []
    db = _db(args.run_dir, readonly=False)
    count = _insert_rows(db, "tasks", tasks)
    dep_count = _insert_rows(db, "dependencies", deps) if deps else 0
    db.commit()
    print(json.dumps({"ok": True, "tasks": count, "dependencies": dep_count}))


# cas-937a5 (F03): what a binary can do, read from the binary itself. Each
# feature is present when every marker (a user-facing string literal of its
# client code) is in the executable, so a step can never be labelled active
# for a build that lacks its code, or pending for one that has it.
FEATURE_MARKERS = {
    "cas-8ea7": [b"Claims visible (team project)"],
    "cas-559c": [b"Claims not visible (different account)"],
    "cas-0078": [b"cloud claim renewal failed; claiming again"],
    "cas-a19b": [b"through Cassy Cloud: no token", b"is not visible to this account (or does not exist)"],
    "cas-4736": [b"accepted epic split", b"decision must be accept, reject or cancel"],
    "cas-13c7": [b"Peer mailbox: not receiving"],
    "cas-f707": [b"Cloud presence: registered"],
}


def probe_binary(binary):
    import mmap
    import re as _re

    version = subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=60)
    text = version.stdout.strip()
    match = _re.search(r"\(([0-9a-f]{7,40})\b", text)
    features = {}
    with open(binary, "rb") as handle:
        with mmap.mmap(handle.fileno(), 0, access=mmap.ACCESS_READ) as data:
            for feature, markers in FEATURE_MARKERS.items():
                features[feature] = all(data.find(marker) != -1 for marker in markers)
    return {"ok": version.returncode == 0, "version": text, "commit": match.group(1) if match else None,
            "features": features}


def cmd_probe(args):
    print(json.dumps(probe_binary(os.path.expanduser(args.binary))))


def cmd_queue_find(args):
    """Prompt-queue rows on this machine whose body or summary contains
    `--token` (the peer mailbox enqueues delivered peer messages here)."""
    db = _db(args.run_dir)
    columns = [row[1] for row in db.execute("PRAGMA table_info(prompt_queue)")]
    wanted = [c for c in ("id", "source", "target", "prompt", "summary", "created_at", "factory_session",
                          "highest_stage", "processed_at", "dedupe_key") if c in columns]
    rows = db.execute(
        f"SELECT {', '.join(wanted)} FROM prompt_queue WHERE prompt LIKE ? OR summary LIKE ? ORDER BY id",
        (f"%{args.token}%", f"%{args.token}%"),
    ).fetchall()
    import hashlib
    result = []
    for row in rows:
        record = dict(zip(wanted, row))
        prompt = record.pop("prompt", "")
        # Only over-cap probes define a body digest: their first and last
        # complete lines are the unique probe token. Routing may contain blank
        # lines, so it cannot delimit the measured body.
        if args.token.startswith("live-overcap-"):
            markers = list(re.finditer(r"(?m)^" + re.escape(args.token) + r"$", prompt))
            if len(markers) == 2:
                body = prompt[markers[0].start():markers[1].end()].encode()
                record["peer_body_sha256"] = hashlib.sha256(body).hexdigest()
                record["peer_body_bytes"] = len(body)
            else:
                record["peer_body_error"] = "missing or ambiguous over-cap probe markers"
        result.append(record)
    print(json.dumps({"ok": True, "rows": result}))


def cmd_spawn_count(args):
    db = _db(args.run_dir)
    total = db.execute("SELECT COUNT(*) FROM spawn_queue").fetchone()[0]
    print(json.dumps({"ok": True, "spawn_queue_rows": total}))


def cmd_installed_cas(_args):
    """`cas --version` and a fingerprint of the operator's installed cas."""
    import hashlib
    import shutil

    login_path = os.environ.get("PATH", "") + ":" + str(Path.home() / ".local" / "bin")
    path = shutil.which("cas", path=login_path)
    if not path:
        print(json.dumps({"ok": True, "path": None}))
        return
    real = os.path.realpath(path)
    digest = hashlib.sha256(Path(real).read_bytes()).hexdigest()
    stat = os.stat(real)
    version = subprocess.run([real, "--version"], capture_output=True, text=True, timeout=60).stdout.strip()
    print(json.dumps({
        "ok": True, "path": path, "realpath": real, "version": version,
        "sha256": digest, "size": stat.st_size, "mtime": stat.st_mtime,
    }))


def machine_main(argv):
    import argparse

    parser = argparse.ArgumentParser(prog="harness.py")
    sub = parser.add_subparsers(dest="command", required=True)
    setup = sub.add_parser("setup")
    setup.add_argument("--run-dir", required=True)
    setup.add_argument("--binary", required=True)
    setup.add_argument("--remote", required=True)
    setup.add_argument("--canonical-id", required=True)
    setup.add_argument("--endpoint", required=True)
    setup.set_defaults(func=cmd_setup)
    launch = sub.add_parser("launch")
    launch.add_argument("--run-dir", required=True)
    launch.add_argument("--session", required=True)
    launch.add_argument("--supervisor-name", required=True)
    launch.add_argument("--worker-name", action="append", default=[])
    launch.add_argument("--serve-without-session-id", action="store_true")
    launch.set_defaults(func=cmd_launch)
    for name, func in (("sweep", cmd_sweep), ("remove", cmd_remove), ("spawn-count", cmd_spawn_count)):
        command = sub.add_parser(name)
        command.add_argument("--run-dir", required=True)
        if name == "sweep":
            command.add_argument("--kill", action="store_true")
        command.set_defaults(func=func)
    token = sub.add_parser("cloud-token")
    token.add_argument("--run-dir", required=True)
    token.add_argument("--invalid", action="store_true")
    token.set_defaults(func=cmd_cloud_token)
    ids = sub.add_parser("agent-ids")
    ids.add_argument("--run-dir", required=True)
    ids.set_defaults(func=cmd_agent_ids)
    probe = sub.add_parser("probe")
    probe.add_argument("--binary", required=True)
    probe.set_defaults(func=cmd_probe)
    export = sub.add_parser("task-export")
    export.add_argument("--run-dir", required=True)
    export.add_argument("--task-id", required=True)
    export.set_defaults(func=cmd_task_export)
    queue_find = sub.add_parser("queue-find")
    queue_find.add_argument("--run-dir", required=True)
    queue_find.add_argument("--token", required=True)
    queue_find.set_defaults(func=cmd_queue_find)
    task_import = sub.add_parser("task-import")
    task_import.add_argument("--run-dir", required=True)
    task_import.set_defaults(func=cmd_task_import)
    sub.add_parser("installed-cas").set_defaults(func=cmd_installed_cas)
    args = parser.parse_args(argv)
    args.func(args)
    return 0


MACHINE_COMMANDS = {
    "setup", "launch", "sweep", "remove", "spawn-count", "task-export", "task-import", "installed-cas",
    "queue-find", "probe", "cloud-token", "agent-ids",
}


def main():
    if len(sys.argv) >= 4 and sys.argv[1] == "ctl":
        sys.stdout.write(ctl(sys.argv[2], sys.argv[3]))
        return 0
    if len(sys.argv) >= 2 and sys.argv[1] in MACHINE_COMMANDS:
        return machine_main(sys.argv[1:])
    if sys.argv[1:] == ["--help"]:
        # The scripted harness accepts these arguments. Native worker
        # isolation probes must see them before it will launch the stub.
        print("Scripted Claude harness: --mcp-config --strict-mcp-config --permission-mode --allowedTools --disallowedTools")
        return 0
    if not sys.stdin.isatty():
        # `claude --version` and `cas init` probes.
        return 0
    run_pane()
    return 0


if __name__ == "__main__":
    sys.exit(main())
