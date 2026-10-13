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

spec = importlib.util.spec_from_file_location("live_check", SCRIPTS / "live-check-peer-collab.py")
live_check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(live_check)


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
    def make_run(self, *extra):
        receipts = tempfile.mkdtemp(prefix="live-check-test-")
        self.addCleanup(shutil.rmtree, receipts, True)
        args = live_check.parse_args([
            "--binary", "/bin/true", "--prowl-binary", "/bin/true", "--receipts-dir", receipts,
            "--run-id", "abc123", *extra,
        ])
        run = live_check.Run(args)
        calls = []
        for step in live_check.STEP_ORDER:
            setattr(run, f"step_{step}", lambda evidence, notes, step=step: calls.append(step))
        return run, calls

    def test_pending_steps_are_not_executed_by_default(self):
        run, calls = self.make_run()
        run.run_steps()
        statuses = {r["step"]: r["status"] for r in run.results}
        for step, task in live_check.PENDING_BY_DEFAULT.items():
            self.assertEqual(statuses[step], "pending", step)
            self.assertNotIn(step, calls)
        self.assertEqual(statuses["peers_discovery"], "pass")
        receipt = json.loads((run.receipts / "02-peer_detail.json").read_text())
        self.assertEqual((receipt["status"], receipt["requires"]), ("pending", "cas-559c"))

    def test_try_pending_executes_but_still_reports_pending(self):
        run, calls = self.make_run("--try-pending")
        run.run_steps()
        statuses = {r["step"]: r["status"] for r in run.results}
        self.assertEqual(set(calls), set(live_check.STEP_ORDER))
        for step in live_check.PENDING_BY_DEFAULT:
            self.assertEqual(statuses[step], "pending", step)

    def test_merged_steps_run_for_real(self):
        run, calls = self.make_run("--merged", "cas-559c")
        run.steps = ["peer_detail"]

        def failing(evidence, notes):
            raise live_check.StepFailed("no detail")

        run.step_peer_detail = failing
        run.run_steps()
        self.assertEqual(run.results[0]["status"], "fail")


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
