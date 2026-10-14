#!/usr/bin/env python3
"""Two-machine live check for same-repo supervisor collaboration (cas-daa0, cas-8ca9).

One command stands up the epic-tip build on soundwave (this host) and prowl
(over ssh) in scratch homes and drives the collaboration flow between two
headless factory supervisors of one repo. Each step writes a JSON receipt,
then everything the run created is torn down on both machines.

Each machine gets a scratch run directory: HOME, bin, a scratch git repo and
its Cassy project. Every scratch process runs with a cleared environment. The
operator's ~/.cas, installed cas and live repos are never read or written.
The installed `cas --version` and a sha256 of each binary are recorded before
and after the run.

On each machine a real `cas factory daemon --supervisor-cli claude` starts
with a `claude` stub on PATH. The stub is scripts/live_check_peer_collab/harness.py,
which plays the harness: it runs `cas serve` and `cas hook SessionStart`, logs
pane input, and takes MCP calls from the runner over a Unix socket.

Cloud: --cloud-url with --token-file-a/--token-file-b (or env
LIVE_CHECK_TOKEN_A/_B) for the real cloud. Without --cloud-url, a local
stand-in (live_check_peer_collab/standin_cloud.py) is started on this host's
tailnet address. Tokens are passed on stdin and written 0600, never printed
or put on a command line.

Before anything is created, a preflight probes both binaries (`cas --version`
and feature markers, see harness.py FEATURE_MARKERS). A step whose feature is
missing from either binary is reported `pending`, never passed; once both
binaries carry it, the step runs for real. Binaries at different commits are
refused unless --allow-mixed.

Exit status: 0 when no step failed (passed, pending or skipped); 1 when a step
failed or errored; 2 on a configuration error (missing token or binary, mixed
binaries, unknown step), a setup failure, an unclean teardown or a changed
installed cas; 128+N when signal N (SIGINT, SIGTERM, SIGHUP) stopped the run.
A signal still tears down everything the run created on both hosts.

Example (stand-in):
  scripts/live-check-peer-collab.py \\
      --binary /mnt/rewind/cas/scratch/peer-collab/bin/cas \\
      --prowl-binary '~/.cas-scratch/peer-collab/bin/cas'
Example (real cloud preview):
  LIVE_CHECK_TOKEN_A=... LIVE_CHECK_TOKEN_B=... scripts/live-check-peer-collab.py \\
      --binary ... --prowl-binary ... --cloud-url https://<preview>.vercel.app
"""

import argparse
import json
import os
import random
import re
import secrets
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import time
import traceback
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
HELPER_DIR = HERE / "live_check_peer_collab"

# Step -> the task whose client code it needs. A step is pending unless the
# preflight probe finds that task's feature in both binaries (cas-937a5 F03).
STEP_REQUIRES = {
    "peer_detail": "cas-559c",
    "spawn_refused": "cas-0078",
    "unknown_name": "cas-a19b",
    "epic_split": "cas-4736",
    "offline_catchup": "cas-13c7",
    "presence_recovery": "cas-f707",
}

STEP_ORDER = [
    "peers_discovery",
    "peer_detail",
    "claim_refused",
    "spawn_refused",
    "message_reply_status",
    "unknown_name",
    "no_route_to_peer_worker",
    "epic_split",
    "offline_catchup",
    "presence_recovery",
]


def utc():
    return datetime.now(timezone.utc).isoformat()


class StepFailed(Exception):
    pass


class ConfigError(Exception):
    """A usage or configuration problem found before anything is created (exit 2)."""


class Interrupted(BaseException):
    """SIGTERM or SIGHUP: like KeyboardInterrupt, it unwinds into teardown."""

    def __init__(self, signum):
        super().__init__(signal.Signals(signum).name)
        self.signum = signum


class StepSkipped(Exception):
    """The step cannot run in this configuration (not a pass, not a fail)."""


class Machine:
    """One host: soundwave runs commands locally, prowl over ssh."""

    def __init__(self, label, ssh_host, run_dir, binary, python):
        self.label = label
        self.ssh_host = ssh_host
        self.run_dir = run_dir
        self.binary = binary
        self.python = python
        self.helper = f"{run_dir}/harness.py"
        self.daemon_pid = None

    def sh(self, argv, input=None, timeout=180, check=True):
        if self.ssh_host:
            command = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15", self.ssh_host,
                       "--", shlex.join(argv)]
        else:
            command = argv
        result = subprocess.run(command, input=input, capture_output=True, text=True, timeout=timeout)
        if check and result.returncode != 0:
            raise RuntimeError(
                f"{self.label}: {' '.join(argv[:4])} exited {result.returncode}: {result.stderr[-1500:]}"
            )
        return result

    def call(self, *args, input=None, timeout=180):
        """Run a harness.py machine subcommand and parse its JSON line."""
        result = self.sh([self.python, self.helper, *args], input=input, timeout=timeout)
        lines = [line for line in result.stdout.splitlines() if line.strip()]
        if not lines:
            raise RuntimeError(f"{self.label}: {args[0]} printed nothing; stderr: {result.stderr[-800:]}")
        return json.loads(lines[-1])

    def put(self, local_path, remote_path):
        if self.ssh_host:
            subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(local_path),
                            f"{self.ssh_host}:{remote_path}"], check=True, timeout=120)
        else:
            shutil.copy2(local_path, remote_path)

    def ctl(self, agent, request, timeout=180):
        sock = f"{self.run_dir}/{agent}.sock"
        result = self.sh([self.python, self.helper, "ctl", sock, json.dumps(request)], timeout=timeout)
        return json.loads(result.stdout)

    def tool(self, agent, name, **arguments):
        return self.ctl(agent, {"op": "tool", "name": name, "arguments": arguments})

    def pane(self, agent):
        return self.ctl(agent, {"op": "pane"}).get("text", "")


