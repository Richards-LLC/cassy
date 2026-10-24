#!/usr/bin/env python3
"""many-agent-load.py fixtures (factory loop v2 C6, cas-9db0b).

No `cas` binary, daemon or database: the synthetic writer runs in a real
pseudo-terminal, and the key-to-echo probe talks to a fake daemon GUI socket.
"""
import argparse
import hashlib
import importlib.util
import os
import pty
import select
import signal
import socket
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("load", ROOT / "scripts/many-agent-load.py")
load = importlib.util.module_from_spec(spec)
spec.loader.exec_module(load)


def stub_dir(rates):
    """A fake bin dir laid out as prepare_run lays it out."""
    tmp = tempfile.TemporaryDirectory()
    fake_bin = Path(tmp.name)
    help_file, version_file = fake_bin / ".claude--help", fake_bin / ".claude--version"
    help_file.write_text("claude help\n")
    version_file.write_text("claude 9.9 (stub)\n")
    stub = fake_bin / "claude"
    stub.write_text(load.stub_script(help_file, version_file, fake_bin, rates))
    stub.chmod(0o755)
    if rates:
        (fake_bin / "writer-rates").write_text("".join(f"{r:g}\n" for r in rates))
        (fake_bin / "writer-slots").mkdir()
        (fake_bin / "synthetic-writer.py").write_text(load.SYNTHETIC_WRITER)
    return tmp, stub


def run_in_pty(stub, role, seconds):
    """Run the stub as a pane would (a terminal on stdin); return its output."""
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["CAS_AGENT_ROLE"] = role
        os.environ["CAS_AGENT_NAME"] = f"{role}-pane"
        os.execv(str(stub), [str(stub), "--dangerously-skip-permissions"])
    out = bytearray()
    deadline = time.monotonic() + seconds
    try:
        while time.monotonic() < deadline:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if ready:
                try:
                    out.extend(os.read(fd, 65536))
                except OSError:
                    break
    finally:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
        os.close(fd)
    return bytes(out)


class Rates(unittest.TestCase):
    def test_parse_rates(self):
        self.assertEqual(load.parse_rates("38,63,236"), [38.0, 63.0, 236.0])
        for bad in ("", "0", "38,-1", "fast"):
            with self.assertRaises(argparse.ArgumentTypeError):
                load.parse_rates(bad)


class StubAgent(unittest.TestCase):
    def test_probes_answer_without_a_terminal(self):
        tmp, stub = stub_dir([38.0])
        with tmp:
            import subprocess

            done = subprocess.run([str(stub), "--version"], capture_output=True, text=True,
                                  env=dict(os.environ, CAS_AGENT_ROLE="worker"), timeout=10)
            self.assertEqual(done.stdout, "claude 9.9 (stub)\n")

    @unittest.skipUnless(sys.platform == "linux", "Linux-only /dev/pts writer receipt")
    def test_each_worker_pane_claims_the_next_rate_and_writes_at_it(self):
        tmp, stub = stub_dir([40.0, 200.0])
        with tmp:
            slow = run_in_pty(stub, "worker", 1.0)
            fast = run_in_pty(stub, "worker", 1.0)
            spare = run_in_pty(stub, "worker", 0.3)
            slow_n = [int(m[1]) for m in load.WRITE_SEQ.findall(slow)]
            fast_n = [int(m[1]) for m in load.WRITE_SEQ.findall(fast)]
            self.assertIn(b"(40/s)", slow)
            self.assertIn(b"(200/s)", fast)
            # Generous bounds: the writer keeps its schedule on a loaded host.
            self.assertGreater(max(slow_n), 20, slow[-200:])
            self.assertLess(max(slow_n), 60)
            self.assertGreater(max(fast_n), 100, fast[-200:])
            self.assertEqual(load.WRITE_SEQ.findall(spare), [], "no rate left: the pane sleeps")
            claims = load.read_writer_claims(Path(tmp.name) / "writer-slots")
            self.assertEqual([(c["label"], c["rate"]) for c in claims],
                             [("worker-pane", 40.0), ("worker-pane", 200.0), ("worker-pane", None)])
            self.assertTrue(all(c["tty"].startswith("/dev/pts/") for c in claims), claims)

    def test_supervisor_pane_sleeps_and_echoes(self):
        tmp, stub = stub_dir([40.0])
        with tmp:
            pid, fd = pty.fork()
            if pid == 0:
                os.environ["CAS_AGENT_ROLE"] = "supervisor"
                os.execv(str(stub), [str(stub)])
            try:
                time.sleep(0.3)
                os.write(fd, b"Q")
                ready, _, _ = select.select([fd], [], [], 5)
                self.assertTrue(ready)
                self.assertIn(b"Q", os.read(fd, 1024), "the terminal echoes typed keys")
            finally:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
                os.close(fd)
            self.assertFalse(any((Path(tmp.name) / "writer-slots").iterdir()),
                             "the supervisor never claims a writer rate")


