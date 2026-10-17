#!/usr/bin/env python3
"""Offline self-test for the two-machine live check (cas-daa0).

Covers what can be checked without two machines or a cas binary: the
stand-in cloud's contract (tokens, claims, mailbox leases, dedupe, faults),
the runner's rule that a step needing unmerged code is never reported as a
pass, and the scratch-directory guard.
"""

import importlib.util
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS / "live_check_peer_collab"))
import standin_cloud  # noqa: E402
import harness  # noqa: E402

spec = importlib.util.spec_from_file_location("live_check", SCRIPTS / "live-check-peer-collab.py")
live_check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(live_check)


class OvercapProbeTest(unittest.TestCase):
    def test_queue_find_hashes_explicit_probe_body_after_routing_blank_lines(self):
        import contextlib
        import hashlib
        import io
        import sqlite3
        from types import SimpleNamespace
        token = "live-overcap-send-fixture"
        body = token + "\nreadable π\n" + token
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo" / ".cas"
            root.mkdir(parents=True)
            with sqlite3.connect(root / "cas.db") as db:
                db.execute("CREATE TABLE prompt_queue (id INTEGER, source TEXT, target TEXT, prompt TEXT, summary TEXT)")
                db.execute("INSERT INTO prompt_queue VALUES (1, 'peer:alpha', 'beta', ?, ?)",
                           ("routing metadata\n\nmore routing instructions\n\n" + body, token))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                harness.cmd_queue_find(SimpleNamespace(run_dir=tmp, token=token))
            row = json.loads(output.getvalue())["rows"][0]
            self.assertEqual(row["peer_body_sha256"], hashlib.sha256(body.encode()).hexdigest())
            self.assertEqual(row["peer_body_bytes"], len(body.encode()))

    def run_probe(self, corrupt=False):
        import hashlib
        from types import SimpleNamespace
        run = object.__new__(live_check.Run)
        run.args = SimpleNamespace(cloud_url="https://live.example", delivery_timeout=1)
        run.rid = "probe-fixture"
        run.names = {"a": "alpha", "b": "beta"}
        class Machine:
            def tool(self, *args, **kwargs):
                return {"text": "delivered"}
        run.a = Machine()
        run.b = Machine()
        bodies = {}
        def message(machine, agent, target, summary, body, **extra):
            bodies[target] = body
            return {"text": "Sent via Cassy Cloud (peer message pm_1)", "is_error": False}
        run.message = message
        run.delivered_locally = lambda *args: 9
        def rows(machine, token):
            body = next(body for body in bodies.values() if token in body)
            if corrupt:
                body = "Body spilled to /tmp/local-artifact.md"
            return [{"id": 9, "peer_body_sha256": hashlib.sha256(body.encode()).hexdigest(),
                     "peer_body_bytes": len(body.encode())}]
        run.queued_rows = rows
        run.wait_for = lambda label, probe, timeout: probe()[1]
        evidence = {}
        run.step_overcap_message_reply_status(evidence, [])
        return evidence

    def test_overcap_probe_records_both_digests_and_receipts(self):
        evidence = self.run_probe()
        self.assertEqual(evidence["receipt_scope"], "live-cloud")
        for direction in ("send", "reply"):
            self.assertGreater(evidence[direction]["body_bytes"], 30000)
            self.assertEqual(evidence[direction]["notification_id"], 9)
            self.assertIn("received_at", evidence[direction])

    def test_overcap_probe_rejects_destination_local_pointer(self):
        with self.assertRaises(live_check.StepFailed):
            self.run_probe(corrupt=True)


class HarnessWorkerCapabilitiesTest(unittest.TestCase):
    def test_scripted_claude_help_advertises_supported_worker_isolation(self):
        # pin: native Claude worker discovery requires these two external CLI switches.
        result = subprocess.run([sys.executable, str(SCRIPTS / "live_check_peer_collab/harness.py"), "--help"],
                                stdin=subprocess.DEVNULL, capture_output=True, text=True, check=True)
        self.assertIn("--mcp-config", result.stdout)
        self.assertIn("--strict-mcp-config", result.stdout)