class Run:
    def __init__(self, args):
        self.args = args
        # Short: macOS limits Unix socket paths to 104 bytes, and the factory's
        # socket lives at <run dir>/home/.cas/factory-<session>.gui.sock.
        self.rid = args.run_id or secrets.token_hex(3)
        self.stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        self.receipts = Path(args.receipts_dir or default_receipts_root()) / self.stamp
        self.receipts.mkdir(parents=True, exist_ok=True)
        self.steps = [s.strip() for s in args.steps.split(",")] if args.steps else STEP_ORDER
        unknown = [s for s in self.steps if s not in STEP_ORDER]
        if unknown:
            raise ConfigError(f"unknown step(s): {unknown}; known: {STEP_ORDER}")
        self.features = {}
        self.probes = {}
        self.machines = ()
        self._tokens = None
        self._torn_down = False
        self.canonical = args.canonical_id or f"github.com/petrastella-live-check/peer-collab-{self.rid}"
        self.remote = f"https://{self.canonical}.git"
        self.names = {
            "a": f"sup-soundwave-{self.rid}",
            "b": f"sup-prowl-{self.rid}",
            "bw": f"wkr-prowl-{self.rid}",
        }
        self.sessions = {"a": f"lc-{self.rid}-a", "b": f"lc-{self.rid}-b"}
        self.standin = None
        self.endpoint = args.cloud_url
        self.results = []
        self.ctx = {}
        self.versions = {}
        self.teardown_report = {}

    # ---- receipts -------------------------------------------------------

    def write(self, name, payload):
        path = self.receipts / name
        path.write_text(json.dumps(payload, indent=2, sort_keys=True, default=str) + "\n")
        return path

    def log(self, message):
        line = f"[{datetime.now().strftime('%H:%M:%S')}] {message}"
        print(line, flush=True)
        with open(self.receipts / "run.log", "a") as handle:
            handle.write(line + "\n")

    # ---- setup ----------------------------------------------------------

    def resolve_machines(self):
        a_dir = str(Path(self.args.soundwave_scratch) / f"live-check-{self.rid}" / "soundwave")
        prowl_home = subprocess.run(
            ["ssh", "-o", "BatchMode=yes", self.args.prowl_host, "echo $HOME"],
            capture_output=True, text=True, timeout=30, check=True,
        ).stdout.strip()
        prowl_scratch = self.args.prowl_scratch.replace("~", prowl_home, 1)
        b_dir = f"{prowl_scratch}/live-check-{self.rid}/prowl"
        prowl_python = self.args.prowl_python or subprocess.run(
            ["ssh", "-o", "BatchMode=yes", self.args.prowl_host,
             "PATH=/opt/homebrew/bin:$PATH command -v python3"],
            capture_output=True, text=True, timeout=30, check=True,
        ).stdout.strip()
        prowl_binary = self.args.prowl_binary.replace("~", prowl_home, 1)
        self.a = Machine("soundwave", None, a_dir, str(Path(self.args.binary).resolve()), sys.executable)
        self.b = Machine("prowl", self.args.prowl_host, b_dir, prowl_binary, prowl_python)
        self.machines = (self.a, self.b)

    def tokens(self):
        if self.args.cloud_url:
            def read(path_arg, env):
                if path_arg:
                    try:
                        value = Path(path_arg).read_text().strip()
                    except OSError as error:
                        raise ConfigError(f"cannot read token file {path_arg}: {error}") from error
                    if not value:
                        raise ConfigError(f"token file {path_arg} is empty")
                    return value
                value = os.environ.get(env, "").strip()
                if not value:
                    raise ConfigError(f"--cloud-url needs a device token: --token-file-a/-b or {env}")
                return value

            return read(self.args.token_file_a, "LIVE_CHECK_TOKEN_A"), read(
                self.args.token_file_b, "LIVE_CHECK_TOKEN_B"
            )
        return secrets.token_hex(24), secrets.token_hex(24)

    def start_standin(self, token_a, token_b):
        cloud_dir = Path(self.a.run_dir).parent / "standin-cloud"
        cloud_dir.mkdir(parents=True, exist_ok=True)
        tokens_file = cloud_dir / "tokens.txt"
        admin_file = cloud_dir / "admin.txt"
        for path, content in ((tokens_file, f"{token_a}\n{token_b}\n"), (admin_file, secrets.token_hex(24))):
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w") as handle:
                handle.write(content)
        bind = self.args.standin_bind or tailnet_ip()
        port = self.args.standin_port or free_port(bind)
        log = open(cloud_dir / "standin.log", "ab")
        proc = subprocess.Popen(
            [sys.executable, str(HELPER_DIR / "standin_cloud.py"), "--bind", bind, "--port", str(port),
             "--tokens-file", str(tokens_file), "--admin-token-file", str(admin_file),
             "--request-log", str(cloud_dir / "requests.jsonl")],
            stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True,
        )
        self.standin = {"proc": proc, "dir": cloud_dir, "admin": admin_file.read_text().strip(),
                        "url": f"http://{bind}:{port}"}
        self.endpoint = self.standin["url"]
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            try:
                with socket.create_connection((bind, port), timeout=1):
                    break
            except OSError:
                time.sleep(0.3)
        else:
            raise RuntimeError("stand-in cloud did not start; see standin.log")
        reach = self.b.sh(["curl", "-s", "-o", "/dev/null", "-w", "%{http_code}",
                           f"{self.endpoint}/api/agents"], check=False)
        self.log(f"stand-in cloud {self.endpoint}; prowl reach status {reach.stdout.strip() or reach.returncode}")
        if reach.stdout.strip() != "401":
            raise RuntimeError(f"prowl cannot reach the stand-in at {self.endpoint} (got {reach.stdout!r})")

    def standin_admin(self, method, path, body=None):
        import urllib.request

        request = urllib.request.Request(
            f"{self.endpoint}{path}", method=method,
            data=json.dumps(body).encode() if body is not None else None,
            headers={"Authorization": f"Bearer {self.standin['admin']}", "Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.loads(response.read())

    def preflight(self):
        """Everything that can be checked before the run creates anything:
        tokens, both binaries and what they can do (cas-937a5 F02, F03)."""
        self._tokens = self.tokens()
        local = Path(self.args.binary)
        if not (local.is_file() and os.access(local, os.X_OK)):
            raise ConfigError(f"--binary {local} is not an executable file")
        self.resolve_machines()
        for machine in self.machines:
            try:
                probe = machine_stream(machine, "probe", "--binary", machine.binary)
            except Exception as error:
                raise ConfigError(f"{machine.label}: cannot probe {machine.binary}: {error}") from error
            if not probe.get("ok") or not probe.get("commit"):
                raise ConfigError(f"{machine.label}: {machine.binary} did not report a version: {probe}")
            self.probes[machine.label] = probe
        commits = {label: probe["commit"] for label, probe in self.probes.items()}
        mixed = len(set(commits.values())) > 1
        if mixed and not self.args.allow_mixed:
            raise ConfigError(
                f"the binaries are at different commits ({', '.join(f'{k} {v}' for k, v in commits.items())}); "
                "rebuild both at one commit, or pass --allow-mixed")
        self.features = {
            feature: all(probe["features"].get(feature) for probe in self.probes.values())
            for feature in set(STEP_REQUIRES.values())
        }
        self.write("preflight.json", {"probes": self.probes, "mixed": mixed,
                                      "allow_mixed": self.args.allow_mixed, "features": self.features})
        self.log(f"preflight: {', '.join(f'{k} {v}' for k, v in commits.items())}"
                 f"{' (mixed, allowed)' if mixed else ''}; features present on both: "
                 f"{sorted(k for k, v in self.features.items() if v) or 'none'}")

    def setup(self):
        for machine in self.machines:
            machine.versions_before = machine_call_installed(machine)
        self.versions["installed_before"] = {m.label: m.versions_before for m in self.machines}
        token_a, token_b = self._tokens
        for machine in self.machines:
            machine.sh(["mkdir", "-p", machine.run_dir])
            machine.put(HELPER_DIR / "harness.py", machine.helper)
        if not self.args.cloud_url:
            self.start_standin(token_a, token_b)
        for machine, token in ((self.a, token_a), (self.b, token_b)):
            report = machine.call(
                "setup", "--run-dir", machine.run_dir, "--binary", machine.binary,
                "--remote", self.remote, "--canonical-id", self.canonical, "--endpoint", self.endpoint,
                input=token + "\n", timeout=240,
            )
            machine.binary_version = report["binary_version"]
            self.log(f"{machine.label}: scratch project ready at {machine.run_dir} ({machine.binary_version})")
        self.versions["run_binaries"] = {m.label: {"path": m.binary, "version": m.binary_version}
                                         for m in self.machines}
        self.launch(self.a, self.sessions["a"], self.names["a"], [])
        self.launch(self.b, self.sessions["b"], self.names["b"], [self.names["bw"]])

    def launch(self, machine, session, supervisor, workers):
        args = ["launch", "--run-dir", machine.run_dir, "--session", session, "--supervisor-name", supervisor]
        if self.args.workaround_eager_registration:
            args.append("--serve-without-session-id")
        for worker in workers:
            args += ["--worker-name", worker]
        machine.daemon_pid = machine.call(*args)["pid"]
        for agent in [supervisor, *workers]:
            self.wait_socket(machine, agent)
        self.log(f"{machine.label}: factory {session} up (daemon pid {machine.daemon_pid}; {supervisor} {workers})")

    def wait_socket(self, machine, agent, timeout=120):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            try:
                reply = machine.ctl(agent, {"op": "ping"}, timeout=40)
                if reply.get("ok"):
                    return reply
            except Exception as error:  # socket not up yet
                last = error
            time.sleep(2)
        raise RuntimeError(f"{machine.label}: {agent} control socket never answered: {last}")

    # ---- steps ----------------------------------------------------------

    def run_steps(self):
        for index, step in enumerate(self.steps, 1):
            requires = STEP_REQUIRES.get(step)
            pending = requires is not None and not self.features.get(requires, False)
            receipt = {"step": step, "index": index, "requires": requires, "started_at": utc(),
                       "evidence": {}, "notes": []}
            if requires:
                receipt["probe"] = {label: probe["features"].get(requires)
                                    for label, probe in self.probes.items()}
            if self.args.workaround_eager_registration:
                receipt["notes"].append(
                    "DIAGNOSTIC RUN: --workaround-eager-registration started cas serve without "
                    "CAS_SESSION_ID; a real harness passes it, so this is not a faithful pass")
            if pending and not self.args.try_pending:
                receipt.update(status="pending",
                               detail=f"{requires} client code is not in both binaries "
                                      f"(probe {receipt['probe']}); not executed")
            else:
                try:
                    getattr(self, f"step_{step}")(receipt["evidence"], receipt["notes"])
                    receipt["status"] = "pending" if pending else "pass"
                    if pending:
                        receipt["detail"] = f"{requires} not in both binaries: executed for observation only, never a pass"
                        receipt["observed"] = "would pass"
                except StepSkipped as skipped:
                    receipt["status"] = "pending" if pending else "skipped"
                    receipt["detail"] = str(skipped)
                except StepFailed as failure:
                    receipt["status"] = "pending" if pending else "fail"
                    receipt["detail"] = str(failure)
                    if pending:
                        receipt["observed"] = f"fails as expected without {requires}: {failure}"
                except Exception as error:
                    receipt["status"] = "pending" if pending else "error"
                    receipt["detail"] = f"{type(error).__name__}: {error}"
                    receipt["traceback"] = traceback.format_exc()[-3000:]
            receipt["finished_at"] = utc()
            self.write(f"{index:02d}-{step}.json", receipt)
            self.results.append(receipt)
            self.log(f"step {index:02d} {step}: {receipt['status'].upper()} {receipt.get('detail', '')}")

    def wait_for(self, what, probe, timeout, interval=3):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            ok, last = probe()
            if ok:
                return last
            time.sleep(interval)
        raise StepFailed(f"{what} not seen within {timeout}s; last: {str(last)[:600]}")

    def peers_text(self, machine, agent):
        reply = machine.tool(agent, "coordination", action="peers")
        return reply.get("text", "")

    def step_peers_discovery(self, evidence, notes):
        a, b = self.names["a"], self.names["b"]

        def probe():
            a_view = self.peers_text(self.a, a)
            b_view = self.peers_text(self.b, b)
            evidence["soundwave_peers"], evidence["prowl_peers"] = a_view, b_view
            return (b in a_view and a in b_view), {"soundwave": a_view, "prowl": b_view}

        self.wait_for("each supervisor listing the other as a peer", probe, self.args.discovery_timeout)
        if "prowl" not in evidence["soundwave_peers"]:
            notes.append("soundwave's peer line does not name machine 'prowl'")

    def step_peer_detail(self, evidence, notes):
        reply = self.a.tool(self.names["a"], "coordination", action="peers", peer=self.names["b"], detail=True)
        evidence["reply"] = reply
        text = reply.get("text", "")
        missing = [word for word in ("focus", "claim", "worker", "branch") if word not in text.lower()]
        if reply.get("is_error") or missing:
            raise StepFailed(f"peer detail lacks {missing or 'a successful reply'}")
        if self.names["bw"] not in text:
            raise StepFailed("peer detail does not list prowl's worker")

    def create_task(self, evidence, key, task_type="task", title=None, epic=None):
        title = title or f"live-check {self.rid} {key}"
        extra = {"epic": epic} if epic else {}
        created = self.a.tool(self.names["a"], "task", action="create", title=title, task_type=task_type,
                              priority=3, risk="none", description="live check scratch task", **extra)
        evidence[f"create_{key}"] = created
        match = re.search(r"\b(cas-[0-9a-f]{4,})\b", created.get("text", ""))
        if created.get("is_error") or not match:
            raise StepFailed(f"task create ({key}) failed: {created.get('text', '')[:300]}")
        return match.group(1)

    def replicate(self, evidence, ids):
        """Copy task rows (and the dependency rows among them) from soundwave
        to prowl: the stand-in has no sync; on the real cloud this still pins
        the ids."""
        exported = self.a.call("task-export", "--run-dir", self.a.run_dir, "--task-id", ",".join(ids))
        if not exported["ok"]:
            raise StepFailed(f"not found in soundwave's store: {exported['missing']}")
        imported = self.b.call("task-import", "--run-dir", self.b.run_dir,
                               input=json.dumps({"tasks": exported["tasks"],
                                                 "dependencies": exported["dependencies"]}))
        evidence.setdefault("replicated", []).append(
            {"ids": ids, "dependencies": imported["dependencies"],
             "method": "runner sqlite copy soundwave->prowl"})

    def shared_task(self, evidence, task_type="task", title=None):
        task_id = self.create_task(evidence, task_type, task_type=task_type, title=title)
        self.replicate(evidence, [task_id])
        return task_id

    def step_claim_refused(self, evidence, notes):
        # Supervisors may not `task start` a non-epic task; `task action=claim`
        # is their lease, and it claims in the cloud the same way (cas-5f28).
        task_id = self.shared_task(evidence)
        self.ctx["task_id"] = task_id
        claimed = self.a.tool(self.names["a"], "task", action="claim", id=task_id)
        evidence["soundwave_claim"] = claimed
        if claimed.get("is_error"):
            raise StepFailed(f"soundwave could not claim {task_id}: {claimed.get('text', '')[:300]}")
        refused = self.b.tool(self.names["b"], "task", action="claim", id=task_id)
        evidence["prowl_claim"] = refused
        text = refused.get("text", "")
        if self.standin:
            state = self.standin_admin("GET", "/__standin/state")
            evidence["cloud_locks"] = {k: v for k, v in state["locks"].items() if k.startswith(task_id)}
        if "HELD BY A PEER" not in text or self.names["a"] not in text:
            raise StepFailed(f"prowl's claim of {task_id} was not refused naming {self.names['a']}: {text[:400]}")

    def step_spawn_refused(self, evidence, notes):
        task_id = self.ctx.get("task_id") or self.shared_task(evidence)
        before = self.b.call("spawn-count", "--run-dir", self.b.run_dir)["spawn_queue_rows"]
        reply = self.b.tool(self.names["b"], "factory", action="spawn_workers", count=1, task_id=task_id)
        after = self.b.call("spawn-count", "--run-dir", self.b.run_dir)["spawn_queue_rows"]
        evidence.update(reply=reply, spawn_queue_before=before, spawn_queue_after=after)
        if after != before:
            raise StepFailed(f"spawn was queued ({before} -> {after} rows) for a task a peer holds")
        if not reply.get("is_error") and "HELD BY A PEER" not in reply.get("text", ""):
            raise StepFailed("spawn_workers was not refused")

    def message(self, machine, agent, target, summary, body, **extra):
        return machine.tool(agent, "coordination", action="message", target=target, summary=summary,
                            message=body, **extra)

    def queued_rows(self, machine, token):
        return machine.call("queue-find", "--run-dir", machine.run_dir, "--token", token)["rows"]

    def delivered_locally(self, machine, agent, sender, token, evidence_key, evidence):
        """Wait until the recipient machine's daemon has claimed `token` from
        the cloud mailbox and enqueued it for `agent` (the peer-mailbox
        delivery contract); return the local notification id.

        Whether the factory then types it into the pane depends on the
        harness looking idle (pane silence and transcript evidence that only a
        real Claude Code produces), so the inbox and pane are recorded as
        evidence only."""
        def probe():
            rows = [row for row in self.queued_rows(machine, token)
                    if row.get("target") == agent and str(row.get("source", "")).startswith(f"peer:{sender}")]
            return bool(rows), rows

        rows = self.wait_for(f"{token} in {machine.label}'s queue for {agent}", probe,
                             self.args.delivery_timeout)
        evidence[evidence_key] = rows
        inbox = machine.tool(agent, "coordination", action="inbox_poll", notification_id=rows[0]["id"])
        evidence[f"{evidence_key}_inbox_poll"] = {"has_token": token in inbox.get("text", ""),
                                                  "text": inbox.get("text", "")[:600]}
        evidence[f"{evidence_key}_in_pane"] = token in machine.pane(agent)
        return rows[0]["id"]

    def step_message_reply_status(self, evidence, notes):
        a, b = self.names["a"], self.names["b"]
        ping = f"live-check-ping-{self.rid}-{secrets.token_hex(3)}"
        sent = self.message(self.a, a, b, f"ping {ping}", f"{ping}: please reply")
        evidence["sent"] = sent
        peer_message = peer_message_id(sent.get("text", ""))
        if sent.get("is_error") or not peer_message or "Cassy Cloud" not in sent.get("text", ""):
            raise StepFailed(f"the message did not go via Cassy Cloud: {sent.get('text', '')[:400]}")
        evidence["peer_message_id"] = peer_message
        notification = self.delivered_locally(self.b, b, a, ping, "prowl_delivery", evidence)
        pong = f"live-check-pong-{self.rid}-{secrets.token_hex(3)}"
        reply = self.message(self.b, b, a, f"pong {pong}", f"{pong}: reply to {ping}",
                             in_reply_to=notification)
        evidence["reply"] = reply
        if reply.get("is_error") or "Cassy Cloud" not in reply.get("text", ""):
            raise StepFailed(f"the reply did not go via Cassy Cloud: {reply.get('text', '')[:400]}")
        self.delivered_locally(self.a, a, b, pong, "soundwave_delivery", evidence)

        def delivered():
            status = self.a.tool(a, "coordination", action="message_status", id=peer_message)
            evidence["message_status"] = status
            return "delivered" in status.get("text", "").lower(), status

        self.wait_for(f"{peer_message} delivered", delivered, 60)

    def step_unknown_name(self, evidence, notes):
        target = f"nobody-{self.rid}"
        reply = self.message(self.a, self.names["a"], target, "unknown target", "live check")
        evidence["reply"] = reply
        text = reply.get("text", "")
        queued = re.search(r"\bqueued\b|will deliver", text, re.IGNORECASE)
        if not reply.get("is_error") or queued:
            raise StepFailed(f"a message to unknown {target} was not an immediate error: {text[:300]}")

    def step_no_route_to_peer_worker(self, evidence, notes):
        worker = self.names["bw"]
        a = self.names["a"]
        peers = self.peers_text(self.a, a)
        evidence["soundwave_peers"] = peers
        before = self.b.call("spawn-count", "--run-dir", self.b.run_dir)["spawn_queue_rows"]
        marker = f"live-check-worker-{self.rid}-{secrets.token_hex(3)}"
        reply = self.message(self.a, a, worker, f"to peer worker {marker}", marker)
        evidence["reply"] = reply
        time.sleep(self.args.worker_quiet_secs)
        worker_pane = self.b.pane(worker)
        after = self.b.call("spawn-count", "--run-dir", self.b.run_dir)["spawn_queue_rows"]
        evidence.update(spawn_queue_before=before, spawn_queue_after=after,
                        worker_pane_has_marker=marker in worker_pane)
        problems = []
        if worker in peers:
            problems.append(f"soundwave lists prowl's worker {worker} as a peer")
        if "Cassy Cloud" in reply.get("text", ""):
            problems.append("the message to prowl's worker was routed via Cassy Cloud")
        if marker in worker_pane:
            problems.append("prowl's worker received the message")
        if after != before:
            problems.append(f"prowl's spawn queue changed ({before} -> {after})")
        if self.standin:
            state = self.standin_admin("GET", "/__standin/state")
            worker_ids = {agent["id"] for agent in state["agents"] if agent.get("name") == worker}
            routed = [m["id"] for m in state["messages"] if m["recipient_agent_id"] in worker_ids]
            evidence["cloud_messages_to_worker"] = routed
            if routed:
                problems.append(f"the cloud holds messages for the worker: {routed}")
        if problems:
            raise StepFailed("; ".join(problems))

    def step_epic_split(self, evidence, notes):
        """cas-4736's `factory action=epic_split`: propose `id=<epic>
        target=<peer> mine= theirs=`; the peer answers `id=<split>
        decision=accept`; the proposer may `decision=cancel`; no arguments
        lists. Each side then refuses the other's share."""
        a, b = self.names["a"], self.names["b"]
        epic = self.create_task(evidence, "epic", task_type="epic", title=f"live-check {self.rid} split epic")
        kids = [self.create_task(evidence, f"child{i}", epic=epic) for i in range(1, 7)]
        self.replicate(evidence, [epic, *kids])
        mine, theirs = kids[0:2], kids[2:4]

        def split_id(reply):
            found = re.search(r"\b(split-[0-9a-f]{8})\b", reply.get("text", ""))
            return found.group(1) if found else None

        proposed = self.a.tool(a, "factory", action="epic_split", id=epic, target=b,
                               mine=",".join(mine), theirs=",".join(theirs))
        evidence["propose"] = proposed
        split = split_id(proposed)
        if proposed.get("is_error") or not split:
            raise StepFailed(f"propose failed: {proposed.get('text', '')[:400]}")

        def in_queue(machine, recipient):
            def probe():
                rows = [r for r in self.queued_rows(machine, split) if r.get("target") == recipient]
                return bool(rows), rows
            return probe

        evidence["proposal_delivered"] = self.wait_for(
            f"{split} proposal in prowl's queue", in_queue(self.b, b), self.args.delivery_timeout)
        accepted = self.b.tool(b, "factory", action="epic_split", id=split, decision="accept")
        evidence["accept"] = accepted
        if accepted.get("is_error") or "accepted epic split" not in accepted.get("text", "").lower():
            raise StepFailed(f"accept failed: {accepted.get('text', '')[:400]}")
        evidence["answer_delivered"] = self.wait_for(
            f"{split} answer in soundwave's queue", in_queue(self.a, a), self.args.delivery_timeout)

        def listed_accepted():
            listing = self.a.tool(a, "factory", action="epic_split")
            line = next((l for l in listing.get("text", "").splitlines() if split in l), "")
            return "accepted" in line.lower(), listing

        evidence["list"] = self.wait_for(f"{split} listed accepted on soundwave", listed_accepted, 60)
        gate = self.b.tool(b, "task", action="claim", id=mine[0])
        evidence["prowl_claims_soundwaves_share"] = gate
        if split not in gate.get("text", ""):
            raise StepFailed(f"prowl's claim of {mine[0]} (soundwave's share) was not refused "
                             f"naming {split}: {gate.get('text', '')[:300]}")
        second = self.a.tool(a, "factory", action="epic_split", id=epic, target=b,
                             mine=kids[4], theirs=kids[5])
        evidence["propose_second"] = second
        split2 = split_id(second)
        if second.get("is_error") or not split2:
            raise StepFailed(f"second proposal failed: {second.get('text', '')[:300]}")
        cancelled = self.a.tool(a, "factory", action="epic_split", id=split2, decision="cancel")
        evidence["cancel"] = cancelled
        if cancelled.get("is_error") or "cancelled epic split" not in cancelled.get("text", "").lower():
            raise StepFailed(f"cancel failed: {cancelled.get('text', '')[:300]}")

    def restart_prowl(self, evidence, key):
        swept = self.b.call("sweep", "--run-dir", self.b.run_dir, "--kill")
        evidence[f"{key}_stop"] = {"stopped": len(swept["found"]), "left": swept["left"]}
        self.sessions["b"] = f"{self.sessions['b']}r"
        return swept

    def set_token(self, machine, valid):
        token = self._tokens[0 if machine is self.a else 1]
        if valid:
            return machine.call("cloud-token", "--run-dir", machine.run_dir, input=token + "\n")
        return machine.call("cloud-token", "--run-dir", machine.run_dir, "--invalid")

    def line_in(self, machine, agent, prefix):
        text = self.peers_text(machine, agent)
        return next((l for l in text.splitlines() if l.startswith(prefix)), ""), text

    def step_offline_catchup(self, evidence, notes):
        """prowl's supervisor keeps its identity while its mailbox is offline
        (its cloud token is swapped for an invalid one); soundwave sends two
        messages; once prowl is back online each lands exactly once, in order.
        A full factory restart is not used: it gives the supervisor a new
        agent id, so messages addressed to the old one are not its to claim."""
        a, b = self.names["a"], self.names["b"]
        self.set_token(self.b, valid=False)

        def offline():
            line, _ = self.line_in(self.b, b, "Peer mailbox:")
            return line.startswith("Peer mailbox: not receiving"), line

        evidence["prowl_mailbox_offline"] = self.wait_for("prowl's mailbox not receiving", offline, 90, interval=5)
        tokens = [f"live-check-offline-{self.rid}-{i}-{secrets.token_hex(2)}" for i in (1, 2)]
        sent = [self.message(self.a, a, b, f"offline {token}", token) for token in tokens]
        evidence["sent_while_offline"] = sent
        ids = [peer_message_id(s_.get("text", "")) for s_ in sent]
        if any(s_.get("is_error") for s_ in sent) or not all(ids):
            raise StepFailed(f"sends while prowl was offline failed: {[s_.get('text', '')[:200] for s_ in sent]}")
        evidence["status_while_offline"] = [
            self.a.tool(a, "coordination", action="message_status", id=m).get("text", "") for m in ids]
        time.sleep(20)  # a mailbox poll while offline: nothing may arrive
        early = {token: len([r for r in self.queued_rows(self.b, token) if r.get("target") == b]) for token in tokens}
        evidence["queued_while_offline"] = early
        if any(early.values()):
            raise StepFailed(f"messages arrived while prowl's mailbox was offline: {early}")
        self.set_token(self.b, valid=True)

        def rows():
            return {token: [r for r in self.queued_rows(self.b, token) if r.get("target") == b] for token in tokens}

        def caught_up():
            current = rows()
            return all(current.values()), {k: len(v) for k, v in current.items()}

        self.wait_for("both offline messages queued for prowl's supervisor", caught_up,
                      self.args.delivery_timeout + 60)
        time.sleep(20)  # one more mailbox poll: a duplicate would land now
        final = rows()
        evidence["queued_after"] = final
        counts = {token: len(found) for token, found in final.items()}
        if any(count != 1 for count in counts.values()):
            raise StepFailed(f"offline messages were not delivered exactly once: {counts}")
        order = [final[token][0]["id"] for token in tokens]
        if order != sorted(order):
            raise StepFailed(f"offline messages arrived out of order: rows {order}")
        evidence["prowl_mailbox_after"], _ = self.line_in(self.b, b, "Peer mailbox:")
        evidence["status_after"] = [
            self.a.tool(a, "coordination", action="message_status", id=m).get("text", "") for m in ids]

    def step_presence_recovery(self, evidence, notes):
        """prowl's factory restarts with an invalid cloud token, so its
        registration fails; once the token is valid again, cas-f707 presence
        re-registers it and soundwave sees the new session live."""
        b = self.names["b"]
        self.restart_prowl(evidence, "prowl")
        self.set_token(self.b, valid=False)
        self.launch(self.b, self.sessions["b"], b, [self.names["bw"]])

        def failing():
            line, _ = self.line_in(self.b, b, "Cloud presence:")
            return "failing" in line or "not registered" in line, line

        evidence["presence_while_failing"] = self.wait_for(
            "prowl's cloud presence failing", failing, 90, interval=5)
        self.set_token(self.b, valid=True)
        session = self.sessions["b"]

        def listed():
            view = self.peers_text(self.a, self.names["a"])
            line = next((l for l in view.splitlines()
                         if b in l and f"session {session} " in l + " "), "")
            return bool(line) and "[live]" in line, view

        evidence["soundwave_peers"] = self.wait_for(
            "prowl's restarted supervisor live again after failed registrations", listed, 400, interval=10)
        evidence["presence_after"], _ = self.line_in(self.b, b, "Cloud presence:")

    # ---- cloud cleanup ---------------------------------------------------

    def cloud_api(self, token, method, path, body=None):
        import urllib.error
        import urllib.request
        request = urllib.request.Request(
            self.endpoint + path, method=method,
            data=json.dumps(body).encode() if body is not None else None,
            headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=20) as response:
                raw = response.read()
                return response.status, json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            with error:
                return error.code, error.read().decode(errors="replace")[:300]
        except Exception as error:
            return None, f"{type(error).__name__}: {error}"

    def cloud_cleanup(self):
        """Leave nothing in the cloud: drain messages queued for any scratch
        agent, shut every scratch agent down (which releases its claims), and
        confirm the scratch project lists no active agent."""
        if not self._tokens or not self.endpoint:
            return {"skipped": "no cloud configured"}
        report = {"drained": [], "shutdown": []}
        for index, machine in enumerate(self.machines):
            token = self._tokens[index]
            try:
                ids = [agent["id"] for agent in machine.call("agent-ids", "--run-dir", machine.run_dir)["agents"]]
            except Exception as error:
                ids, report[f"{machine.label}_ids_error"] = [], str(error)
            if ids:
                consumer = f"live-check-cleanup-{self.rid}-{machine.label}"
                code, claimed = self.cloud_api(token, "POST", "/api/peer-messages/claim", {
                    "project_ids": [self.canonical], "recipient_agent_ids": ids[:50],
                    "consumer_id": consumer, "max": 50, "lease_secs": 30})
                messages = claimed.get("messages", []) if isinstance(claimed, dict) else []
                if messages:
                    self.cloud_api(token, "POST", "/api/peer-messages/ack", {
                        "consumer_id": consumer,
                        "acks": [{"id": m["id"], "outcome": "delivered"} for m in messages]})
                report["drained"] += [m["id"] for m in messages]
            for agent_id in ids:
                code, body = self.cloud_api(token, "POST", f"/api/agents/{agent_id}/shutdown", {})
                report["shutdown"].append({"machine": machine.label, "id": agent_id, "status": code})
        code, listed = self.cloud_api(self._tokens[0], "GET",
                                      f"/api/agents?project={self.canonical}&limit=100")
        agents = listed.get("agents", []) if isinstance(listed, dict) else []
        report["project_agents_after"] = [(a.get("name"), a.get("status")) for a in agents]
        report["active_left"] = [a.get("name") for a in agents if a.get("status") not in ("shutdown",)]
        report["list_status"] = code
        return report

    # ---- teardown -------------------------------------------------------

    def teardown(self):
        """Idempotent; ignores further SIGINT/SIGTERM/SIGHUP so a second
        signal cannot abort cleanup half way (cas-937a5 F01)."""
        if self._torn_down:
            return self.teardown_report
        self._torn_down = True
        for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            try:
                signal.signal(signum, signal.SIG_IGN)
            except (ValueError, OSError):
                pass
        report = {}
        for machine in getattr(self, "machines", ()):
            entry = {}
            try:
                swept = machine.call("sweep", "--run-dir", machine.run_dir, "--kill", timeout=120)
                entry["processes_stopped"] = len(swept["found"])
                entry["processes_left"] = swept["left"]
            except Exception as error:
                entry["error"] = f"{type(error).__name__}: {error}"
            report[machine.label] = entry
        try:
            report["cloud"] = self.cloud_cleanup()
        except Exception as error:
            report["cloud"] = {"error": f"{type(error).__name__}: {error}"}
        if self.standin:
            proc = self.standin["proc"]
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)
            log = self.standin["dir"] / "requests.jsonl"
            if log.exists():
                shutil.copy2(log, self.receipts / "standin-requests.jsonl")
            report["standin"] = {"stopped": proc.poll() is not None}
        for machine in getattr(self, "machines", ()):
            entry = report[machine.label]
            # The run root holds this machine's dir (and, on soundwave, the
            # stand-in's); nothing else is ever created.
            root = str(Path(machine.run_dir).parent)
            if self.args.keep:
                entry["run_root"] = {"kept": root}
                continue
            try:
                entry["run_root"] = machine.call("remove", "--run-dir", root)
            except Exception as error:
                entry["run_root"] = {"ok": False, "error": f"{type(error).__name__}: {error}"}
        for machine in getattr(self, "machines", ()):
            try:
                after = machine_call_installed(machine)
                before = getattr(machine, "versions_before", None)
                report[machine.label]["installed_cas"] = {
                    "before": before, "after": after,
                    "unchanged": bool(before) and before.get("sha256") == after.get("sha256")
                    and before.get("version") == after.get("version"),
                }
            except Exception as error:
                report.setdefault(machine.label, {})["installed_cas_error"] = str(error)
        self.teardown_report = report
        self.write("teardown.json", report)
        return report


def peer_message_id(text):
    """The cloud id in a send receipt: production uses UUIDs, the stand-in
    `pm_N` ("... via Cassy Cloud (peer message <id>, queued) ...")."""
    found = re.search(r"peer message ([0-9A-Za-z_-]+)", text)
    return found.group(1) if found else None


def machine_stream(machine, *args, timeout=180):
    """Run a harness.py subcommand on `machine` with this checkout's copy
    streamed over stdin (works before the run directory exists)."""
    script = (HELPER_DIR / "harness.py").read_text()
    result = machine.sh([machine.python, "-", *args], input=script, timeout=timeout)
    return json.loads(result.stdout.strip().splitlines()[-1])


def machine_call_installed(machine):
    """Installed cas fingerprint (version and sha256)."""
    return machine_stream(machine, "installed-cas")


def tailnet_ip():
    result = subprocess.run(["tailscale", "ip", "-4"], capture_output=True, text=True)
    address = result.stdout.strip().splitlines()[0] if result.returncode == 0 and result.stdout.strip() else ""
    if not address:
        raise ConfigError("no tailnet address; pass --standin-bind")
    return address


def free_port(bind):
    for _ in range(50):
        port = random.randint(18000, 28999)
        with socket.socket() as probe:
            try:
                probe.bind((bind, port))
                return port
            except OSError:
                continue
    raise RuntimeError("no free port for the stand-in")


def default_receipts_root():
    roots = sorted(Path.home().glob(".cas/artifacts/cassy-*"))
    if len(roots) != 1:
        raise ConfigError("cannot pick the artifacts root; pass --receipts-dir")
    return roots[0] / "cas-8ca9" / "live"


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="\n\n".join(__doc__.split("\n\n")[1:]),
    )
    parser.add_argument("--binary", required=True, help="epic-tip cas for soundwave (never built here)")
    parser.add_argument("--prowl-binary", required=True, help="epic-tip cas, a path on prowl")
    parser.add_argument("--prowl-host", default="prowl")
    parser.add_argument("--prowl-python", help="python3 on prowl (default: Homebrew's)")
    parser.add_argument("--soundwave-scratch", default="/mnt/rewind/cas/scratch")
    parser.add_argument("--prowl-scratch", default="~/.cas-scratch")
    parser.add_argument("--cloud-url", help="real cloud base URL; omit to start the local stand-in")
    parser.add_argument("--token-file-a", help="soundwave device token file (else LIVE_CHECK_TOKEN_A)")
    parser.add_argument("--token-file-b", help="prowl device token file (else LIVE_CHECK_TOKEN_B)")
    parser.add_argument("--standin-bind", help="stand-in bind address (default: this host's tailnet IPv4)")
    parser.add_argument("--standin-port", type=int)
    parser.add_argument("--canonical-id", help="shared repo identity (default: a fresh per-run id)")
    parser.add_argument("--allow-mixed", action="store_true",
                        help="run even when the two binaries report different commits")
    parser.add_argument("--try-pending", action="store_true",
                        help="also execute pending steps and record what they do (still reported pending)")
    parser.add_argument("--steps", help=f"comma subset of: {','.join(STEP_ORDER)}")
    parser.add_argument("--receipts-dir", help="default: <artifacts root>/cas-8ca9/live")
    parser.add_argument("--run-id")
    parser.add_argument("--keep", action="store_true", help="stop processes but keep scratch dirs")
    parser.add_argument(
        "--workaround-eager-registration", action="store_true",
        help="DIAGNOSTIC, not faithful: start the harness's `cas serve` without CAS_SESSION_ID so "
             "SessionStart registers the agent in the cloud. Use only to exercise later steps while "
             "the eager-registration gap blocks cloud presence; recorded in every receipt.")
    parser.add_argument("--discovery-timeout", type=int, default=90)
    parser.add_argument("--delivery-timeout", type=int, default=90)
    parser.add_argument("--worker-quiet-secs", type=int, default=35)
    return parser.parse_args(argv)


