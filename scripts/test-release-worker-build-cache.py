#!/usr/bin/env python3
"""Exercise the real atomic cache publisher with fake Cargo; no Rust builds."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("host_update", SCRIPTS / "release-host-update.py")
host_update = importlib.util.module_from_spec(spec)
spec.loader.exec_module(host_update)


class WorkerCacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        (self.repo / "source").write_text("released\n")
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "release")
        self.released = self.git("rev-parse", "HEAD")
        self.git("tag", "v9.99.20")
        # Main can advance and its checkout can be dirty without poisoning the cache.
        (self.repo / "source").write_text("next main commit\n")
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "next")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        (self.repo / "source").write_text("uncommitted edit\n")
        (self.repo / ".gitignore").write_text(".cas/\n.gitignore\n")
        self.cache = self.repo / ".cas/build-cache"
        self.cache.mkdir(parents=True)
        (self.cache / "snapshots/old").mkdir(parents=True)
        (self.cache / "current").write_text("old\n")
        self.cargo = self.root / "fake-cargo"
        self.cargo.write_text('''#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == 'check --workspace --lib --tests' ]] || exit 21
[[ "$(git rev-parse HEAD)" == "$FIXTURE_RELEASED" ]] || exit 22
[[ "$(cat source)" == released ]] || exit 23
[[ "$CARGO_TARGET_DIR" == "$CAS_ROOT/build-cache/snapshots/target-"* ]] || exit 24
[[ "$(cat "$CAS_ROOT/build-cache/current")" == old ]] || exit 25
mkdir -p "$CARGO_TARGET_DIR"
printf 'complete artifact\n' >"$CARGO_TARGET_DIR/artifact"
[[ ! -e "$CARGO_TARGET_DIR/.cas-build-cache-metadata" ]] || exit 26
exit "${FIXTURE_CARGO_EXIT:-0}"
''')
        self.cargo.chmod(0o755)
        self.receipt = self.root / "host-update.json"
        self.env = patch.dict(os.environ, {
            "CARGO": str(self.cargo), "FIXTURE_RELEASED": self.released,
            "FIXTURE_CARGO_EXIT": "0", "CAS_ROOT": str(self.root / "wrong-root"),
            "CAS_RELEASE_TRAIN_WORKER_CACHE_CMD": str(SCRIPTS / "refresh-worker-build-cache.sh"),
            "CAS_RELEASE_TRAIN_WORKER_CACHE_TIMEOUT_SECS": "60",
        })
        self.env.start()
        self.addCleanup(self.env.stop)

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.repo), *args], capture_output=True,
                              text=True, check=True).stdout.strip()

    def refresh(self):
        return host_update.refresh_worker_cache("9.99.20", self.repo, self.receipt)

    def assert_no_checkouts(self):
        self.assertEqual(len(self.git("worktree", "list", "--porcelain").split("worktree ")) - 1, 1)
        self.assertEqual(list(self.cache.glob("release-cache-*")), [])

    def test_success_publishes_completed_released_snapshot(self):
        result = self.refresh()
        self.assertEqual(result["status"], "PASS", result)
        self.assertEqual(result["source_commit"], self.released)
        self.assertEqual((self.cache / "current").read_text().strip(), result["snapshot"])
        snapshot = self.cache / "snapshots" / result["snapshot"]
        self.assertEqual((snapshot / "artifact").read_text(), "complete artifact\n")
        self.assertIn(f"source_commit={self.released}\n",
                      (snapshot / ".cas-build-cache-metadata").read_text())
        self.assertTrue((self.cache / "snapshots/old").is_dir())
        self.assertEqual((self.repo / "source").read_text(), "uncommitted edit\n")
        self.assertFalse((self.root / "wrong-root").exists())
        self.assert_no_checkouts()

    def test_failed_build_keeps_old_snapshot_and_removes_incomplete_one(self):
        with patch.dict(os.environ, FIXTURE_CARGO_EXIT="12"):
            result = self.refresh()
        self.assertEqual(result["status"], "WARN")
        self.assertIsNone(result["snapshot"])
        self.assertEqual(result["exit"], 12)
        self.assertEqual((self.cache / "current").read_text(), "old\n")
        self.assertEqual([p.name for p in (self.cache / "snapshots").iterdir()], ["old"])
        self.assert_no_checkouts()

    def test_missing_tag_warns_without_building(self):
        self.git("tag", "-d", "v9.99.20")
        self.assertEqual(self.refresh()["status"], "WARN")
        self.assertFalse(self.receipt.with_name("worker-build-cache.log").exists())

    def test_tag_off_main_warns_without_building(self):
        self.git("update-ref", "refs/remotes/origin/main", self.released)
        self.git("tag", "-f", "v9.99.20", "HEAD")
        self.assertEqual(self.refresh()["status"], "WARN")
        self.assertFalse(self.receipt.with_name("worker-build-cache.log").exists())

    def stub(self, body):
        script = self.root / "refresh-stub"
        script.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body)
        script.chmod(0o755)
        return patch.dict(os.environ, CAS_RELEASE_TRAIN_WORKER_CACHE_CMD=str(script))

    def test_zero_exit_without_publication_is_warn(self):
        with self.stub("echo no-publication\n"):
            result = self.refresh()
        self.assertEqual(result["status"], "WARN")
        self.assertIsNone(result["snapshot"])
        self.assert_no_checkouts()

    def test_provenance_mismatch_is_warn(self):
        with self.stub('''mkdir -p "$CAS_ROOT/build-cache/snapshots/bad"
printf 'source_commit=unrelated\n' >"$CAS_ROOT/build-cache/snapshots/bad/.cas-build-cache-metadata"
echo 'Published worker target baseline: bad'
'''):
            result = self.refresh()
        self.assertEqual(result["status"], "WARN")
        self.assertIsNone(result["snapshot"])
        self.assert_no_checkouts()

    def test_receipt_names_this_refresh_even_if_another_pointer_wins(self):
        with self.stub('''mkdir -p "$CAS_ROOT/build-cache/snapshots/ours"
printf 'source_commit=%s\n' "$FIXTURE_RELEASED" >"$CAS_ROOT/build-cache/snapshots/ours/.cas-build-cache-metadata"
echo other >"$CAS_ROOT/build-cache/current"
echo 'Published worker target baseline: ours'
'''):
            result = self.refresh()
        self.assertEqual(result["status"], "PASS", result)
        self.assertEqual(result["snapshot"], "ours")
        self.assertEqual((self.cache / "current").read_text(), "other\n")

    def test_invalid_timeout_warns_without_building(self):
        with patch.dict(os.environ, CAS_RELEASE_TRAIN_WORKER_CACHE_TIMEOUT_SECS="bad"):
            result = self.refresh()
        self.assertEqual(result["status"], "WARN")
        self.assertEqual((self.cache / "current").read_text(), "old\n")
        self.assert_no_checkouts()

    def test_timeout_stops_descendant_writer_and_keeps_old_pointer(self):
        marker = self.root / "late-publication"
        with self.stub(f"(sleep 2; touch '{marker}') &\nwait\n"), patch.dict(
                os.environ, CAS_RELEASE_TRAIN_WORKER_CACHE_TIMEOUT_SECS="1"):
            result = self.refresh()
        self.assertEqual(result["status"], "WARN")
        self.assertIn("timed out after 1s", result["detail"])
        time.sleep(1.2)
        self.assertFalse(marker.exists(), "descendant writer survived refresh timeout")
        self.assertEqual((self.cache / "current").read_text(), "old\n")
        self.assert_no_checkouts()


if __name__ == "__main__":
    unittest.main(verbosity=2)