class TeamClaimsProbeTest(unittest.TestCase):
    def run_probe(self, visibility="team_project", missing=False, queued=False):
        from types import SimpleNamespace
        run = object.__new__(live_check.Run)
        run.args = SimpleNamespace(cloud_url="https://sandbox.example")
        run.standin = None
        run.ctx = {"task_id": "cas-held"}
        run.names = {"a": "alpha", "b": "beta", "bw": "beta-worker"}
        class Machine:
            def __init__(self):
                self.run_dir = "/sandbox-fixture"
                self.assigned = False
                self.counts = iter([0, int(queued)])
            def call(self, *args, **kwargs):
                return {"spawn_queue_rows": next(self.counts)}
            def tool(self, agent, name, **args):
                if name == "coordination":
                    return {"text": json.dumps({"peers": [{
                        "user_id": "account-a", "claims_visibility": visibility,
                        "supervisor": {"machine": "alpha-host"},
                        "claims": [] if missing else [{"task_id": "cas-held", "agent_name": "alpha"}],
                    }]})}
                if args["action"] == "update":
                    self.assigned = bool(args.get("assignee"))
                    return {"text": "updated", "is_error": False}
                if name == "factory" and self.assigned:
                    return {"text": "already assigned to live worker beta-worker", "is_error": True}
                return {"text": "HELD BY A PEER alpha on alpha-host (account account-a)", "is_error": True}
        run.a = Machine()
        run.b = Machine()
        evidence = {}
        run.step_team_claims(evidence, [])
        return evidence

    def test_team_claim_probe_requires_owner_claim_and_all_refusals(self):
        evidence = self.run_probe()
        self.assertEqual(evidence["receipt_scope"], "live-cloud-two-account")
        self.assertEqual(evidence["owner_user_id"], "account-a")
        self.assertIn("start", evidence)
        self.assertIn("claim", evidence)
        self.assertIn("spawn", evidence)

    def test_team_claim_probe_rejects_missing_claim_or_queued_spawn(self):
        for options in ({"missing": True}, {"queued": True}):
            with self.subTest(options=options), self.assertRaises(live_check.StepFailed):
                self.run_probe(**options)

    def test_same_account_run_never_counts_as_two_account_proof(self):
        with self.assertRaises(live_check.StepSkipped):
            self.run_probe(visibility="same_account")