def install_signal_handlers():
    """SIGTERM and SIGHUP unwind like SIGINT, so `finally: teardown()` runs."""
    def handler(signum, _frame):
        raise Interrupted(signum)

    for signum in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(signum, handler)


def main(argv=None):
    args = parse_args(argv)
    install_signal_handlers()
    try:
        run = Run(args)
    except ConfigError as error:
        print(f"live-check: {error}", file=sys.stderr)
        return 2
    run.log(f"run {run.rid}: receipts {run.receipts}")
    try:
        run.preflight()
    except ConfigError as error:
        run.log(f"configuration error: {error}")
        run.write("summary.json", {"run_id": run.rid, "config_error": str(error),
                                   "probes": run.probes, "finished_at": utc()})
        return 2
    except (Interrupted, KeyboardInterrupt) as stop:
        run.log(f"interrupted during preflight: {stop}; nothing was created")
        return 128 + getattr(stop, "signum", signal.SIGINT)
    setup_error = None
    stopped_by = None
    try:
        run.setup()
        run.run_steps()
    except Interrupted as stop:
        setup_error, stopped_by = f"interrupted by {stop}", stop.signum
        run.log(f"{setup_error}: tearing down")
    except KeyboardInterrupt:
        setup_error, stopped_by = "interrupted by SIGINT", signal.SIGINT
        run.log(f"{setup_error}: tearing down")
    except Exception as error:
        setup_error = f"{type(error).__name__}: {error}"
        run.log(f"setup failed: {setup_error}")
        run.write("setup-error.json", {"error": setup_error, "traceback": traceback.format_exc()})
    finally:
        teardown = run.teardown()
    statuses = {r["step"]: r["status"] for r in run.results}
    clean = all(
        not entry.get("processes_left") and not entry.get("error")
        and (args.keep or entry.get("run_root", {}).get("ok"))
        for label, entry in teardown.items() if label in ("soundwave", "prowl")
    ) and not teardown.get("cloud", {}).get("active_left") and not teardown.get("cloud", {}).get("error")
    unchanged = all(teardown.get(label, {}).get("installed_cas", {}).get("unchanged")
                    for label in ("soundwave", "prowl"))
    summary = {
        "run_id": run.rid,
        "endpoint": run.endpoint,
        "cloud": "real" if args.cloud_url else "stand-in",
        "canonical_id": run.canonical,
        "agents": run.names,
        "binaries": run.versions.get("run_binaries"),
        "steps": statuses,
        "passed": [s for s, v in statuses.items() if v == "pass"],
        "failed": [s for s, v in statuses.items() if v in ("fail", "error")],
        "skipped": [s for s, v in statuses.items() if v == "skipped"],
        "pending": {s: STEP_REQUIRES.get(s) for s, v in statuses.items() if v == "pending"},
        "probes": run.probes,
        "features_on_both": run.features,
        "setup_error": setup_error,
        "diagnostic_workaround_eager_registration": args.workaround_eager_registration,
        "teardown_clean": clean,
        "installed_cas_unchanged": unchanged,
        "finished_at": utc(),
    }
    run.write("summary.json", summary)
    run.log(json.dumps({k: summary[k] for k in ("steps", "pending", "teardown_clean",
                                                 "installed_cas_unchanged", "setup_error")}))
    if stopped_by:
        return 128 + int(stopped_by)
    if setup_error or not clean or not unchanged:
        return 2
    return 1 if summary["failed"] else 0


if __name__ == "__main__":
    sys.exit(main())
