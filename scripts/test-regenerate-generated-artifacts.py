#!/usr/bin/env python3
"""Two lanes that both change builtins merge without a reference-history conflict.

python3 scripts/test-regenerate-generated-artifacts.py

cas-fb11: cas-cli/src/builtins/reference-history.json is derived from git
history, so two lanes that each regenerate it always conflicted. The repository
.gitattributes marks it `merge=cas-generated` (keep the target's copy), and
scripts/regenerate-generated-artifacts.sh reruns the real generator on the
merged history. The fixture repository carries this checkout's scripts and
.gitattributes, so the test exercises exactly what ships.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
LEDGER = "cas-cli/src/builtins/reference-history.json"
SCRIPTS = ("gen-builtin-reference-history.sh", "regenerate-generated-artifacts.sh", "cas-merge-drivers.py")


class ReferenceHistoryMergeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        self.env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.org",
                    "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.org"}
        self.git("init", "-q", "-b", "main")
        (self.repo / "scripts").mkdir()
        for name in SCRIPTS:
            shutil.copy2(ROOT / "scripts" / name, self.repo / "scripts" / name)
        shutil.copy2(ROOT / ".gitattributes", self.repo / ".gitattributes")
        self.write("cas-cli/src/builtins/agents/task-verifier.body.md", "Verify the task.\n")
        self.write("cas-cli/src/builtins/skills/demo/SKILL.md", "---\nname: demo\n---\n# Demo\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "baseline")
        self.run_script("gen-builtin-reference-history.sh")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "baseline ledger")
        subprocess.run(["python3", "scripts/cas-merge-drivers.py", "install"], cwd=self.repo, check=True,
                       capture_output=True)

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, relative, text):
        path = self.repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def git(self, *args, check=True):
        return subprocess.run(["git", *args], cwd=self.repo, env=self.env, check=check,
                              capture_output=True, text=True)

    def run_script(self, name, *args):
        result = subprocess.run(["bash", f"scripts/{name}", *args], cwd=self.repo, env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, f"{name} failed:\n{result.stdout}{result.stderr}")
        return result

    def lane(self, branch, reference):
        """A lane adds a builtin reference and commits its own regenerated ledger."""
        self.git("checkout", "-q", "-b", branch, "main")
        self.write(f"cas-cli/src/builtins/skills/demo/references/{reference}.md", f"# {reference}\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", f"{branch}: add {reference}")
        self.run_script("gen-builtin-reference-history.sh")
        self.git("commit", "-q", "-am", f"{branch}: regenerate the ledger")
        self.git("checkout", "-q", "main")

    def test_two_lanes_adding_builtin_history_merge_cleanly_and_the_ledger_equals_a_fresh_run(self):
        self.lane("factory/one", "first")
        self.lane("factory/two", "second")
        self.git("merge", "--no-ff", "-q", "factory/one", "-m", "merge one")
        before = self.git("rev-parse", "HEAD").stdout.strip()
        merged = self.git("merge", "--no-ff", "-q", "factory/two", "-m", "merge two", check=False)
        self.assertEqual(merged.returncode, 0, "the ledger must not conflict:\n" + merged.stdout + merged.stderr)
        after = self.git("rev-parse", "HEAD").stdout.strip()
        self.run_script("regenerate-generated-artifacts.sh", before, after)
        self.assertEqual(self.git("status", "--porcelain").stdout, "", "the regenerated ledger is committed")
        committed = (self.repo / LEDGER).read_text()
        self.run_script("gen-builtin-reference-history.sh")
        self.assertEqual((self.repo / LEDGER).read_text(), committed, "the ledger equals a fresh generator run")
        for reference in ("first", "second"):
            self.assertIn(f"skills/demo/references/{reference}.md", committed)


if __name__ == "__main__":
    unittest.main()