class StandinCloudTest(unittest.TestCase):
    def setUp(self):
        self.state = standin_cloud.State()
        handler = standin_cloud.make_handler(self.state, {"tok-a", "tok-b"}, "admin", None)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()

    def call(self, method, path, body=None, token="tok-a"):
        request = urllib.request.Request(
            self.url + path, method=method,
            data=json.dumps(body).encode() if body is not None else None,
            headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                return response.status, json.loads(response.read())
        except urllib.error.HTTPError as error:
            with error:
                return error.code, json.loads(error.read())

    def register(self, agent_id, name, repo="github.com/acme/widgets", token="tok-a"):
        return self.call("POST", "/api/agents/register", {
            "id": agent_id, "name": name, "agent_type": "primary", "machine_id": name,
            "metadata": {"role": "supervisor", "canonical_id": repo, "factory_session": f"s-{name}"},
        }, token=token)

    def test_unknown_token_is_refused(self):
        self.assertEqual(self.call("GET", "/api/agents", token="nope")[0], 401)
        self.assertEqual(self.call("GET", "/api/agents", token="")[0], 401)
        self.assertEqual(self.call("GET", "/__standin/state", token="tok-a")[0], 401)

    def test_claim_conflict_names_the_owner_and_release_frees_it(self):
        self.register("a1", "alpha")
        self.register("b1", "beta", token="tok-b")
        status, reply = self.call("POST", "/api/agents/tasks/cas-1~abcd/claim",
                                  {"agent_id": "a1", "duration_secs": 600, "reason": "r"})
        self.assertEqual((status, reply["result"]), (200, "claimed"))
        status, reply = self.call("POST", "/api/agents/tasks/cas-1~abcd/claim",
                                  {"agent_id": "b1"}, token="tok-b")
        self.assertEqual((status, reply["owner_agent_id"]), (409, "a1"))
        self.call("POST", "/api/agents/tasks/cas-1~abcd/release", {"agent_id": "a1"})
        self.assertEqual(self.call("GET", "/api/agents/tasks/cas-1~abcd/lock")[1], {"lock": None})

    def test_mailbox_dedupes_scopes_by_repo_and_leases(self):
        self.register("a1", "alpha")
        self.register("b1", "beta", token="tok-b")
        self.register("c1", "gamma", repo="github.com/acme/other")
        message = {"project_id": "github.com/acme/widgets", "sender_agent_id": "a1",
                   "recipient_agent_id": "b1", "body": "hi", "dedupe_key": "k1"}
        self.assertEqual(self.call("POST", "/api/peer-messages", message)[1],
                         {"id": "pm_1", "status": "queued"})
        self.assertEqual(self.call("POST", "/api/peer-messages", message)[1]["status"], "duplicate")
        other = dict(message, recipient_agent_id="c1", dedupe_key="k2")
        self.assertEqual(self.call("POST", "/api/peer-messages", other)[0], 403)
        unknown = dict(message, recipient_agent_id="zz", dedupe_key="k3")
        self.assertEqual(self.call("POST", "/api/peer-messages", unknown)[0], 404)

        claim = {"project_ids": ["github.com/acme/widgets"], "recipient_agent_ids": ["b1"],
                 "consumer_id": "beta:s", "max": 50, "lease_secs": 1}
        claimed = self.call("POST", "/api/peer-messages/claim", claim, token="tok-b")[1]["messages"]
        self.assertEqual([m["id"] for m in claimed], ["pm_1"])
        self.assertEqual(claimed[0]["sender_name"], "alpha")
        self.assertEqual(self.call("POST", "/api/peer-messages/claim", claim, token="tok-b")[1]["messages"], [])
        time.sleep(1.2)  # the lease runs out: the message is claimable again
        again = self.call("POST", "/api/peer-messages/claim", claim, token="tok-b")[1]["messages"]
        self.assertEqual([(m["id"], m["attempts"]) for m in again], [("pm_1", 2)])
        self.call("POST", "/api/peer-messages/ack",
                  {"consumer_id": "beta:s", "acks": [{"id": "pm_1", "outcome": "delivered"}]}, token="tok-b")
        status = self.call("GET", "/api/peer-messages/pm_1")[1]
        self.assertEqual(status["status"], "delivered")
        self.assertIsNotNone(status["delivered_at"])

    def test_injected_registration_failures_are_counted_down(self):
        self.assertEqual(self.call("POST", "/__standin/faults", {"register_fail": 1}, token="admin")[0], 200)
        self.assertEqual(self.register("a1", "alpha")[0], 503)
        self.assertEqual(self.register("a1", "alpha")[0], 200)


class PendingIsNeverAPassTest(unittest.TestCase):
    """cas-937a5 F03: a step's pending state comes from the binary probe."""

    def make_run(self, *extra, features=None):
        receipts = tempfile.mkdtemp(prefix="live-check-test-")
        self.addCleanup(shutil.rmtree, receipts, True)
        args = live_check.parse_args([
            "--binary", "/bin/true", "--prowl-binary", "/bin/true", "--receipts-dir", receipts,
            "--run-id", "abc123", *extra,
        ])
        run = live_check.Run(args)
        run.features = dict.fromkeys(set(live_check.STEP_REQUIRES.values()), False)
        run.features.update(features or {})
        run.probes = {"soundwave": {"features": dict(run.features)}, "prowl": {"features": dict(run.features)}}
        calls = []
        for step in live_check.STEP_ORDER:
            setattr(run, f"step_{step}", lambda evidence, notes, step=step: calls.append(step))
        return run, calls

    def test_steps_whose_feature_is_missing_are_pending_and_not_executed(self):
        run, calls = self.make_run()
        run.run_steps()
        statuses = {r["step"]: r["status"] for r in run.results}
        for step in live_check.STEP_REQUIRES:
            self.assertEqual(statuses[step], "pending", step)
            self.assertNotIn(step, calls)
        self.assertEqual(statuses["peers_discovery"], "pass")
        receipt = json.loads((run.receipts / "02-peer_detail.json").read_text())
        self.assertEqual((receipt["status"], receipt["requires"]), ("pending", "cas-559c"))
        self.assertEqual(receipt["probe"], {"soundwave": False, "prowl": False})

    def test_try_pending_executes_but_still_reports_pending(self):
        run, calls = self.make_run("--try-pending")
        run.run_steps()
        statuses = {r["step"]: r["status"] for r in run.results}
        self.assertEqual(set(calls), set(live_check.STEP_ORDER))
        for step in live_check.STEP_REQUIRES:
            self.assertEqual(statuses[step], "pending", step)

    def test_a_probed_feature_makes_its_step_run_for_real(self):
        run, calls = self.make_run(features={"cas-559c": True})
        run.steps = ["peer_detail", "epic_split"]

        def failing(evidence, notes):
            raise live_check.StepFailed("no detail")

        run.step_peer_detail = failing
        run.run_steps()
        statuses = {r["step"]: r["status"] for r in run.results}
        self.assertEqual(statuses, {"peer_detail": "fail", "epic_split": "pending"})


class FakeMachine:
    def __init__(self, label):
        self.label = label
        self.binary = f"/fake/{label}/cas"


class PreflightTest(unittest.TestCase):
    """cas-937a5 F02/F03: configuration problems exit 2 before anything is
    created; binaries at different commits are refused unless --allow-mixed."""

    def run_preflight(self, commits, *extra, features=None):
        receipts = tempfile.mkdtemp(prefix="live-check-test-")
        self.addCleanup(shutil.rmtree, receipts, True)
        args = live_check.parse_args(["--binary", sys.executable, "--prowl-binary", "/x",
                                      "--receipts-dir", receipts, "--run-id", "pf0001", *extra])
        run = live_check.Run(args)
        machines = (FakeMachine("soundwave"), FakeMachine("prowl"))

        def resolve():
            run.machines = machines

        run.resolve_machines = resolve
        original = live_check.machine_stream

        def fake_stream(machine, *argv, timeout=180):
            commit = commits[machine.label]
            return {"ok": True, "commit": commit, "version": f"cas 0.0.0-fixture ({commit} 2026-10-13)",
                    "features": (features or {}).get(machine.label, {})}

        live_check.machine_stream = fake_stream
        self.addCleanup(setattr, live_check, "machine_stream", original)
        run.preflight()
        return run

    def test_mixed_binaries_are_refused_unless_allowed(self):
        with self.assertRaises(live_check.ConfigError) as refused:
            self.run_preflight({"soundwave": "1ce845a", "prowl": "c25f0e3"})
        self.assertIn("different commits", str(refused.exception))
        run = self.run_preflight({"soundwave": "1ce845a", "prowl": "c25f0e3"}, "--allow-mixed",
                                 features={"soundwave": {"cas-a19b": True, "cas-13c7": True},
                                           "prowl": {"cas-a19b": True}})
        self.assertTrue(run.features["cas-a19b"])
        self.assertFalse(run.features["cas-13c7"], "a feature counts only when both binaries carry it")
        self.assertTrue((run.receipts / "preflight.json").exists())

    def test_a_missing_token_exits_2_before_creating_anything(self):
        receipts = tempfile.mkdtemp(prefix="live-check-test-")
        self.addCleanup(shutil.rmtree, receipts, True)
        env = {k: v for k, v in os.environ.items() if k not in ("LIVE_CHECK_TOKEN_A", "LIVE_CHECK_TOKEN_B")}
        result = subprocess.run(
            [sys.executable, str(SCRIPTS / "live-check-peer-collab.py"), "--binary", sys.executable,
             "--prowl-binary", "/x", "--cloud-url", "https://example.invalid", "--receipts-dir", receipts,
             "--run-id", "tok001", "--soundwave-scratch", receipts],
            capture_output=True, text=True, env=env, timeout=60)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("needs a device token", result.stdout)
        self.assertFalse(list(Path(receipts).glob("live-check-*")), "nothing was created")
        summary = json.loads(next(Path(receipts).glob("*/summary.json")).read_text())
        self.assertIn("device token", summary["config_error"])

    def test_unknown_step_exits_2(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPTS / "live-check-peer-collab.py"), "--binary", sys.executable,
             "--prowl-binary", "/x", "--steps", "nope", "--receipts-dir", tempfile.gettempdir()],
            capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 2, result.stderr)


