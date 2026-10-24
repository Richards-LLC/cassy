#!/usr/bin/env python3
"""Many-agent load harness for Cassy (cas-98b24, GH #1165).

Runs N real `cas serve` MCP processes against a scratch copy of a project
database. Each process carries a mixed agent workload: task show, list,
notes and update, coordination message and inbox, memory remember, and
search. Alongside them run an optional headless `cas factory daemon
--workers 0` and the boot-time background work those processes start on
their own. While the run is live, the harness samples:

* per-call latency for every tool/action (p50/p99/max) and calls that reach
  the 55 s MCP deadline;
* SQLite busy/write-lock retry warnings from the scratch `.cas/logs`;
* waits on `task-sync-intents.lock` (and its per-task stripes) from
  `/proc/locks`, as in the GH #1165 field evidence;
* the factory daemon loop: `.cas/factory-daemon/<session>.loop.json` plus the
  loop thread's kernel wait channel;
* with `--synthetic-writers` (factory loop v2 C6, cas-9db0b), worker panes
  that print at GH #1210's codex rates (38, 63 and 236 writes/s), and a
  key-to-echo probe that types into the quiet supervisor pane over the
  daemon's GUI socket and times the echo frame;
* the host's /proc/loadavg, so a receipt taken on a loaded host says so.

It writes a JSON receipt and a Markdown summary, and asserts the SLOs. The
exit status is 0 when every SLO holds, 1 when one is violated and 2 when the
run cannot be completed.

It never touches the live database. The database is copied with SQLite's
online backup into the scratch root, and every child runs with a scratch
HOME, a scratch CAS_ROOT and a cloud endpoint on a closed local port.

Usage (see cas-cli/docs/MANY_AGENT_LOAD_HARNESS.md):

    python3 -I scripts/many-agent-load.py \
        --cas-bin ~/.local/bin/cas --label baseline-0d670c33c \
        --agents 16 --duration 300 \
        --source-db ~/Petrastella/cassy/.cas/cas.db \
        --scratch /mnt/rewind/cas/scratch/load-harness \
        --out /path/to/artifacts/cas-98b24

Stdlib only; Linux only (it reads /proc).
"""

from __future__ import annotations

import argparse
import collections
import datetime as dt
import glob
import hashlib
import json
import os
import random
import re
import shutil
import signal
import sqlite3
import statistics
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path

# Service-level objectives (cas-98b24).
SLO_P99_SECS = 2.0  # MCP task/coordination p99
MCP_DEADLINE_SECS = 55.0  # the cas serve tool deadline
SLO_LOOP_PASS_P99_MS = 100.0  # factory daemon loop pass p99
HEADLESS_SLEEP_SECS = 0.5  # daemon idle sleep with no clients attached
CALL_TIMEOUT_SECS = 120.0  # harness-side give-up for one call
LOCK_WAIT_REPORT_SECS = 0.4  # matches the GH #1165 threshold
SLO_KEY_TO_ECHO_P99_MS = 50.0  # factory loop v2 §7 key -> echo p99
ECHO_TIMEOUT_SECS = 5.0  # a probe without an echo by then counts as this slow
# Factory loop v2 §7 receipt names (spec 2026-10-15-factory-loop-v2.md).
RECEIPT_CONTROL_PASS = "flv2_control_pass_p99_under_100ms_with_16_agents"
RECEIPT_KEY_TO_ECHO = "flv2_key_to_echo_p99_under_50ms_under_load"
# Keys the probe types; none of them appears in a kill-line echo.
PROBE_KEYS = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ"
KILL_LINE = 0x15  # ^U, so the quiet pane's line never fills

# Every child process, so an interrupted run leaves nothing behind.
CHILDREN: list[subprocess.Popen] = []

BUSY_PATTERNS = re.compile(
    r"SQLite busy|write lock held|database busy|database is locked|retrying after backoff",
    re.IGNORECASE,
)


def now() -> float:
    return time.monotonic()


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def percentile(values: list[float], pct: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    rank = max(0, min(len(ordered) - 1, int(round(pct / 100.0 * len(ordered) + 0.5)) - 1))
    return ordered[rank]


# ---------------------------------------------------------------------------
# Scratch environment
# ---------------------------------------------------------------------------


def copy_database(source_db: Path, cache_dir: Path, refresh: bool) -> Path:
    """Consistent snapshot of `source_db` via the SQLite online backup API.

    The live database is opened read-only; one backup step holds a WAL read
    snapshot, which never blocks the live writers.
    """
    cache_dir.mkdir(parents=True, exist_ok=True)
    cached = cache_dir / "cas.db"
    if cached.exists() and not refresh:
        return cached
    tmp = cache_dir / "cas.db.partial"
    if tmp.exists():
        tmp.unlink()
    src = sqlite3.connect(f"file:{source_db}?mode=ro", uri=True)
    dst = sqlite3.connect(str(tmp))
    src.backup(dst)
    dst.close()
    src.close()
    tmp.rename(cached)
    return cached


def export_project(repo: Path, rev: str, project: Path) -> None:
    """A git checkout of `repo` at `rev` (source for the boot code index)."""
    project.mkdir(parents=True, exist_ok=True)
    archive = subprocess.run(
        ["git", "-C", str(repo), "archive", "--format=tar", rev],
        check=True,
        capture_output=True,
    ).stdout
    subprocess.run(["tar", "-x", "-C", str(project)], input=archive, check=True)
    git_env = dict(os.environ, GIT_AUTHOR_NAME="load", GIT_AUTHOR_EMAIL="load@localhost",
                   GIT_COMMITTER_NAME="load", GIT_COMMITTER_EMAIL="load@localhost")
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=project, check=True, env=git_env)
    subprocess.run(["git", "add", "-A"], cwd=project, check=True, env=git_env)
    subprocess.run(["git", "commit", "-q", "-m", "load harness snapshot"], cwd=project,
                   check=True, env=git_env)


def prepare_run(args: argparse.Namespace, run_dir: Path) -> dict:
    # The daemon binds unix sockets under $HOME/.cas named after the session,
    # and sun_path is limited to 108 bytes: keep both short.
    token = uuid.uuid4().hex[:6]
    home = run_dir / "h"
    session = f"lh{token}"
    socket = home / ".cas" / f"factory-{session}.gui.sock"
    if len(str(socket)) >= 104:
        raise SystemExit(f"scratch path too long for the daemon's unix sockets: {socket}")
    project = run_dir / "project"
    cas_dir = project / ".cas"
    fake_bin = run_dir / "bin"
    for path in (home / ".cas", fake_bin):
        path.mkdir(parents=True, exist_ok=True)

    if args.project_src:
        export_project(Path(args.project_src), args.project_rev, project)
    else:
        project.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "init", "-q", "-b", "main"], cwd=project, check=True)
    cas_dir.mkdir(parents=True, exist_ok=True)

    if args.source_db:
        cached = copy_database(Path(args.source_db).expanduser(), Path(args.scratch) / "db-cache",
                               args.refresh_db)
        shutil.copyfile(cached, cas_dir / "cas.db")
    if args.source_config and Path(args.source_config).expanduser().exists():
        shutil.copyfile(Path(args.source_config).expanduser(), cas_dir / "config.toml")
    if args.code_index is not None:
        set_code_index(cas_dir / "config.toml", args.code_index)

    # Logged in, as the field fleet was: the syncing task store (and its
    # task-sync-intents.lock) is only in play for a logged-in project. The
    # endpoint is a closed local port, so no request leaves the host.
    cloud = {"endpoint": args.cloud_endpoint, "token": "cas-load-harness-not-a-token"}
    for path in (cas_dir / "cloud.json", home / ".cas" / "cloud.json"):
        path.write_text(json.dumps(cloud))

    # The daemon probes the supervisor CLI (`--help`, `--version`) and then
    # launches it into a PTY. Answer the probes with the real CLI's text when
    # it is installed, and run an inert process instead of an agent.
    for name in ("claude", "codex", "grok", "opencode"):
        probes = {}
        real = shutil.which(name)
        for flag in ("--help", "--version"):
            text = f"{name} (cas load harness stub)\n"
            if real:
                try:
                    done = subprocess.run([real, flag], capture_output=True, text=True, timeout=20,
                                          stdin=subprocess.DEVNULL)
                    text = done.stdout or text
                except (OSError, subprocess.TimeoutExpired):
                    pass
            probe_file = fake_bin / f".{name}{flag}"
            probe_file.write_text(text)
            probes[flag] = probe_file
        stub = fake_bin / name
        stub.write_text(stub_script(probes["--help"], probes["--version"], fake_bin,
                                    args.synthetic_writers))
        stub.chmod(0o755)
    if args.synthetic_writers:
        (fake_bin / "writer-rates").write_text(
            "".join(f"{rate:g}\n" for rate in args.synthetic_writers))
        (fake_bin / "writer-slots").mkdir(exist_ok=True)
        (fake_bin / "synthetic-writer.py").write_text(SYNTHETIC_WRITER)

    return {
        "home": home,
        "project": project,
        "cas_dir": cas_dir,
        "fake_bin": fake_bin,
        "session": session,
    }