class Framing(unittest.TestCase):
    def test_frames_round_trip_across_split_reads(self):
        frames = load.encode_frame({"Output": {"pane_id": "w1", "data": [65, 66]}}) + \
            load.encode_frame("Pong")
        decoder = load.FrameDecoder()
        got = []
        for i in range(len(frames)):
            got += decoder.feed(frames[i:i + 1])
        self.assertEqual(got, [{"Output": {"pane_id": "w1", "data": [65, 66]}}, "Pong"])
        self.assertEqual(load.pane_output(got[0]), ("w1", b"AB"))
        self.assertIsNone(load.pane_output(got[1]))

    def test_supervisor_pane_from_welcome(self):
        welcome = {"Welcome": {"state": {"panes": [
            {"id": "w1", "kind": "Worker"}, {"id": "load-supervisor", "kind": "Supervisor"}]}}}
        self.assertEqual(load.supervisor_pane(welcome), "load-supervisor")
        self.assertIsNone(load.supervisor_pane({"Welcome": {}}))


class FakeDaemon(threading.Thread):
    """A GUI socket that sends Welcome, echoes Input after `delay` and streams a writer."""

    def __init__(self, path, delay, drop_every=0):
        super().__init__(daemon=True)
        self.path, self.delay, self.drop_every = path, delay, drop_every
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(str(path))
        self.server.listen(1)
        self.inputs = 0

    def run(self):
        conn, _ = self.server.accept()
        self.server.close()
        lock = threading.Lock()

        def send(message):
            with lock:
                conn.sendall(load.encode_frame(message))

        send({"Welcome": {"state": {"panes": [{"id": "sup", "kind": "Supervisor"},
                                              {"id": "w1", "kind": "Worker"}]}}})

        def writer():
            n = 0
            while True:
                n += 1
                try:
                    send({"Output": {"pane_id": "w1",
                                     "data": list(f"\r w1 write {n} (100/s)".encode())}})
                except OSError:
                    return
                time.sleep(0.01)

        threading.Thread(target=writer, daemon=True).start()
        decoder = load.FrameDecoder()
        while True:
            data = conn.recv(4096)
            if not data:
                conn.close()
                return
            for message in decoder.feed(data):
                key = message["Input"]["data"]
                if key == [load.KILL_LINE]:
                    continue
                self.inputs += 1
                if self.drop_every and self.inputs % self.drop_every == 0:
                    continue
                time.sleep(self.delay)
                send({"Output": {"pane_id": "sup", "data": key}})


class EchoProbeTest(unittest.TestCase):
    def run_probe(self, delay, drop_every=0, seconds=1.2):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        path = Path(tmp.name) / "gui.sock"
        daemon = FakeDaemon(path, delay, drop_every)
        daemon.start()
        stop = threading.Event()
        probe = load.EchoProbe(path, stop, interval=0.02, timeout=0.3)
        probe.measuring.set()
        probe.start()
        time.sleep(seconds)
        stop.set()
        probe.join(timeout=5)
        return probe

    def test_probe_times_echo_on_the_supervisor_pane_and_counts_writer_rate(self):
        probe = self.run_probe(delay=0.005)
        echo = load.summarize_echo(probe)
        self.assertIsNone(echo["error"])
        self.assertEqual(echo["pane"], "sup")
        self.assertGreater(echo["probes"], 10)
        self.assertEqual(echo["timeouts"], 0)
        self.assertGreaterEqual(echo["p50_ms"], 5.0)
        self.assertLess(echo["p99_ms"], 250.0)
        writers = load.summarize_writers(probe, [100.0])
        self.assertEqual([row["pane"] for row in writers["observed"]], ["w1"])
        self.assertEqual(writers["observed"][0]["panes"], ["w1"])
        self.assertGreater(writers["observed"][0]["writes_per_sec"], 20)
        self.assertTrue(writers["complete"])
        missing = load.summarize_writers(probe, [100.0, 236.0])
        self.assertEqual((missing["complete"], missing["missing_writes_per_sec"]), (False, [236.0]))
        twice = load.summarize_writers(probe, [100.0, 100.0])
        self.assertEqual(twice["missing_writes_per_sec"], [100.0], "rates compare as a multiset")

    def test_a_lost_echo_counts_as_the_timeout(self):
        probe = self.run_probe(delay=0.0, drop_every=3)
        echo = load.summarize_echo(probe)
        self.assertGreater(echo["timeouts"], 0)
        self.assertEqual(echo["max_ms"], 300.0)


