#!/usr/bin/env python3
"""Exercise the real detached cache refresh without invoking Cargo."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("host_update", Path(__file__).with_name("release-host-update.py"))
host = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(host)


class WorkerCacheTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "test@test.invalid")
        self.git("config", "user.name", "Fixture")
        (self.repo / "seed").write_text("seed")
        self.git("add", ".")
        self.git("commit", "-qm", "seed")
        self.git("tag", "v9.99.1")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.receipt = self.root / "host-update.json"
        self.observed = self.root / "observed.json"
        self.cache = self.root / "cache-stub"
        self.cache.write_text('''#!/usr/bin/env python3
import json, os
from pathlib import Path
import subprocess
zig = Path(os.environ["ZIG"])
assert zig.is_absolute() and zig.is_file() and os.access(zig, os.X_OK)
assert not Path(".context/zig/zig").exists()
Path(os.environ["CACHE_OBSERVED"]).write_text(json.dumps({"ZIG": str(zig), "cwd": os.getcwd()}))
head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
metadata = Path(os.environ["CAS_ROOT"]) / "build-cache/snapshots/fixture/.cas-build-cache-metadata"
metadata.parent.mkdir(parents=True, exist_ok=True)
metadata.write_text("source_commit=" + head + "\\n")
print("Published worker target baseline: fixture")
''')
        self.cache.chmod(0o755)
        self.env = {"ZIG": "", "CAS_RELEASE_TRAIN_WORKER_CACHE_CMD": str(self.cache),
                    "CACHE_OBSERVED": str(self.observed)}

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], text=True).strip()

    def zig(self, path):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("#!/bin/sh\nexit 0\n")
        path.chmod(0o755)
        return path.resolve()

    def refresh(self, worktree=None):
        with patch.dict(os.environ, self.env):
            return host.refresh_worker_cache("9.99.1", worktree or self.repo, self.receipt)

    def test_release_worker_build_cache(self):
        zig = self.zig(self.repo / ".context/zig/zig")
        evidence = self.refresh()
        self.assertEqual(evidence["status"], "PASS", evidence)
        self.assertEqual(json.loads(self.observed.read_text())["ZIG"], str(zig))
        self.assertEqual(self.git("worktree", "list", "--porcelain").count("worktree "), 1)

    def test_explicit_zig_wins(self):
        expected = self.zig(self.root / "configured/zig")
        self.zig(self.repo / ".context/zig/zig")
        self.env["ZIG"] = str(expected)
        self.assertEqual(self.refresh()["status"], "PASS")
        self.assertEqual(json.loads(self.observed.read_text())["ZIG"], str(expected))

    def test_relative_zig_is_bound_to_release_worktree(self):
        expected = self.zig(self.repo / "tools/zig")
        self.env["ZIG"] = "tools/zig"
        self.assertEqual(self.refresh()["status"], "PASS")
        self.assertEqual(json.loads(self.observed.read_text())["ZIG"], str(expected))

    def test_invalid_configured_zig_falls_back(self):
        self.env["ZIG"] = str(self.root / "missing")
        self.zig(self.repo / ".context/zig/zig")
        self.assertEqual(self.refresh()["status"], "PASS")

    def test_main_checkout_fallback_from_linked_worktree(self):
        expected = self.zig(self.repo / ".context/zig/zig")
        linked = self.root / "release"
        self.git("worktree", "add", "--detach", str(linked), "HEAD")
        self.assertEqual(self.refresh(linked)["status"], "PASS")
        self.assertEqual(json.loads(self.observed.read_text())["ZIG"], str(expected))

    def test_missing_zig_warns_before_starting_refresh(self):
        evidence = self.refresh()
        self.assertEqual(evidence["status"], "WARN")
        self.assertIn("bootstrap-zig.sh", evidence["detail"])
        self.assertFalse(self.observed.exists())
        self.assertEqual(self.git("worktree", "list", "--porcelain").count("worktree "), 1)


if __name__ == "__main__":
    unittest.main()