# A codex-like status line rewritten in place at a fixed rate (GH #1210).
# Each write carries its sequence number, so the harness can count the
# writes that reached it.
SYNTHETIC_WRITER = r"""
import os, sys, time
rates_file, slots, label = sys.argv[1], sys.argv[2], sys.argv[3]
# Claim the first free rate with an exclusive create, and log the claim
# (or its absence) so a receipt can show which process wrote at which rate.
rate = None
for index, line in enumerate(open(rates_file).read().split()):
    try:
        os.close(os.open(os.path.join(slots, str(index)), os.O_CREAT | os.O_EXCL | os.O_WRONLY))
    except FileExistsError:
        continue
    rate = float(line)
    break
with open(os.path.join(slots, "claims.log"), "a") as log:
    log.write("%d %d %s %s %s\n" % (os.getpid(), os.getppid(), label, rate, os.ttyname(0)))
if rate is None:
    while True:
        time.sleep(3600)
interval, out = 1.0 / rate, sys.stdout.buffer
spinner = "|/-\\"
n, due = 0, time.monotonic()
while True:
    n += 1
    out.write(("\r\x1b[2K%s %s write %d (%g/s)" % (spinner[n % 4], label, n, rate)).encode())
    if n % 200 == 0:
        out.write(b"\r\n")
    out.flush()
    due += interval
    pause = due - time.monotonic()
    if pause > 0:
        time.sleep(pause)
    elif pause < -1.0:
        due = time.monotonic()
"""


def stub_script(help_file: Path, version_file: Path, fake_bin: Path,
                writer_rates: list[float] | None) -> str:
    """The stub agent CLI the daemon launches.

    It answers the daemon's `--help` and `--version` probes. A worker pane
    (a terminal on stdin and `CAS_AGENT_ROLE=worker`) claims the next free
    synthetic writer rate, when the run has any, and prints at that rate.
    Every other pane, the supervisor's included, sleeps: its terminal still
    echoes typed keys, which the key-to-echo probe times.
    """
    lines = [
        "#!/bin/sh",
        "for arg in \"$@\"; do",
        f"  case \"$arg\" in --help|-h) cat '{help_file}'; exit 0;;",
        f"    --version|-v) cat '{version_file}'; exit 0;; esac",
        "done",
    ]
    if writer_rates:
        lines += [
            "if [ -t 0 ] && [ \"$CAS_AGENT_ROLE\" = worker ]; then",
            f"  exec python3 -I '{fake_bin / 'synthetic-writer.py'}' '{fake_bin / 'writer-rates'}' "
            f"'{fake_bin / 'writer-slots'}' \"${{CAS_AGENT_NAME:-worker}}\"",
            "fi",
        ]
    lines.append("exec sleep 100000")
    return "\n".join(lines) + "\n"


def set_code_index(config: Path, enabled: bool) -> None:
    """Set `[code] enabled` in the scratch copy's config.toml."""
    text = config.read_text() if config.exists() else ""
    value = "true" if enabled else "false"
    section = re.search(r"(?ms)^\[code\]\s*$(.*?)(?=^\[|\Z)", text)
    if section is None:
        text = text.rstrip("\n") + f"\n\n[code]\nenabled = {value}\n"
    elif re.search(r"(?m)^enabled\s*=", section.group(1)):
        body = re.sub(r"(?m)^enabled\s*=.*$", f"enabled = {value}", section.group(1), count=1)
        text = text[:section.start(1)] + body + text[section.end(1):]
    else:
        text = text[:section.end(0)] + f"enabled = {value}\n" + text[section.end(0):]
    config.write_text(text)


def child_env(args: argparse.Namespace, env: dict, extra: dict) -> dict:
    base = {
        "PATH": f"{env['fake_bin']}:{os.environ.get('PATH', '/usr/bin:/bin')}",
        "HOME": str(env["home"]),
        "LANG": os.environ.get("LANG", "C.UTF-8"),
        "CAS_ROOT": str(env["cas_dir"]),
        "CAS_CLOUD_ENDPOINT": args.cloud_endpoint,
        "CAS_USER_CLOUD_JSON": str(env["home"] / ".cas" / "cloud.json"),
        "CAS_FACTORY_MODE": "1",
        "CAS_FACTORY_SESSION": env["session"],
        "CAS_SUPERVISOR_NAME": "load-supervisor",
        "CAS_FACTORY_SUPERVISOR_CLI": "claude",
        "CAS_FACTORY_WORKER_CLI": "claude",
        "RUST_LOG": os.environ.get("RUST_LOG", "warn"),
    }
    base.update(extra)
    return base


# ---------------------------------------------------------------------------
# MCP stdio client
# ---------------------------------------------------------------------------


class McpClient:
    """Newline-delimited JSON-RPC over a `cas serve` child's stdio."""

    def __init__(self, cas_bin: str, cwd: Path, env: dict, stderr_path: Path):
        self._stderr = open(stderr_path, "ab")
        self.proc = subprocess.Popen(
            [cas_bin, "serve"],
            cwd=cwd,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr,
        )
        CHILDREN.append(self.proc)
        self._next_id = 0
        self._lock = threading.Lock()
        self._pending: dict[int, dict] = {}
        self._cond = threading.Condition(self._lock)
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    def _send(self, message: dict) -> None:
        data = (json.dumps(message) + "\n").encode()
        self.proc.stdin.write(data)
        self.proc.stdin.flush()

    def _read_loop(self) -> None:
        for raw in self.proc.stdout:
            try:
                message = json.loads(raw)
            except ValueError:
                continue
            if "method" in message and "id" in message:
                # A server-to-client request (roots/list, ping, ...).
                reply = {"jsonrpc": "2.0", "id": message["id"]}
                if message["method"] == "roots/list":
                    reply["result"] = {"roots": []}
                elif message["method"] == "ping":
                    reply["result"] = {}
                else:
                    reply["error"] = {"code": -32601, "message": "not supported"}
                try:
                    with self._lock:
                        self._send(reply)
                except OSError:
                    pass
                continue
            if "id" in message:
                with self._cond:
                    self._pending[message["id"]] = message
                    self._cond.notify_all()
        with self._cond:
            self._cond.notify_all()

    def request(self, method: str, params: dict, timeout: float) -> dict:
        with self._cond:
            self._next_id += 1
            request_id = self._next_id
            self._send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
            deadline = now() + timeout
            while request_id not in self._pending:
                remaining = deadline - now()
                if remaining <= 0:
                    raise TimeoutError(f"{method} timed out after {timeout:.0f}s")
                if self.proc.poll() is not None:
                    raise RuntimeError(f"cas serve exited ({self.proc.returncode})")
                self._cond.wait(min(remaining, 1.0))
            return self._pending.pop(request_id)

    def notify(self, method: str, params: dict) -> None:
        with self._lock:
            self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def initialize(self, timeout: float) -> None:
        self.request(
            "initialize",
            {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "cas-load-harness", "version": "1"},
            },
            timeout,
        )
        self.notify("notifications/initialized", {})

    def call_tool(self, name: str, arguments: dict, timeout: float) -> tuple[bool, str]:
        response = self.request("tools/call", {"name": name, "arguments": arguments}, timeout)
        if "error" in response:
            return False, json.dumps(response["error"])[:400]
        result = response.get("result", {})
        text = " ".join(
            item.get("text", "") for item in result.get("content", []) if isinstance(item, dict)
        )
        return not result.get("isError", False), text[:400]

    def close(self) -> None:
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        self._stderr.close()


# ---------------------------------------------------------------------------
# Agent workload
# ---------------------------------------------------------------------------

# (weight, tool, action). Weights approximate a working agent's mix: reads
# dominate, each agent writes notes/updates on its own task and messages a
# peer now and then.
WORKLOAD = [
    (20, "task", "show"),
    (8, "task", "list"),
    (4, "task", "ready"),
    (4, "task", "mine"),
    (8, "task", "notes"),
    (4, "task", "update"),
    (6, "coordination", "message"),
    (12, "coordination", "inbox_poll"),
    (4, "coordination", "heartbeat"),
    (3, "memory", "remember"),
    (6, "search", "search"),
]


MEMORY_WORDS = (
    "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike "
    "november oscar papa quebec romeo sierra tango uniform victor whiskey xray yankee "
    "zulu amber basalt cobalt dune ember fjord granite harbor iris jasper kelp lagoon "
    "meadow nickel opal prairie quartz river saffron tundra umber violet willow zinc"
).split()


def agent_name(index: int) -> str:
    return "load-supervisor" if index == 0 else f"load-agent-{index:02d}"