FAKE_SERVE = r"""#!/usr/bin/env python3
import json, os, sys
marker = os.environ["FAKE_SERVE_MARKER"]
if not os.path.exists(marker):
    open(marker, "w").close()
    sys.stderr.write("[ERROR] eager store init failed at 'task_store'\n")
    sys.exit(1)
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    result = {"content": [{"type": "text", "text": "Created task: cas-abcd"}]}
    if message["method"] == "initialize":
        result = {}
    if message["method"] == "tools/call" and os.environ.get("FAKE_SERVE_EXIT_AFTER_CREATE"):
        with open(os.environ["FAKE_SERVE_CREATES"], "a") as creates:
            creates.write("cas-abcd\n")
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}) + "\n")
    sys.stdout.flush()
    if message["method"] == "tools/call" and os.environ.get("FAKE_SERVE_EXIT_AFTER_CREATE"):
        sys.exit(1)
"""


class AgentBoot(unittest.TestCase):
    def test_a_serve_exit_after_create_does_not_create_another_task(self):
        class ExitAfterCreate(load.Agent):
            def timed_call(self, *args):
                result = super().timed_call(*args)
                # Wait for the real child exit after its acknowledged response.
                self.client.proc.wait(timeout=5)
                return result

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake = root / "cas"
            fake.write_text(FAKE_SERVE)
            fake.chmod(0o755)
            (root / "run").mkdir()
            marker = root / "booted-once"
            marker.touch()  # Start cleanly; fail only after a successful create.
            creates = root / "creates"
            args = argparse.Namespace(cas_bin=str(fake), boot_retries=2, seed=1,
                                      run_dir=str(root / "run"), t0=0.0)
            agent = ExitAfterCreate(1, args, {"project": root}, [], [], [], threading.Lock(), 0)
            env = dict(os.environ, FAKE_SERVE_MARKER=str(marker), FAKE_SERVE_EXIT_AFTER_CREATE="1",
                       FAKE_SERVE_CREATES=str(creates))
            with self.assertRaisesRegex(RuntimeError, r"cas serve exited \(1\)"):
                agent.boot(env, root / "run")
            self.assertEqual(creates.read_text().splitlines(), ["cas-abcd"],
                             "acknowledged create must fence the boot retry")
            self.assertEqual(agent.own_task, "cas-abcd", "retain the acknowledged task for the receipt")
            self.assertEqual(len(agent.boot_failures), 1)
            self.assertIsNone(agent.client)

    def test_a_serve_that_exits_at_boot_is_restarted_and_recorded(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake = root / "cas"
            fake.write_text(FAKE_SERVE)
            fake.chmod(0o755)
            (root / "run").mkdir()
            args = argparse.Namespace(cas_bin=str(fake), boot_retries=2, seed=1, run_dir=str(root / "run"),
                                      exclude=[], think_mean=1.0, t0=0.0)
            agent = load.Agent(1, args, {"project": root}, ["cas-0000"], [], [], threading.Lock(), 0)
            env = dict(os.environ, FAKE_SERVE_MARKER=str(root / "booted-once"))
            agent.boot(env, root / "run")
            self.assertEqual(agent.own_task, "cas-abcd")
            self.assertEqual(len(agent.boot_failures), 1, agent.boot_failures)
            agent.client.close()

            agent = load.Agent(2, argparse.Namespace(**{**vars(args), "boot_retries": 0}),
                               {"project": root}, ["cas-0000"], [], [], threading.Lock(), 0)
            env["FAKE_SERVE_MARKER"] = str(root / "fresh-marker")
            with self.assertRaises(RuntimeError):
                agent.boot(env, root / "run")
            self.assertEqual(len(agent.boot_failures), 1)


class Receipts(unittest.TestCase):
    def receipt(self, pass_ok, echo_p99, agents=16, writers=(38.0, 63.0, 236.0)):
        return {
            "agents": agents,
            "agent_errors": {},
            "cas_bin": "/x/cas",
            "cas_bin_sha256": "ab" * 32,
            "background_writers": {"writers": 2},
            "host_load": {"load1_min": 20.0, "load1_mean": 25.0, "load1_max": 30.0, "cpus": 32},
            "synthetic_writers": {"configured_writes_per_sec": list(writers),
                                  "complete": bool(writers)},
            "daemon": {"enabled": True, "pass_p99_ms": 12, "pass_p99_under_100ms": pass_ok,
                       "reported": {"passes_over_100ms": 0, "timed_passes": 500}},
            "key_to_echo": {"enabled": True, "error": None, "probes": 100, "timeouts": 0,
                            "p99_ms": echo_p99},
        }

    def test_rows_carry_names_budgets_and_host_load(self):
        rows = load.flv2_receipts(self.receipt(True, 20.0))
        self.assertEqual([r["receipt"] for r in rows],
                         [load.RECEIPT_CONTROL_PASS, load.RECEIPT_KEY_TO_ECHO])
        self.assertTrue(all(r["met"] for r in rows))
        self.assertEqual(rows[1]["host_load1"]["load1_max"], 30.0)
        self.assertEqual(rows[1]["synthetic_writers"], [38.0, 63.0, 236.0])
        self.assertEqual((rows[0]["live_agents"], rows[0]["cas_bin_sha256"]), (16, "ab" * 32))

    def test_misses_and_under_specified_runs_are_not_met(self):
        self.assertFalse(load.flv2_receipts(self.receipt(True, 50.0))[1]["met"])
        self.assertFalse(load.flv2_receipts(self.receipt(False, 10.0))[0]["met"])
        self.assertFalse(load.flv2_receipts(self.receipt(True, 10.0, agents=4))[0]["met"])
        self.assertFalse(load.flv2_receipts(self.receipt(True, 10.0, writers=()))[1]["met"])
        lost = self.receipt(True, 10.0)
        lost["agent_errors"] = {"load-agent-11": "RuntimeError: cas serve exited (1)"}
        self.assertEqual([r["met"] for r in load.flv2_receipts(lost)], [False, False],
                         "15 live agents are not a 16-agent receipt")
        incomplete = self.receipt(True, 10.0)
        incomplete["synthetic_writers"]["complete"] = False
        self.assertFalse(load.flv2_receipts(incomplete)[1]["met"], "a writer never started")

    def test_echo_slo_joins_the_verdict(self):
        echo = {"enabled": True, "error": None, "probes": 10, "timeouts": 1,
                "timeout_ms": 5000.0, "p99_ms": 70.0}
        slos = load.evaluate_slos([], {"enabled": False}, [], echo)
        self.assertEqual(slos[-1]["slo"], "key -> echo p99 < 50 ms")
        self.assertFalse(slos[-1]["met"])

    def test_binary_digest(self):
        with tempfile.NamedTemporaryFile() as handle:
            handle.write(b"cas")
            handle.flush()
            self.assertEqual(load.file_sha256(handle.name), hashlib.sha256(b"cas").hexdigest())
        self.assertIsNone(load.file_sha256("/nonexistent/cas"))

    @unittest.skipUnless(sys.platform == "linux", "Linux-only /proc/loadavg receipt")
    def test_loadavg_summary(self):
        samples = [{"load1": v, "load5": 1.0, "load15": 1.0, "at": "t"} for v in (3.0, 9.0, 6.0)]
        summary = load.summarize_load(samples)
        self.assertEqual((summary["load1_min"], summary["load1_max"]), (3.0, 9.0))
        self.assertEqual(summary["middle"]["load1"], 9.0)
        self.assertIsNotNone(load.read_loadavg())


if __name__ == "__main__":
    unittest.main()