class ProbeTest(unittest.TestCase):
    def test_probe_reads_the_commit_and_feature_markers_from_the_binary(self):
        sys.path.insert(0, str(SCRIPTS / "live_check_peer_collab"))
        import harness

        with tempfile.TemporaryDirectory() as tmp:
            fake = Path(tmp) / "cas"
            markers = b"\n".join(b"# " + marker for marker in harness.FEATURE_MARKERS["cas-4736"])
            fake.write_bytes(b"#!/bin/sh\necho 'cas 0.0.0-fixture (abc1234 2026-10-13)'\nexit 0\n" + markers + b"\n")
            fake.chmod(0o755)
            probe = harness.probe_binary(str(fake))
        self.assertEqual(probe["commit"], "abc1234")
        self.assertTrue(probe["features"]["cas-4736"])
        self.assertFalse(probe["features"]["cas-559c"])


SIGTERM_CHILD = r"""
import importlib.util, json, shutil, subprocess, sys, time
from pathlib import Path
spec = importlib.util.spec_from_file_location("lc", sys.argv[1]); lc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lc)
scratch = Path(sys.argv[2])

def preflight(self):
    self.machines = tuple(
        lc.Machine(label, None, str(scratch / f"live-check-{self.rid}" / label), "/bin/true", sys.executable)
        for label in ("soundwave", "prowl"))

def setup(self):
    for machine in self.machines:
        Path(machine.run_dir).mkdir(parents=True)
        shutil.copy2(lc.HELPER_DIR / "harness.py", machine.helper)
        # Stand-ins for the factory daemons: detached, cwd inside the run dir.
        subprocess.Popen(["sleep", "300"], cwd=machine.run_dir, start_new_session=True)
        machine.versions_before = {"version": "v", "sha256": "s"}

def run_steps(self):
    print("READY", flush=True)
    time.sleep(120)

lc.Run.preflight, lc.Run.setup, lc.Run.run_steps = preflight, setup, run_steps
lc.machine_call_installed = lambda machine: {"version": "v", "sha256": "s"}
sys.exit(lc.main(sys.argv[3:]))
"""