class Agent(threading.Thread):
    def __init__(self, index: int, args, env, task_pool, peers, results, results_lock, stop_at):
        super().__init__(daemon=True, name=f"agent-{index}")
        self.index = index
        # Agent 0 is the factory supervisor (peer messages need one registered
        # in the session); the rest are workers.
        self.name_ = agent_name(index)
        self.role = "supervisor" if index == 0 else "worker"
        self.args = args
        self.env = env
        self.task_pool = task_pool
        self.peers = peers
        self.results = results
        self.results_lock = results_lock
        self.stop_at = stop_at
        self.rng = random.Random(args.seed * 1000 + index)
        self.own_task: str | None = None
        self.client: McpClient | None = None
        self.boot_secs: float | None = None
        self.boot_failures: list[str] = []
        self.error: str | None = None

    def record(self, tool, action, started, latency, ok, detail):
        with self.results_lock:
            self.results.append({
                "agent": self.name_,
                "tool": tool,
                "action": action,
                "t": round(started - self.args.t0, 3),
                "latency": round(latency, 4),
                "ok": ok,
                "detail": "" if ok else detail[:200],
            })

    def boot(self, env: dict, log_dir: Path) -> None:
        """Start `cas serve` and create the agent's task.

        A serve that exits during boot is restarted, up to `--boot-retries`
        times, so a receipt keeps its configured agent count. Each failure is
        recorded in the receipt (`agent_boot_failures`), so a boot bug stays
        visible instead of silently thinning the load. Once create is
        acknowledged, a later exit fails the run without creating another task.
        """
        attempts = 1 + max(0, self.args.boot_retries)
        create_acknowledged = False
        for attempt in range(attempts):
            try:
                started = now()
                self.client = McpClient(self.args.cas_bin, self.env["project"], env,
                                        log_dir / f"{self.name_}.stderr")
                self.client.initialize(CALL_TIMEOUT_SECS)
                self.boot_secs = now() - started
                ok, text = self.timed_call("task", "create", {
                    "action": "create",
                    "title": f"Load harness work item for {self.name_}",
                    "description": "Synthetic task owned by one load-harness agent.",
                    "task_type": "chore",
                    "risk": "none",
                    "priority": 3,
                })
                create_acknowledged = ok
                match = re.search(r"\b(cas-[0-9a-f]{4,6})\b", text)
                if ok and match:
                    self.own_task = match.group(1)
                if self.client.proc.poll() is not None:
                    raise RuntimeError(f"cas serve exited ({self.client.proc.returncode})")
                return
            except (RuntimeError, TimeoutError, OSError) as error:
                self.boot_failures.append(f"{type(error).__name__}: {error}")
                if self.client is not None:
                    self.client.close()
                    self.client = None
                if create_acknowledged or attempt + 1 == attempts:
                    raise
                time.sleep(1.0)

    def timed_call(self, tool: str, action: str, arguments: dict) -> tuple[bool, str]:
        started = now()
        try:
            ok, text = self.client.call_tool(tool, arguments, CALL_TIMEOUT_SECS)
        except TimeoutError as error:
            ok, text = False, f"harness timeout: {error}"
        except (RuntimeError, OSError, BrokenPipeError) as error:
            ok, text = False, f"transport: {error}"
        self.record(tool, action, started, now() - started, ok, text)
        return ok, text

    def arguments_for(self, tool: str, action: str, counter: int) -> dict | None:
        if tool == "task":
            if action == "show":
                return {"action": "show", "id": self.rng.choice(self.task_pool)}
            if action == "list":
                return {"action": "list", "limit": 20}
            if action in ("ready", "mine"):
                return {"action": action, "limit": 20}
            if self.own_task is None:
                return None
            if action == "notes":
                return {"action": "notes", "id": self.own_task, "note_type": "progress",
                        "notes": f"load harness progress {counter} from {self.name_}"}
            if action == "update":
                return {"action": "update", "id": self.own_task,
                        "priority": self.rng.choice([1, 2, 3])}
        if tool == "coordination":
            if action == "message":
                target = self.rng.choice([p for p in self.peers if p != self.name_] or [self.name_])
                if target == agent_name(0) and self.role == "worker":
                    # Workers address their supervisor by role, not by name.
                    target = "supervisor"
                return {"action": "message", "target": target, "summary": f"load ping {counter}",
                        "message": f"load harness message {counter} from {self.name_}"}
            return {"action": action}
        if tool == "memory":
            # Distinct vocabulary per call, so overlap detection does not
            # reject the write as a duplicate before it reaches the store.
            words = " ".join(self.rng.choice(MEMORY_WORDS) for _ in range(12))
            token = uuid.uuid4().hex[:8]
            return {"action": "remember", "title": f"load {self.name_} {counter} {token}",
                    "content": f"Observation {token} from {self.name_} call {counter}: {words}.",
                    "entry_type": "observation"}
        if tool == "search":
            return {"action": "search",
                    "query": self.rng.choice(["sqlite busy", "factory daemon loop",
                                              "task sync intents lock", "code index",
                                              "worker verification"]),
                    "limit": 5}
        return None

    def run(self) -> None:
        env = child_env(self.args, self.env, {
            "CAS_AGENT_NAME": self.name_,
            "CAS_AGENT_ROLE": self.role,
            "CAS_SESSION_ID": str(uuid.uuid4()),
            "CAS_CLONE_PATH": str(self.env["project"]),
        })
        log_dir = Path(self.args.run_dir) / "serve-logs"
        log_dir.mkdir(exist_ok=True)
        try:
            self.boot(env, log_dir)
            workload = [entry for entry in WORKLOAD
                        if f"{entry[1]}.{entry[2]}" not in self.args.exclude]
            weights = [w for w, _, _ in workload]
            counter = 0
            while now() < self.stop_at:
                counter += 1
                _, tool, action = self.rng.choices(workload, weights=weights)[0]
                arguments = self.arguments_for(tool, action, counter)
                if arguments is not None:
                    self.timed_call(tool, action, arguments)
                think = self.rng.expovariate(1.0 / self.args.think_mean)
                time.sleep(max(0.0, min(think, self.stop_at - now())))
        except Exception as error:  # noqa: BLE001 - recorded in the receipt
            self.error = f"{type(error).__name__}: {error}"
        finally:
            if self.client is not None:
                self.client.close()


class BackgroundWriter(threading.Thread):
    """Synthetic long write transactions on the scratch database.

    The field stall (GH #1165) had a background holder of the WAL write lock
    running repeated scans inside write transactions while the fleet worked.
    This reproduces that pressure at a declared level (hold and gap times)
    so different builds are compared under the same contention. The default
    (2 writers, 400 ms hold, 600 ms gap) is the standard profile; the receipt
    records the setting.
    """

    def __init__(self, index: int, db: Path, hold_ms: float, gap_ms: float,
                 stop: threading.Event):
        super().__init__(daemon=True, name=f"bg-writer-{index}")
        self.index = index
        self.db = db
        self.hold = hold_ms / 1000.0
        self.gap = gap_ms / 1000.0
        self.stop = stop
        self.rng = random.Random(index)
        self.commits = 0
        self.busy = 0

    def run(self) -> None:
        conn = sqlite3.connect(str(self.db), timeout=30, isolation_level=None)
        conn.execute("CREATE TABLE IF NOT EXISTS load_harness_pressure "
                     "(id INTEGER PRIMARY KEY, writer INTEGER, payload BLOB)")
        while not self.stop.is_set():
            try:
                conn.execute("BEGIN IMMEDIATE")
                conn.execute("INSERT INTO load_harness_pressure (writer, payload) VALUES (?, ?)",
                             (self.index, os.urandom(4096)))
                # Hold the write lock as a scan inside the transaction would.
                self.stop.wait(self.hold * self.rng.uniform(0.5, 1.5))
                conn.execute("COMMIT")
                self.commits += 1
            except sqlite3.OperationalError:
                self.busy += 1
                try:
                    conn.execute("ROLLBACK")
                except sqlite3.OperationalError:
                    pass
            self.stop.wait(self.gap * self.rng.uniform(0.5, 1.5))
        conn.close()


# ---------------------------------------------------------------------------
# Samplers
# ---------------------------------------------------------------------------


class LockSampler(threading.Thread):
    """Waits on the task-sync lock files, from /proc/locks."""

    def __init__(self, cas_dir: Path, interval: float, stop: threading.Event):
        super().__init__(daemon=True, name="lock-sampler")
        self.cas_dir = cas_dir
        self.interval = interval
        self.stop = stop
        self.episodes: list[dict] = []
        self.samples = 0
        self.max_waiters = 0
        self._open: dict[tuple[int, str], float] = {}

    def _watched_inodes(self) -> dict[int, str]:
        paths = [self.cas_dir / "task-sync-intents.lock"]
        paths += [Path(p) for p in glob.glob(str(self.cas_dir / "task-sync-intents.d" / "*.lock"))]
        inodes = {}
        for path in paths:
            try:
                inodes[path.stat().st_ino] = path.name
            except OSError:
                pass
        return inodes

    def run(self) -> None:
        while not self.stop.is_set():
            inodes = self._watched_inodes()
            waiting: set[tuple[int, str]] = set()
            try:
                lines = Path("/proc/locks").read_text().splitlines()
            except OSError:
                lines = []
            for line in lines:
                if "->" not in line:
                    continue
                fields = line.split()
                # "N: -> FLOCK ADVISORY WRITE <pid> <maj>:<min>:<inode> ..."
                try:
                    pid = int(fields[5])
                    inode = int(fields[6].split(":")[2])
                except (IndexError, ValueError):
                    continue
                if inode in inodes:
                    waiting.add((pid, inodes[inode]))
            self.samples += 1
            self.max_waiters = max(self.max_waiters, len(waiting))
            t = now()
            for key in waiting:
                self._open.setdefault(key, t)
            for key in list(self._open):
                if key not in waiting:
                    self._close(key, t)
            self.stop.wait(self.interval)
        t = now()
        for key in list(self._open):
            self._close(key, t)

    def _close(self, key, t):
        started = self._open.pop(key)
        duration = t - started
        if duration >= LOCK_WAIT_REPORT_SECS:
            self.episodes.append({"pid": key[0], "lock": key[1], "wait_secs": round(duration, 2)})


class DaemonSampler(threading.Thread):
    """Daemon loop watchdog file plus the loop thread's kernel wait channel."""

    def __init__(self, cas_dir: Path, session: str, pid: int, stop: threading.Event,
                 wchan_interval: float):
        super().__init__(daemon=True, name="daemon-sampler")
        self.path = cas_dir / "factory-daemon" / f"{re.sub(r'[^A-Za-z0-9_-]', '_', session)}.loop.json"
        self.pid = pid
        self.stop = stop
        self.wchan_interval = wchan_interval
        self.status_samples: list[dict] = []
        self.wchan_counts: dict[str, int] = {}
        self.wchan_samples = 0

    def run(self) -> None:
        last_status = 0.0
        while not self.stop.is_set():
            try:
                wchan = Path(f"/proc/{self.pid}/wchan").read_text().strip() or "running"
            except OSError:
                wchan = "gone"
            self.wchan_counts[wchan] = self.wchan_counts.get(wchan, 0) + 1
            self.wchan_samples += 1
            if now() - last_status >= 1.0:
                last_status = now()
                try:
                    status = json.loads(self.path.read_text())
                    status["_sampled_at"] = utc_now()
                    if not self.status_samples or status.get("written_at") != \
                            self.status_samples[-1].get("written_at"):
                        self.status_samples.append(status)
                except (OSError, ValueError):
                    pass
            self.stop.wait(self.wchan_interval)


