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

    def install_recovery_stub(self):
        stub = self.root / ".cas/fake-cas"
        stub.write_text("""#!/bin/sh
set -eu
test \"$1\" = factory
test \"$2\" = integration-recover
test \"$3\" = --base-only
printf '%s\\n' \"${CAS_FACTORY_SESSION:-missing}\" > .cas/recovery-session
printf '%s\\n' \"${CAS_AGENT_ID:-missing}|${CAS_SESSION_ID:-missing}|${CAS_AGENT_NAME:-missing}|${CAS_AGENT_ROLE:-missing}\" > .cas/recovery-identity
base=$(git rev-parse refs/remotes/origin/main)
git update-ref refs/heads/integration/project \"$base\"
python3 - \"$base\" <<'PY'
import json
from pathlib import Path
import sys
path = Path('.cas/merge-sweeps/integration.json')
receipt = json.loads(path.read_text())
receipt['base'] = sys.argv[1]
receipt['tip'] = sys.argv[1]
receipt['status'] = 'PASSED'
receipt['epics'] = []
receipt['detail'] = 'base-only recovery passed'
path.write_text(json.dumps(receipt))
Path('.cas/healed').write_text('yes\\n')
PY
""")
        stub.chmod(0o755)
        return stub

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

    def test_changed_main_heals_stale_union_once(self):
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        self.install_recovery_stub()
        run_dir = self.root / ".cas/release-run"
        run_dir.mkdir(parents=True)
        (run_dir / "run.env").write_text(
            "factory_session=fixture-session\n"
            "agent_id=fixture-agent-id\n"
            "session_id=fixture-supervisor-session\n"
            "agent_name=fixture-supervisor\n"
            "agent_role=supervisor\n"
        )
        env_names = [
            "CAS_RELEASE_TRAIN_CAS", "CAS_RELEASE_TRAIN_RUN_DIR", "CAS_FACTORY_SESSION",
            "CAS_AGENT_ID", "CAS_SESSION_ID", "CAS_AGENT_NAME", "CAS_AGENT_ROLE",
        ]
        old_env = {name: os.environ.get(name) for name in env_names}
        os.environ["CAS_RELEASE_TRAIN_CAS"] = str(self.root / ".cas/fake-cas")
        os.environ["CAS_RELEASE_TRAIN_RUN_DIR"] = str(run_dir)
        for name in env_names[2:]:
            os.environ.pop(name, None)
        try:
            result = self.assemble()
        finally:
            for name, value in old_env.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)
        self.assertTrue((self.root / ".cas/healed").exists())
        self.assertEqual((self.root / ".cas/recovery-session").read_text().strip(), "fixture-session")
        self.assertEqual(
            (self.root / ".cas/recovery-identity").read_text().strip(),
            "fixture-agent-id|fixture-supervisor-session|fixture-supervisor|supervisor",
        )

    def test_docs_only_receipts_base_names_the_receipts_commit(self):
        self.git("checkout", "main")
        docs = self.root / "docs"
        docs.mkdir()
        (docs / "receipt.md").write_text("posted\n")
        self.git("add", "docs/receipt.md")
        self.git("commit", "-m", "docs receipt")
        docs_tip = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/remotes/origin/main", docs_tip)
        self.git("checkout", "release/test")
        artifacts = Path(self.temp.name) / "artifacts"
        run_dir = artifacts / "v0.0.0-release-test"
        run_dir.mkdir(parents=True)
        (run_dir / "receipts.commit").write_text(
            f"COMMIT_SHA={docs_tip}\nBRANCH=release/test\nBASE_SHA={self.base}\n"
        )
        result = subprocess.run(
            [str(TRAIN), "0.0.0", str(self.root), "--assemble"],
            capture_output=True,
            text=True,
            env={**os.environ, "CAS_RELEASE_ARTIFACTS_ROOT": str(artifacts)},
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("only under docs/", result.stderr)
        self.assertIn(f"receipts commit {docs_tip}", result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.base)

    def test_docs_only_release_commit_rebases_onto_integration_tip(self):
        self.git("checkout", "release/test")
        (self.root / "CHANGELOG.md").write_text("# Changelog\n\n## [0.0.0]\n")
        release_notes = self.root / "docs/release-notes"
        release_notes.mkdir(parents=True)
        (release_notes / "2099-01-01-v0.0.0-slack.md").write_text("draft\n")
        self.git("add", "CHANGELOG.md", "docs/release-notes")
        self.git("commit", "-m", "release docs")
        docs_tip = self.git("rev-parse", "HEAD")

        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        assembled_tip = self.git("rev-parse", "HEAD")
        self.assertNotEqual(assembled_tip, docs_tip)
        self.assertEqual(self.git("rev-parse", "HEAD^"), self.tip)
        self.assertEqual(self.git("show", "--format=%s", "--no-patch", "HEAD"), "release docs")

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