class SigtermTeardownTest(unittest.TestCase):
    """cas-937a5 F01: SIGTERM (and SIGHUP) unwind into teardown: every process
    in the run directories is stopped and the directories are removed."""

    def check_signal(self, signum):
        with tempfile.TemporaryDirectory() as tmp:
            receipts = Path(tmp) / "receipts"
            child = subprocess.Popen(
                [sys.executable, "-c", SIGTERM_CHILD, str(SCRIPTS / "live-check-peer-collab.py"), tmp,
                 "--binary", "/bin/true", "--prowl-binary", "/bin/true", "--receipts-dir", str(receipts),
                 "--run-id", "sig001"],
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            output = ""
            for line in child.stdout:
                output += line
                if "READY" in line:
                    break
            run_root = Path(tmp) / "live-check-sig001"
            sys.path.insert(0, str(SCRIPTS / "live_check_peer_collab"))
            import harness

            self.assertEqual(len(harness.run_dir_processes(str(run_root))), 2, output)
            child.send_signal(signum)
            output += child.communicate(timeout=60)[0]
            self.assertEqual(child.returncode, 128 + signum, output)
            self.assertFalse(run_root.exists(), output)
            self.assertEqual(harness.run_dir_processes(str(run_root)), [])
            teardown = json.loads(next(receipts.glob("*/teardown.json")).read_text())
            self.assertEqual(teardown["soundwave"]["processes_stopped"], 1)
            self.assertEqual(teardown["prowl"]["processes_left"], [])
            summary = json.loads(next(receipts.glob("*/summary.json")).read_text())
            self.assertIn(signal.Signals(signum).name, summary["setup_error"])

    def test_sigterm_tears_down(self):
        self.check_signal(signal.SIGTERM)

    def test_sighup_tears_down(self):
        self.check_signal(signal.SIGHUP)


class PeerMessageIdTest(unittest.TestCase):
    def test_send_receipts_from_production_and_the_stand_in_parse(self):
        production = ("Message sent to peer supervisor sup-b@prowl via Cassy Cloud (peer message "
                      "7fbc15f5-15b7-46f1-8f94-997903f2fa0b, queued): delivered to sup-b@prowl mailbox.")
        self.assertEqual(live_check.peer_message_id(production), "7fbc15f5-15b7-46f1-8f94-997903f2fa0b")
        self.assertEqual(live_check.peer_message_id("via Cassy Cloud (peer message pm_1, queued)"), "pm_1")
        self.assertIsNone(live_check.peer_message_id("no supervisor or worker named x"))


class PeerBlocksTest(unittest.TestCase):
    def test_a_peer_and_its_session_line_form_one_block(self):
        view = ("Peers (github.com/x/y, via Cassy\n  Cloud):\n"
                "  sup-b [stale] on prowl, heartbeat 6m ago\n    session lc-1-b | focus -\n"
                "  sup-b [live] on prowl, heartbeat 3s ago\n    session lc-1-br | focus -\n"
                "Epic splits:\n  split-aa [accepted, outgoing] epic cas-1\n")
        blocks = live_check.peer_blocks(view)
        live = [blk for blk in blocks if blk.split()[0] == "sup-b" and "session lc-1-br " in blk + " "]
        self.assertEqual(len(live), 1)
        self.assertIn("[live]", live[0])
        self.assertFalse([blk for blk in blocks if blk.split()[0] == "sup-b" and "session lc-1-b " in blk + " "
                          and "[live]" in blk])


class ScratchGuardTest(unittest.TestCase):
    def test_remove_refuses_anything_but_a_live_check_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            keep = Path(tmp) / "precious"
            keep.mkdir()
            result = subprocess.run(
                [sys.executable, str(SCRIPTS / "live_check_peer_collab" / "harness.py"), "remove",
                 "--run-dir", str(keep)], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue(keep.exists())
            scratch = Path(tmp) / "live-check-abc123"
            (scratch / "soundwave").mkdir(parents=True)
            result = subprocess.run(
                [sys.executable, str(SCRIPTS / "live_check_peer_collab" / "harness.py"), "remove",
                 "--run-dir", str(scratch)], capture_output=True, text=True)
            self.assertEqual(json.loads(result.stdout)["ok"], True)
            self.assertFalse(scratch.exists())

    def test_scratch_env_drops_inherited_cassy_identity(self):
        sys.path.insert(0, str(SCRIPTS / "live_check_peer_collab"))
        import harness

        os.environ["CAS_AGENT_ROLE"] = "worker"
        try:
            env = harness.scratch_env("/scratch/live-check-x/a")
        finally:
            del os.environ["CAS_AGENT_ROLE"]
        self.assertEqual(env["HOME"], "/scratch/live-check-x/a/home")
        self.assertFalse(any(key.startswith("CAS_") for key in env))
        self.assertTrue(env["PATH"].startswith("/scratch/live-check-x/a/bin:"))


if __name__ == "__main__":
    unittest.main()