def read_loadavg() -> dict | None:
    """`/proc/loadavg` as numbers, or None when it cannot be read."""
    try:
        fields = Path("/proc/loadavg").read_text().split()
        return {"load1": float(fields[0]), "load5": float(fields[1]),
                "load15": float(fields[2]), "at": utc_now()}
    except (OSError, IndexError, ValueError):
        return None


class LoadSampler(threading.Thread):
    """The host's load average through the run (factory loop v2 C6).

    Receipts are wall-clock measurements on a shared host; the load average
    taken alongside says how loaded that host was.
    """

    def __init__(self, stop: threading.Event, interval: float = 5.0):
        super().__init__(daemon=True, name="load-sampler")
        self.stop = stop
        self.interval = interval
        self.samples: list[dict] = []

    def run(self) -> None:
        while True:
            sample = read_loadavg()
            if sample is not None:
                self.samples.append(sample)
            if self.stop.wait(self.interval):
                break
        sample = read_loadavg()
        if sample is not None:
            self.samples.append(sample)


def summarize_load(samples: list[dict]) -> dict:
    """Start, middle and end load averages plus the 1-minute range."""
    if not samples:
        return {"samples": 0}
    load1 = [s["load1"] for s in samples]
    return {
        "samples": len(samples),
        "cpus": os.cpu_count(),
        "start": samples[0],
        "middle": samples[len(samples) // 2],
        "end": samples[-1],
        "load1_min": min(load1),
        "load1_max": max(load1),
        "load1_mean": round(statistics.fmean(load1), 2),
    }


# ---------------------------------------------------------------------------
# Key-to-echo probe (factory loop v2 C6)
# ---------------------------------------------------------------------------


def encode_frame(message) -> bytes:
    """The daemon's GUI framing: a 4-byte big-endian length, then JSON."""
    body = json.dumps(message).encode()
    return len(body).to_bytes(4, "big") + body


class FrameDecoder:
    """Splits the daemon's length-prefixed JSON frames out of a byte stream."""

    def __init__(self) -> None:
        self.buffer = bytearray()

    def feed(self, data: bytes) -> list:
        self.buffer.extend(data)
        messages = []
        while len(self.buffer) >= 4:
            length = int.from_bytes(self.buffer[:4], "big")
            if len(self.buffer) < 4 + length:
                break
            body = bytes(self.buffer[4:4 + length])
            del self.buffer[:4 + length]
            try:
                messages.append(json.loads(body))
            except ValueError:
                continue
        return messages


def pane_output(message) -> tuple[str, bytes] | None:
    """`(pane_id, data)` of an `Output` frame, else None."""
    if not isinstance(message, dict) or "Output" not in message:
        return None
    output = message["Output"]
    try:
        return output["pane_id"], bytes(output["data"])
    except (KeyError, TypeError, ValueError):
        return None


def supervisor_pane(welcome) -> str | None:
    """The supervisor pane's id from the daemon's `Welcome` frame."""
    try:
        panes = welcome["Welcome"]["state"]["panes"]
    except (KeyError, TypeError):
        return None
    for pane in panes:
        if pane.get("kind") == "Supervisor":
            return pane.get("id")
    return None


# `<label> write <n> (<rate>/s)`, as the synthetic writer prints it.
WRITE_SEQ = re.compile(rb"(\S+) write (\d+) \(([\d.]+)/s\)")


class EchoProbe(threading.Thread):
    """Types one key at a time into the quiet supervisor pane and times its echo.

    The probe is a GUI client of the daemon, as Commander and the desktop app
    are. A key goes out as an `Input` frame; its latency is the time until an
    `Output` frame for the same pane carries it back (the pane's terminal
    echo). After each echo a ^U clears the line. The same connection receives
    the synthetic writers' output, so their delivered write rates are
    counted from the sequence numbers they print.
    """

    def __init__(self, socket_path: Path, stop: threading.Event, interval: float,
                 timeout: float = ECHO_TIMEOUT_SECS):
        super().__init__(daemon=True, name="echo-probe")
        self.socket_path = socket_path
        self.stop = stop
        self.interval = interval
        self.timeout = timeout
        self.pane: str | None = None
        self.latencies_ms: list[float] = []
        self.timeouts = 0
        self.error: str | None = None
        self.writers: dict[str, dict] = {}
        self.measuring = threading.Event()
        self._cond = threading.Condition()
        self._echoed: set[int] = set()
        self._sock = None

    def _reader(self) -> None:
        decoder = FrameDecoder()
        while True:
            try:
                data = self._sock.recv(65536)
            except OSError:
                break
            if not data:
                break
            arrived = now()
            for message in decoder.feed(data):
                if self.pane is None and isinstance(message, dict) and "Welcome" in message:
                    with self._cond:
                        self.pane = supervisor_pane(message) or ""
                        self._cond.notify_all()
                    continue
                output = pane_output(message)
                if output is None:
                    continue
                pane_id, payload = output
                if pane_id == self.pane:
                    with self._cond:
                        self._echoed.update(payload)
                        self._cond.notify_all()
                    continue
                if not self.measuring.is_set():
                    continue
                # Keyed by the writer's own label, not the pane id: a pane
                # that is renamed or mirrored must not count a writer twice.
                for label, seq, rate in WRITE_SEQ.findall(payload):
                    key = label.decode(errors="replace")
                    entry = self.writers.setdefault(
                        key, {"first_at": arrived, "first_seq": int(seq), "frames": 0,
                              "rate": float(rate), "panes": set()})
                    entry["last_at"] = arrived
                    entry["last_seq"] = int(seq)
                    entry["frames"] += 1
                    entry["panes"].add(pane_id)

    def _connect(self) -> None:
        import socket as socket_module

        deadline = now() + 60
        while True:
            sock = socket_module.socket(socket_module.AF_UNIX, socket_module.SOCK_STREAM)
            try:
                sock.connect(str(self.socket_path))
                self._sock = sock
                return
            except OSError as error:
                sock.close()
                if now() > deadline or self.stop.is_set():
                    raise RuntimeError(f"GUI socket {self.socket_path}: {error}") from error
                time.sleep(0.5)

    def _send_key(self, key: int) -> bool:
        try:
            self._sock.sendall(encode_frame({"Input": {"pane_id": self.pane, "data": [key]}}))
            return True
        except OSError as error:
            self.error = f"send: {error}"
            return False

    def run(self) -> None:
        try:
            self._connect()
        except RuntimeError as error:
            self.error = str(error)
            return
        threading.Thread(target=self._reader, daemon=True, name="echo-reader").start()
        with self._cond:
            self._cond.wait_for(lambda: self.pane is not None, timeout=30)
        if not self.pane:
            self.error = "no supervisor pane in the daemon's Welcome"
            return
        index = 0
        while not self.stop.is_set():
            key = PROBE_KEYS[index % len(PROBE_KEYS)]
            index += 1
            with self._cond:
                self._echoed.discard(key)
            sent = now()
            if not self._send_key(key):
                return
            with self._cond:
                echoed = self._cond.wait_for(lambda: key in self._echoed, timeout=self.timeout)
            latency_ms = (now() - sent) * 1000
            if self.measuring.is_set():
                if echoed:
                    self.latencies_ms.append(latency_ms)
                else:
                    self.timeouts += 1
                    self.latencies_ms.append(self.timeout * 1000)
            if not self._send_key(KILL_LINE):
                return
            self.stop.wait(self.interval)
        try:
            self._sock.close()
        except OSError:
            pass


def summarize_echo(probe: EchoProbe | None) -> dict:
    if probe is None:
        return {"enabled": False}
    lat = probe.latencies_ms
    return {
        "enabled": True,
        "pane": probe.pane,
        "error": probe.error,
        "probes": len(lat),
        "timeouts": probe.timeouts,
        "timeout_ms": probe.timeout * 1000,
        "p50_ms": round(percentile(lat, 50), 2) if lat else None,
        "p99_ms": round(percentile(lat, 99), 2) if lat else None,
        "max_ms": round(max(lat), 2) if lat else None,
        "over_50ms": sum(1 for ms in lat if ms >= SLO_KEY_TO_ECHO_P99_MS),
    }


def read_writer_claims(slots: Path) -> list[dict]:
    """Which process claimed which writer rate (from the writers' own log)."""
    claims = []
    try:
        lines = (slots / "claims.log").read_text().splitlines()
    except OSError:
        return claims
    for line in lines:
        fields = line.split()
        if len(fields) == 5:
            claims.append({"pid": int(fields[0]), "ppid": int(fields[1]), "label": fields[2],
                           "rate": None if fields[3] == "None" else float(fields[3]),
                           "tty": fields[4]})
    return claims


def summarize_writers(probe: EchoProbe | None, rates: list[float] | None) -> dict:
    """Configured writer rates against the write rates that reached a client."""
    observed = []
    if probe is not None:
        for label, entry in sorted(probe.writers.items()):
            span = entry.get("last_at", entry["first_at"]) - entry["first_at"]
            writes = entry.get("last_seq", entry["first_seq"]) - entry["first_seq"]
            observed.append({
                "pane": label,
                "panes": sorted(entry["panes"]),
                "configured_writes_per_sec": entry["rate"],
                "frames": entry["frames"],
                "writes_per_sec": round(writes / span, 1) if span > 0 else None,
            })
    # A multiset: two writers configured at one rate need two writers seen.
    missing = sorted((collections.Counter(rates or [])
                      - collections.Counter(row["configured_writes_per_sec"] for row in observed)
                      ).elements())
    return {"configured_writes_per_sec": list(rates or []), "observed": observed,
            "missing_writes_per_sec": missing, "complete": not missing}


def parse_ts(value: str) -> dt.datetime:
    return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))


