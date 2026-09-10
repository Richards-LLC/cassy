#!/usr/bin/env python3
"""Fixture proof for release-train.sh --assemble (no Cargo or remote service)."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

TRAIN = Path(__file__).resolve().with_name("release-train.sh")


class IntegrationAssembly(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "project"
        self.root.mkdir()
        self.git("init", "-b", "main")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "Release Fixture")
        self.git("config", "commit.gpgsign", "false")
        (self.root / ".gitignore").write_text(".cas/\n")
        self.git("add", ".")
        self.git("commit", "-m", "base")
        self.base = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/remotes/origin/main", self.base)
        self.git("checkout", "-b", "epic/one")
        (self.root / "feature").write_text("one\n")
        self.git("add", ".")
        self.git("commit", "-m", "feature")
        self.tip = self.git("rev-parse", "HEAD")
        self.git("branch", "integration/project")
        self.git("checkout", "-b", "release/test", "main")
        self.receipt_path = self.root / ".cas/merge-sweeps/integration.json"
        self.receipt_path.parent.mkdir(parents=True)
        self.receipt = {"status": "PASSED", "base": self.base, "tip": self.tip,
                        "epics": [{"id": "one", "branch": "epic/one", "tip": self.tip}]}
        self.save()

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.root), *args],
                                       stderr=subprocess.DEVNULL, text=True).strip()

    def save(self):
        self.receipt_path.write_text(json.dumps(self.receipt))

    def assemble(self):
        return subprocess.run([str(TRAIN), "0.0.0", str(self.root), "--assemble"],
                              capture_output=True, text=True)

    def refused(self, text):
        result = self.assemble()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(text, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.base)

    def test_clean_tip_is_consumed_via_train_action(self):
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)
        self.assertEqual(result.stdout.splitlines()[0], "PASS release assembly")
        self.assertIn(self.tip, result.stdout)

    def test_red_or_pending_sweep_refuses_old_tip(self):
        for status in ["CONFLICT", "FAILED", "RUNNING", "DEFERRED"]:
            self.receipt["status"] = status
            self.save()
            self.refused("no passing sweep")

    def test_changed_epic_refuses_stale_union(self):
        self.git("update-ref", "refs/heads/epic/one", self.base)
        self.refused("epic changed")

    def test_newer_remote_epic_is_used_over_stale_local_ref(self):
        self.git("update-ref", "refs/remotes/origin/epic/one", self.tip)
        self.git("update-ref", "refs/heads/epic/one", self.base)
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)

    def test_changed_main_refuses_stale_union(self):
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        self.refused("Main changed")

    def test_changed_integration_refuses_wrong_receipt(self):
        self.git("update-ref", "refs/heads/integration/project", self.base)
        self.refused("tip changed")

    def test_dirty_destination_is_preserved(self):
        (self.root / "uncommitted").write_text("keep me")
        self.refused("checkout has changes")
        self.assertEqual((self.root / "uncommitted").read_text(), "keep me")

    def test_live_daemon_lock_is_nonblocking(self):
        common = (self.root / ".git").resolve()
        key = hashlib.sha256(b"cas-0a21/delivery-target-lock/v1\0" + os.fsencode(common)
                             + b"\0integration/project").hexdigest()
        path = self.root / ".cas/locks/delivery-target" / (key + ".lock")
        path.parent.mkdir(parents=True)
        with path.open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            self.refused("sweep is running")

    def test_protected_destination_is_preserved(self):
        self.git("checkout", "main")
        self.refused("detached checkout or a release/")


if __name__ == "__main__":
    unittest.main()