def summarize_daemon(sampler: DaemonSampler | None, alive: bool) -> dict:
    if sampler is None:
        return {"enabled": False}
    samples = sampler.status_samples
    summary: dict = {
        "enabled": True,
        "alive_at_end": alive,
        "status_samples": len(samples),
        "wchan_samples": sampler.wchan_samples,
        "wchan": dict(sorted(sampler.wchan_counts.items(), key=lambda kv: -kv[1])),
    }
    if not samples:
        summary["verdict_basis"] = "no loop.json written"
        return summary
    ages, stalls = [], []
    for status in samples:
        try:
            age = (parse_ts(status["written_at"]) - parse_ts(status["last_pass_at"])).total_seconds()
        except (KeyError, ValueError):
            continue
        ages.append(age)
        if status.get("phase") != "idle":
            # A pass has been running for at least age minus the idle sleep.
            stalls.append({"phase": status.get("phase"), "age_secs": round(age, 2),
                           "in_pass_secs_lower_bound": round(max(0.0, age - HEADLESS_SLEEP_SECS), 2),
                           "loop_thread_wait": status.get("loop_thread_wait")})
    first, last = samples[0], samples[-1]
    try:
        span = (parse_ts(last["written_at"]) - parse_ts(first["written_at"])).total_seconds()
        passes = int(last["passes"]) - int(first["passes"])
    except (KeyError, ValueError):
        span, passes = 0.0, 0
    summary.update({
        "pass_age_max_secs": round(max(ages), 2) if ages else None,
        "pass_age_p99_secs": round(percentile(ages, 99), 2) if ages else None,
        "passes": passes,
        "span_secs": round(span, 1),
        "mean_pass_ms_estimate": round(max(0.0, span / passes - HEADLESS_SLEEP_SECS) * 1000, 1)
        if passes > 0 else None,
        "in_pass_samples": len(stalls),
        "longest_in_pass": sorted(stalls, key=lambda s: -s["in_pass_secs_lower_bound"])[:10],
    })
    # Prefer per-pass timings when the daemon reports them (cas-04db). Each
    # snapshot covers the passes completed since the previous snapshot.
    windows = [s for s in samples if "window_passes" in s]
    if windows:
        timed = sum(int(s.get("window_passes", 0)) for s in windows)
        over = sum(int(s.get("passes_over_100ms", 0)) for s in windows)
        p99s = [s["p99_pass_ms"] for s in windows if s.get("p99_pass_ms") is not None]
        maxes = [s["max_pass_ms"] for s in windows if s.get("max_pass_ms") is not None]
        summary["reported"] = {
            "timed_passes": timed,
            "passes_over_100ms": over,
            "fraction_over_100ms": round(over / timed, 5) if timed else None,
            "worst_window_p99_ms": max(p99s, default=None),
            "max_pass_ms": max(maxes, default=None),
        }
        # Over the whole run, p99 < 100 ms exactly when fewer than 1% of the
        # timed passes took 100 ms or more.
        summary["pass_p99_under_100ms"] = timed > 0 and over / timed < 0.01
        # Window series, so slow passes can be lined up with the agent load.
        summary["windows"] = [
            {"written_at": s.get("written_at"), "phase": s.get("phase"),
             "passes": s.get("window_passes", 0), "over_100ms": s.get("passes_over_100ms", 0),
             "p99_ms": s.get("p99_pass_ms"), "max_ms": s.get("max_pass_ms"),
             "phase_latency": s.get("phase_latency")}
            for s in windows
            if s.get("passes_over_100ms", 0) > 0 or (s.get("max_pass_ms") or 0) >= 100
        ]
        # cas-ee9ab: per-phase totals across every window, so slow passes
        # are attributed to the loop phase that took the time.
        phases: dict = {}
        for snapshot in windows:
            for name, lat in (snapshot.get("phase_latency") or {}).items():
                agg = phases.setdefault(name, {"entries": 0, "max_ms": 0, "over_100ms": 0})
                agg["entries"] += int(lat.get("entries", 0))
                agg["max_ms"] = max(agg["max_ms"], int(lat.get("max_ms", 0)))
                agg["over_100ms"] += int(lat.get("over_100ms", 0))
        summary["phase_latency"] = phases
        summary["pass_p99_ms"] = max(p99s, default=None)
        summary["verdict_basis"] = ("daemon-reported per-pass timings: p99 < 100 ms iff under 1% "
                                    "of timed passes took 100 ms or more (pass_p99_ms is the "
                                    "worst 5 s window's p99)")
    else:
        # Lower bound for the slowest pass from the 5 s watchdog snapshots.
        # Without per-pass timings, bound the slowest pass from below: a pass
        # seen in progress for X s took at least X - 0.5 s, and the mean pass
        # (span / passes minus the idle sleep) cannot exceed the slowest.
        worst_ms = max((s["in_pass_secs_lower_bound"] for s in stalls), default=0.0) * 1000
        mean_ms = summary["mean_pass_ms_estimate"] or 0.0
        summary["pass_p99_ms"] = round(max(worst_ms, mean_ms), 1)
        summary["verdict_basis"] = ("lower bound from loop.json snapshots (watchdog every 5 s): "
                                    "max(longest pass seen in progress minus the 0.5 s idle sleep, "
                                    "mean pass estimate)")
    return summary


def count_busy_warnings(cas_dir: Path, t0_wall: float) -> dict:
    per_minute: dict[str, int] = {}
    total = 0
    files = sorted(glob.glob(str(cas_dir / "logs" / "*.log*")))
    for path in files:
        try:
            with open(path, errors="replace") as handle:
                for line in handle:
                    if BUSY_PATTERNS.search(line):
                        total += 1
                        stamp = re.match(r"\s*(\d{4}-\d\d-\d\dT\d\d:\d\d)", line)
                        key = stamp.group(1) if stamp else "unstamped"
                        per_minute[key] = per_minute.get(key, 0) + 1
        except OSError:
            continue
    return {"total": total, "log_files": [Path(p).name for p in files],
            "per_minute": dict(sorted(per_minute.items()))}


# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------


def task_pool_from(cas_dir: Path, size: int) -> list[str]:
    conn = sqlite3.connect(f"file:{cas_dir / 'cas.db'}?mode=ro", uri=True)
    try:
        rows = conn.execute("SELECT id FROM tasks ORDER BY updated_at DESC LIMIT ?", (size,)).fetchall()
    except sqlite3.Error:
        rows = []
    conn.close()
    return [row[0] for row in rows] or ["cas-0000"]


def prewarm(args, env, run_dir: Path) -> dict:
    """Run one canonical `cas serve` alone before the measured load.

    A build's first canonical serve may do one-off boot work on the copied
    database: from cas-8256 on, it purges code-index copies left by factory
    worktrees. Only a non-worker serve in the store's own checkout does that,
    so the prewarm serve runs as the supervisor role. It stays up for at
    least `--prewarm-secs`, then until the code-index footprint has been
    stable for 10 s (capped at `--prewarm-max-secs`), so that work is
    reported here and kept out of the SLO latencies.
    """
    if args.prewarm_secs <= 0:
        return {"secs": 0}
    started = now()
    stderr_path = run_dir / "prewarm.stderr"
    client = McpClient(args.cas_bin, env["project"], child_env(args, env, {
        "CAS_AGENT_NAME": "load-prewarm",
        "CAS_AGENT_ROLE": "supervisor",
        "CAS_SESSION_ID": str(uuid.uuid4()),
    }), stderr_path)
    samples = []
    try:
        client.initialize(CALL_TIMEOUT_SECS)
        boot = now() - started
        first = code_index_rows(env["cas_dir"])
        last_rows, rows_since = first, now()
        while True:
            rows = code_index_rows(env["cas_dir"])
            elapsed = now() - started
            samples.append({"t": round(elapsed, 1), **rows})
            if rows != last_rows:
                last_rows, rows_since = rows, now()
            try:
                reported = "Code index cop" in stderr_path.read_text(errors="replace")
            except OSError:
                reported = False
            # The purge is done when the serve reports it (it prints only when
            # it changed something or failed), or after a further minute with
            # an unchanged footprint (nothing to purge).
            purged = reported or (rows == first and elapsed >= args.prewarm_secs + 60)
            # With indexing on, also let the canonical writer's first build
            # settle: files present and unchanged for 20 s.
            indexed = not args.code_index or (
                rows.get("code_files", 0) > 0 and now() - rows_since >= 20)
            if elapsed >= args.prewarm_secs and purged and indexed:
                break
            if elapsed >= args.prewarm_max_secs:
                break
            time.sleep(2.0)
    finally:
        client.close()
    try:
        lines = [line.strip() for line in stderr_path.read_text(errors="replace").splitlines()
                 if "Code index" in line]
    except OSError:
        lines = []
    return {"secs": args.prewarm_secs, "initialize_secs": round(boot, 2),
            "elapsed_secs": round(now() - started, 1), "code_index_log": lines[-5:],
            "footprint": samples[:: max(1, len(samples) // 20)]}


def code_index_rows(cas_dir: Path) -> dict:
    """Code-index footprint of the scratch copy (boot purges change it)."""
    try:
        conn = sqlite3.connect(f"file:{cas_dir / 'cas.db'}?mode=ro", uri=True, timeout=30)
        files, repos = conn.execute(
            "SELECT COUNT(*), COUNT(DISTINCT repository) FROM code_files").fetchone()
        conn.close()
        return {"code_files": files, "repositories": repos}
    except sqlite3.Error as error:
        return {"error": str(error)}


def start_daemon(args, env, log_path: Path) -> subprocess.Popen:
    daemon_env = child_env(args, env, {"CAS_AGENT_ROLE": "supervisor"})
    log = open(log_path, "ab")
    proc = subprocess.Popen(
        [args.cas_bin, "factory", "daemon", "--session", env["session"], "--cwd",
         str(env["project"]), "--workers", str(len(args.synthetic_writers or [])),
         "--no-worktrees", "--no-phone-home",
         "--foreground", "--supervisor-name", "load-supervisor"],
        cwd=env["project"],
        env=daemon_env,
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=log,
        start_new_session=True,
    )
    CHILDREN.append(proc)
    return proc


def stop_process_group(proc: subprocess.Popen | None) -> None:
    if proc is None or proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=15)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def summarize_calls(results: list[dict]) -> dict:
    by_key: dict[str, list[dict]] = {}
    for row in results:
        by_key.setdefault(f"{row['tool']}.{row['action']}", []).append(row)
    by_key["_all"] = list(results)
    out = {}
    for key, rows in sorted(by_key.items()):
        lat = [r["latency"] for r in rows]
        out[key] = {
            "n": len(rows),
            "errors": sum(1 for r in rows if not r["ok"]),
            "p50": round(percentile(lat, 50), 3) if lat else None,
            "p99": round(percentile(lat, 99), 3) if lat else None,
            "max": round(max(lat), 3) if lat else None,
            "mean": round(statistics.fmean(lat), 3) if lat else None,
            "over_deadline": sum(1 for r in rows if r["latency"] >= MCP_DEADLINE_SECS
                                 or "deadline" in r["detail"].lower()),
        }
    return out


def evaluate_slos(calls: list[dict], daemon: dict, lock_episodes: list[dict],
                  echo: dict | None = None) -> list[dict]:
    # Per-task stripe waits (task-sync-intents.d/NN.lock) are same-task
    # serialization and are reported, not judged; the shared lease is judged.
    global_waits = sum(1 for e in lock_episodes if e["lock"] == "task-sync-intents.lock")
    task_coord = [r["latency"] for r in calls if r["tool"] in ("task", "coordination")]
    p99 = percentile(task_coord, 99)
    deadline_hits = sum(1 for r in calls if r["latency"] >= MCP_DEADLINE_SECS
                        or "deadline" in r["detail"].lower())
    slos = [
        {"slo": "MCP task/coordination p99 < 2 s", "observed": p99,
         "met": p99 is not None and p99 < SLO_P99_SECS},
        {"slo": f"no wait >= {LOCK_WAIT_REPORT_SECS} s on the task-sync-intents.lock lease",
         "observed": global_waits, "met": global_waits == 0},
        {"slo": "no call reaches the 55 s deadline", "observed": deadline_hits,
         "met": deadline_hits == 0},
    ]
    if daemon.get("enabled"):
        observed = daemon.get("pass_p99_ms")
        if "pass_p99_under_100ms" in daemon:
            within = daemon["pass_p99_under_100ms"]
        else:
            within = observed is None or observed < SLO_LOOP_PASS_P99_MS
        met = daemon.get("status_samples", 0) > 0 and daemon.get("alive_at_end", False) and within
        slos.append({"slo": "daemon loop pass p99 < 100 ms", "observed": observed, "met": met,
                     "basis": daemon.get("verdict_basis")})
    if echo and echo.get("enabled"):
        observed = echo.get("p99_ms")
        met = (echo.get("error") is None and echo.get("probes", 0) > 0
               and observed is not None and observed < SLO_KEY_TO_ECHO_P99_MS)
        slos.append({"slo": "key -> echo p99 < 50 ms", "observed": observed, "met": met,
                     "basis": f"{echo.get('probes', 0)} probes over the GUI socket; "
                              f"{echo.get('timeouts', 0)} timeouts counted as "
                              f"{echo.get('timeout_ms')} ms"})
    return slos


def live_agents(receipt: dict) -> int:
    """Agents that ran the whole phase: an agent that failed is not load."""
    return receipt["agents"] - len(receipt.get("agent_errors") or {})


def file_sha256(path: str) -> str | None:
    try:
        digest = hashlib.sha256()
        with open(path, "rb") as handle:
            for block in iter(lambda: handle.read(1 << 20), b""):
                digest.update(block)
        return digest.hexdigest()
    except OSError:
        return None


def flv2_receipts(receipt: dict) -> list[dict]:
    """Factory loop v2 §7 receipt rows (C6, cas-9db0b).

    Receipts are reference-host measurements, not merge gates. Each row
    carries the host load average taken during the run, because the
    factory hosts are shared.
    """
    rows = []
    load = receipt.get("host_load", {})
    live = live_agents(receipt)
    context = {
        "agents": receipt["agents"],
        "live_agents": live,
        "agent_boot_failures": sum(len(v) for v in (receipt.get("agent_boot_failures") or {}).values()),
        "cas_bin": receipt.get("cas_bin"),
        "cas_bin_sha256": receipt.get("cas_bin_sha256"),
        "cas_version": receipt.get("cas_version"),
        "synthetic_writers": receipt.get("synthetic_writers", {}).get(
            "configured_writes_per_sec", []),
        "background_writers": receipt["background_writers"]["writers"],
        "host_load1": {key: load.get(key) for key in ("load1_min", "load1_mean", "load1_max")},
        "cpus": load.get("cpus"),
    }
    daemon = receipt["daemon"]
    if daemon.get("enabled"):
        within = daemon.get("pass_p99_under_100ms")
        rows.append({
            "receipt": RECEIPT_CONTROL_PASS,
            "budget": "control pass p99 < 100 ms",
            "observed_ms": daemon.get("pass_p99_ms"),
            "passes_over_100ms": daemon.get("reported", {}).get("passes_over_100ms"),
            "timed_passes": daemon.get("reported", {}).get("timed_passes"),
            "met": bool(within) and live >= 16,
            **context,
        })
    echo = receipt.get("key_to_echo", {})
    if echo.get("enabled"):
        rows.append({
            "receipt": RECEIPT_KEY_TO_ECHO,
            "budget": "key -> echo p99 < 50 ms",
            "observed_ms": echo.get("p99_ms"),
            "probes": echo.get("probes"),
            "timeouts": echo.get("timeouts"),
            "met": (echo.get("error") is None and echo.get("p99_ms") is not None
                    and echo["p99_ms"] < SLO_KEY_TO_ECHO_P99_MS
                    and live >= 16 and bool(context["synthetic_writers"])
                    and receipt.get("synthetic_writers", {}).get("complete", False)),
            "writers_missing": receipt.get("synthetic_writers", {}).get("missing_writes_per_sec"),
            **context,
        })
    return rows


def fmt(value, unit="") -> str:
    if value is None:
        return "n/a"
    if isinstance(value, float):
        return f"{value:.3f}{unit}"
    return f"{value}{unit}"


def write_markdown(receipt: dict, path: Path) -> None:
    lines = [
        f"# Many-agent load harness: {receipt['label']}",
        "",
        f"- Binary: `{receipt['cas_version']}` ({receipt['cas_bin']})",
        f"- Agents: {receipt['agents']}; duration {receipt['duration_secs']} s; "
        f"think mean {receipt['think_mean_secs']} s; daemon {'on' if receipt['daemon']['enabled'] else 'off'}",
        f"- Background writers: {receipt['background_writers']['writers']} "
        f"(hold {receipt['background_writers']['hold_ms']} ms, gap {receipt['background_writers']['gap_ms']} ms; "
        f"{receipt['background_writers']['commits']} commits)",
        f"- Database: {receipt['database']['bytes'] / 1e9:.2f} GB scratch copy of `{receipt['database']['source']}`",
        f"- Started {receipt['started_at']}; result **{'PASS' if receipt['slos_met'] else 'FAIL'}**",
        "",
        "## SLOs",
        "",
        "| SLO | Observed | Met |",
        "|---|---|---|",
    ]
    for slo in receipt["slos"]:
        lines.append(f"| {slo['slo']} | {fmt(slo['observed'])} | {'yes' if slo['met'] else '**no**'} |")
    lines += ["", "## Calls (seconds)", "", "| Tool.action | n | errors | p50 | p99 | max | >= 55 s |",
              "|---|---|---|---|---|---|---|"]
    for key, row in receipt["calls"].items():
        lines.append(f"| {key} | {row['n']} | {row['errors']} | {fmt(row['p50'])} | {fmt(row['p99'])} "
                     f"| {fmt(row['max'])} | {row['over_deadline']} |")
    locks = receipt["lock_waits"]
    lines += ["", "## Task-sync lock waits (/proc/locks)", "",
              f"- Waits >= {LOCK_WAIT_REPORT_SECS} s: {locks['count']}; total {locks['total_secs']} s; "
              f"max {locks['max_secs']} s; max simultaneous waiters {locks['max_waiters']}"]
    busy = receipt["sqlite_busy"]
    lines += ["", "## SQLite busy/retry warnings (scratch logs)", "",
              f"- Total {busy['total']}; peak per minute {max(busy['per_minute'].values(), default=0)}"]
    daemon = receipt["daemon"]
    if daemon.get("enabled"):
        lines += ["", "## Factory daemon loop", "",
                  f"- loop.json snapshots: {daemon.get('status_samples')}; passes {daemon.get('passes')} "
                  f"over {daemon.get('span_secs')} s; mean pass estimate {fmt(daemon.get('mean_pass_ms_estimate'), ' ms')}",
                  f"- Oldest pass age seen {fmt(daemon.get('pass_age_max_secs'), ' s')}; "
                  f"pass p99 {fmt(daemon.get('pass_p99_ms'), ' ms')} ({daemon.get('verdict_basis')})",
                  f"- Loop thread wait channels: {daemon.get('wchan')}"]
        if daemon.get("phase_latency"):
            lines += ["", "| Loop phase | entries | >= 100 ms | max ms |", "|---|---|---|---|"]
            for name, agg in sorted(daemon["phase_latency"].items(),
                                    key=lambda kv: (-kv[1]["over_100ms"], -kv[1]["max_ms"])):
                lines.append(f"| {name} | {agg['entries']} | {agg['over_100ms']} | {agg['max_ms']} |")
            lines.append("")
        if daemon.get("reported"):
            rep = daemon["reported"]
            lines.append(f"- Daemon-timed passes: {rep['timed_passes']}; >= 100 ms: "
                         f"{rep['passes_over_100ms']} ({fmt(rep['fraction_over_100ms'])}); "
                         f"worst window p99 {fmt(rep['worst_window_p99_ms'], ' ms')}; "
                         f"max {fmt(rep['max_pass_ms'], ' ms')}")
    lines += markdown_flv2(receipt)
    control = receipt.get("no_pressure")
    if control:
        all_calls = control["calls"].get("_all", {})
        lines += ["", "## No-pressure control (same build, background writers off)", "",
                  "| SLO | Observed | Met |", "|---|---|---|"]
        for slo in control["slos"]:
            lines.append(f"| {slo['slo']} | {fmt(slo['observed'])} | {'yes' if slo['met'] else '**no**'} |")
        lines += ["",
                  f"- All calls: n {all_calls.get('n')}; p50 {fmt(all_calls.get('p50'))} s; "
                  f"p99 {fmt(all_calls.get('p99'))} s; max {fmt(all_calls.get('max'))} s",
                  f"- Lock waits >= {LOCK_WAIT_REPORT_SECS} s: {control['lock_waits']['count']} "
                  f"(max {control['lock_waits']['max_secs']} s); SQLite busy warnings "
                  f"{control['sqlite_busy']['total']}",
                  f"- Daemon pass p99 {fmt(control['daemon'].get('pass_p99_ms'), ' ms')} "
                  f"({control['daemon'].get('verdict_basis')})"]
        lines += markdown_flv2(control)
    agents = receipt["agent_errors"]
    if agents:
        lines += ["", "## Agent failures", ""] + [f"- {name}: {err}" for name, err in agents.items()]
    path.write_text("\n".join(lines) + "\n")


def markdown_flv2(receipt: dict) -> list[str]:
    """Host load, synthetic writers, key-to-echo and the §7 receipt rows."""
    lines = []
    load = receipt.get("host_load") or {}
    if load.get("samples"):
        lines += ["", "### Host load (/proc/loadavg)", "",
                  f"- {load['samples']} samples on {load.get('cpus')} CPUs; 1-min load "
                  f"{load['load1_min']}–{load['load1_max']} (mean {load['load1_mean']}); "
                  f"start {load['start']['load1']}, middle {load['middle']['load1']}, "
                  f"end {load['end']['load1']}"]
    writers = receipt.get("synthetic_writers") or {}
    if writers.get("configured_writes_per_sec"):
        observed = ", ".join(
            f"{row['pane']} {fmt(row['writes_per_sec'])}/s ({row['frames']} frames)"
            for row in writers.get("observed", [])) or "none observed"
        lines += ["", "### Synthetic PTY writers", "",
                  f"- Configured {writers['configured_writes_per_sec']} writes/s; "
                  f"delivered to the GUI client: {observed}"
                  + ("" if writers.get("complete", True) else
                     f"; **missing** {writers.get('missing_writes_per_sec')} writes/s")]
    echo = receipt.get("key_to_echo") or {}
    if echo.get("enabled"):
        lines += ["", "### Key -> echo (GUI socket, supervisor pane)", "",
                  f"- Probes {echo.get('probes')}; timeouts {echo.get('timeouts')}; "
                  f"p50 {fmt(echo.get('p50_ms'), ' ms')}; p99 {fmt(echo.get('p99_ms'), ' ms')}; "
                  f"max {fmt(echo.get('max_ms'), ' ms')}; >= 50 ms {echo.get('over_50ms')}"
                  + (f"; error: {echo['error']}" if echo.get("error") else "")]
    rows = receipt.get("flv2_receipts") or []
    if rows:
        lines += ["", "### Factory loop v2 receipts", "",
                  f"- Binary `{rows[0].get('cas_bin')}` sha256 `{rows[0].get('cas_bin_sha256')}`", "",
                  "| Receipt | Budget | Observed | Met | Host load1 (min/mean/max) |",
                  "|---|---|---|---|---|"]
        for row in rows:
            host = row["host_load1"]
            lines.append(f"| `{row['receipt']}` ({row['live_agents']}/{row['agents']} agents live) "
                         f"| {row['budget']} | {fmt(row['observed_ms'], ' ms')} "
                         f"| {'yes' if row['met'] else '**no**'} | {host.get('load1_min')}/"
                         f"{host.get('load1_mean')}/{host.get('load1_max')} on {row['cpus']} CPUs |")
    return lines


def harness_commit() -> str:
    """Commit of this script (with `+dirty` when it differs from that commit)."""
    here = Path(__file__).resolve().parent
    try:
        sha = subprocess.run(["git", "-C", str(here), "rev-parse", "HEAD"], capture_output=True,
                             text=True, timeout=30).stdout.strip()
        dirty = subprocess.run(["git", "-C", str(here), "diff", "--quiet", "HEAD", "--",
                                Path(__file__).name], timeout=30).returncode != 0
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"
    return f"{sha}+dirty" if dirty else sha


def cas_version(cas_bin: str) -> str:
    try:
        return subprocess.run([cas_bin, "--version"], capture_output=True, text=True,
                              timeout=30).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"


def run_once(args: argparse.Namespace) -> tuple[dict, list[dict]]:
    """One load phase on a fresh scratch copy; returns the receipt and calls."""
    scratch = Path(args.scratch).expanduser()
    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    # Short on purpose: the daemon's unix socket paths live under it.
    run_dir = scratch / f"run-{stamp}"
    run_dir.mkdir(parents=True)
    (run_dir / "LABEL").write_text(f"{args.label} n={args.agents}\n")
    args.run_dir = str(run_dir)

    env = prepare_run(args, run_dir)
    index_at_copy = code_index_rows(env["cas_dir"])
    prewarm_report = prewarm(args, env, run_dir)
    index_at_load = code_index_rows(env["cas_dir"])
    pool = task_pool_from(env["cas_dir"], 200)
    peers = [agent_name(i) for i in range(args.agents)]
    print(f"[load] run dir {run_dir}; {len(pool)} tasks in the show pool", flush=True)

    stop = threading.Event()
    lock_sampler = LockSampler(env["cas_dir"], args.lock_interval, stop)
    lock_sampler.start()
    load_sampler = LoadSampler(stop)
    load_sampler.start()

    daemon_proc = None
    daemon_sampler = None
    if args.daemon:
        daemon_proc = start_daemon(args, env, run_dir / "daemon.log")
        daemon_sampler = DaemonSampler(env["cas_dir"], env["session"], daemon_proc.pid, stop,
                                       args.wchan_interval)
        daemon_sampler.start()
        time.sleep(args.daemon_warmup)

    echo_probe = None
    if args.daemon and args.echo_probe:
        echo_probe = EchoProbe(env["home"] / ".cas" / f"factory-{env['session']}.gui.sock", stop,
                               args.echo_interval)
        echo_probe.start()

    writers = [BackgroundWriter(i, env["cas_dir"] / "cas.db", args.bg_hold_ms, args.bg_gap_ms, stop)
               for i in range(args.bg_writers)]
    for writer in writers:
        writer.start()

    results: list[dict] = []
    results_lock = threading.Lock()
    args.t0 = now()
    if echo_probe is not None:
        echo_probe.measuring.set()
    t0_wall = time.time()
    started_at = utc_now()
    stop_at = args.t0 + args.duration
    agents = [Agent(i, args, env, pool, peers, results, results_lock, stop_at)
              for i in range(args.agents)]
    for agent in agents:
        agent.start()
        time.sleep(args.stagger)
    for agent in agents:
        agent.join(timeout=args.duration + CALL_TIMEOUT_SECS + 60)

    daemon_alive = daemon_proc is not None and daemon_proc.poll() is None
    stop.set()
    for writer in writers:
        writer.join(timeout=30)
    lock_sampler.join(timeout=10)
    load_sampler.join(timeout=10)
    if echo_probe is not None:
        echo_probe.join(timeout=ECHO_TIMEOUT_SECS + 5)
    if daemon_sampler is not None:
        daemon_sampler.join(timeout=10)
    stop_process_group(daemon_proc)

    calls = summarize_calls(results)
    daemon = summarize_daemon(daemon_sampler, daemon_alive)
    index_at_end = code_index_rows(env["cas_dir"])
    waits = [e["wait_secs"] for e in lock_sampler.episodes]
    receipt = {
        "schema": "cas-load-harness/v1",
        "label": args.label,
        "cas_bin": args.cas_bin,
        "cas_version": cas_version(args.cas_bin),
        "cas_bin_sha256": file_sha256(shutil.which(args.cas_bin) or args.cas_bin),
        "harness_commit": harness_commit(),
        "started_at": started_at,
        "finished_at": utc_now(),
        "agents": args.agents,
        "duration_secs": args.duration,
        "think_mean_secs": args.think_mean,
        "seed": args.seed,
        "excluded_workload": sorted(args.exclude),
        "prewarm": prewarm_report,
        "code_index_enabled": args.code_index,
        "code_index": {"at_copy": index_at_copy, "at_load_start": index_at_load,
                       "at_end": index_at_end},
        "background_writers": {
            "writers": args.bg_writers,
            "hold_ms": args.bg_hold_ms,
            "gap_ms": args.bg_gap_ms,
            "commits": sum(w.commits for w in writers),
            "busy": sum(w.busy for w in writers),
        },
        "run_dir": str(run_dir),
        "database": {
            "source": args.source_db,
            "bytes": (env["cas_dir"] / "cas.db").stat().st_size
            if (env["cas_dir"] / "cas.db").exists() else 0,
        },
        "agent_boot_secs": {a.name_: round(a.boot_secs, 2) if a.boot_secs else None for a in agents},
        "agent_errors": {a.name_: a.error for a in agents if a.error},
        "agent_boot_failures": {a.name_: a.boot_failures for a in agents if a.boot_failures},
        "calls": calls,
        "lock_waits": {
            "count": len(waits),
            "total_secs": round(sum(waits), 1),
            "max_secs": max(waits, default=0.0),
            "max_waiters": lock_sampler.max_waiters,
            "samples": lock_sampler.samples,
            "episodes": sorted(lock_sampler.episodes, key=lambda e: -e["wait_secs"])[:50],
        },
        "sqlite_busy": count_busy_warnings(env["cas_dir"], t0_wall),
        "daemon": daemon,
        "host_load": summarize_load(load_sampler.samples),
        "writer_claims": read_writer_claims(env["fake_bin"] / "writer-slots"),
        "synthetic_writers": summarize_writers(echo_probe, args.synthetic_writers),
        "key_to_echo": summarize_echo(echo_probe),
    }
    receipt["slos"] = evaluate_slos(results, daemon, lock_sampler.episodes,
                                    receipt["key_to_echo"])
    receipt["slos_met"] = all(s["met"] for s in receipt["slos"])
    receipt["flv2_receipts"] = flv2_receipts(receipt)
    if not args.keep_scratch:
        for suffix in ("cas.db", "cas.db-wal", "cas.db-shm"):
            path = env["cas_dir"] / suffix
            if path.exists():
                path.unlink()
    return receipt, results


def control_summary(receipt: dict) -> dict:
    """The no-pressure control phase, kept compact inside the main receipt."""
    keep = ("started_at", "finished_at", "run_dir", "background_writers", "calls", "slos",
            "slos_met", "sqlite_busy", "agent_errors", "agent_boot_failures", "host_load",
            "synthetic_writers", "writer_claims",
            "key_to_echo", "flv2_receipts", "agents")
    summary = {key: receipt[key] for key in keep}
    summary["lock_waits"] = {k: v for k, v in receipt["lock_waits"].items() if k != "episodes"}
    summary["daemon"] = {k: v for k, v in receipt["daemon"].items() if k != "longest_in_pass"}
    return summary


def run(args: argparse.Namespace) -> int:
    out = Path(args.out).expanduser()
    out.mkdir(parents=True, exist_ok=True)
    receipt, results = run_once(args)
    for row in results:
        row["phase"] = "pressure" if args.bg_writers else "no-pressure"
    # The SLO verdict is the run as configured. With background pressure on,
    # a second phase without it is recorded alongside, as the control.
    if args.control and args.bg_writers > 0:
        control_args = argparse.Namespace(**vars(args))
        control_args.bg_writers = 0
        print("[load] no-pressure control phase", flush=True)
        control, control_results = run_once(control_args)
        receipt["no_pressure"] = control_summary(control)
        for row in control_results:
            row["phase"] = "no-pressure"
        results = results + control_results

    base = out / f"load-{args.label}-n{args.agents}"
    (base.with_suffix(".json")).write_text(json.dumps(receipt, indent=2) + "\n")
    with open(base.with_suffix(".calls.jsonl"), "w") as handle:
        for row in results:
            handle.write(json.dumps(row) + "\n")
    write_markdown(receipt, base.with_suffix(".md"))
    print(f"[load] receipt {base.with_suffix('.json')}", flush=True)
    for slo in receipt["slos"]:
        print(f"[load] {'PASS' if slo['met'] else 'FAIL'} {slo['slo']}: observed {slo['observed']}",
              flush=True)
    for row in receipt.get("flv2_receipts", []):
        print(f"[load] receipt {row['receipt']}: observed {row['observed_ms']} ms; "
              f"{'met' if row['met'] else 'NOT met'}; host load1 {row['host_load1']}", flush=True)
    if "no_pressure" in receipt:
        for row in receipt["no_pressure"].get("flv2_receipts", []):
            print(f"[load] control receipt {row['receipt']}: observed {row['observed_ms']} ms; "
                  f"{'met' if row['met'] else 'NOT met'}", flush=True)
        for slo in receipt["no_pressure"]["slos"]:
            print(f"[load] control {'PASS' if slo['met'] else 'FAIL'} {slo['slo']}: "
                  f"observed {slo['observed']}", flush=True)
    if not results:
        return 2
    return 0 if receipt["slos_met"] else 1


def parse_rates(text: str) -> list[float]:
    """`38,63,236` -> [38.0, 63.0, 236.0]; every rate must be positive."""
    try:
        rates = [float(part) for part in text.split(",") if part.strip()]
    except ValueError as error:
        raise argparse.ArgumentTypeError(f"not a rate list: {text}") from error
    if not rates or any(rate <= 0 for rate in rates):
        raise argparse.ArgumentTypeError(f"rates must be positive: {text}")
    return rates


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--cas-bin", default=shutil.which("cas") or "cas")
    parser.add_argument("--label", required=True, help="receipt name, e.g. baseline-0d670c33c")
    parser.add_argument("--agents", "-n", type=int, default=16)
    parser.add_argument("--duration", type=float, default=300.0, help="seconds of agent load")
    parser.add_argument("--think-mean", type=float, default=3.0,
                        help="mean seconds between one agent's calls (exponential)")
    parser.add_argument("--exclude", action="append", default=[], metavar="TOOL.ACTION",
                        help="drop one workload entry, e.g. coordination.message (repeatable)")
    parser.add_argument("--stagger", type=float, default=0.25, help="seconds between agent starts")
    parser.add_argument("--seed", type=int, default=1165)
    parser.add_argument("--source-db", help="database to snapshot (opened read-only)")
    parser.add_argument("--refresh-db", action="store_true", help="re-snapshot the source database")
    parser.add_argument("--source-config", help="config.toml to copy into the scratch .cas")
    parser.add_argument("--project-src", help="git repository exported as the scratch project")
    parser.add_argument("--project-rev", default="HEAD")
    parser.add_argument("--scratch", required=True, help="scratch root (never the live .cas)")
    parser.add_argument("--out", required=True, help="artifact directory for receipts")
    parser.add_argument("--daemon", action=argparse.BooleanOptionalAction, default=True,
                        help="run a headless `cas factory daemon --workers 0`")
    parser.add_argument("--daemon-warmup", type=float, default=10.0)
    parser.add_argument("--prewarm-secs", type=float, default=0.0,
                        help="run one canonical cas serve alone at least this long before the "
                             "load, so one-off boot work on the copied DB stays out of the "
                             "measured window")
    parser.add_argument("--prewarm-max-secs", type=float, default=600.0)
    parser.add_argument("--code-index", action=argparse.BooleanOptionalAction, default=None,
                        help="set [code] enabled in the scratch config (default: as copied)")
    parser.add_argument("--cloud-endpoint", default="http://127.0.0.1:9",
                        help="cloud endpoint for children; default is a closed local port")
    parser.add_argument("--lock-interval", type=float, default=0.2)
    parser.add_argument("--wchan-interval", type=float, default=0.2)
    parser.add_argument("--bg-writers", type=int, default=2,
                        help="synthetic background write-transaction holders (field-like "
                             "pressure; 0 turns it off)")
    parser.add_argument("--bg-hold-ms", type=float, default=400.0,
                        help="mean time each background transaction holds the write lock")
    parser.add_argument("--bg-gap-ms", type=float, default=600.0,
                        help="mean pause between one writer's transactions")
    parser.add_argument("--control", action=argparse.BooleanOptionalAction, default=True,
                        help="after a run with background pressure, record a no-pressure "
                             "control phase in the same receipt")
    parser.add_argument("--synthetic-writers", type=parse_rates, default=None, metavar="RATES",
                        help="comma-separated writes/s, one worker pane per rate (factory loop "
                             "v2 C6; GH #1210's codex rates are 38,63,236)")
    parser.add_argument("--echo-probe", action=argparse.BooleanOptionalAction, default=True,
                        help="time key -> echo on the supervisor pane over the GUI socket")
    parser.add_argument("--echo-interval", type=float, default=0.1,
                        help="seconds between key-to-echo probes")
    parser.add_argument("--boot-retries", type=int, default=2,
                        help="restart an agent whose cas serve exits during boot this many "
                             "times; failures are recorded in the receipt")
    parser.add_argument("--keep-scratch", action="store_true",
                        help="keep the scratch database copy after the run")
    args = parser.parse_args(argv)
    if args.agents < 1:
        parser.error("--agents must be at least 1")
    scratch = Path(args.scratch).expanduser().resolve()
    if args.source_db:
        live = Path(args.source_db).expanduser().resolve().parent
        if scratch == live or live in scratch.parents:
            parser.error("--scratch must not be inside the live .cas directory")
    def interrupted(*_):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    try:
        return run(args)
    except KeyboardInterrupt:
        return 2
    finally:
        for proc in CHILDREN:
            if proc.poll() is None:
                try:
                    os.killpg(proc.pid, signal.SIGKILL) if os.getpgid(proc.pid) == proc.pid \
                        else proc.kill()
                except (ProcessLookupError, PermissionError):
                    pass


if __name__ == "__main__":
    sys.exit(main())
