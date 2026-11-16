#!/usr/bin/env python3
"""scripts/doc-fragments.py: parallel fragment deliveries and the release compile."""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import yaml

SCRIPT = Path(__file__).resolve().parent / "doc-fragments.py"
REGISTRY = """version: 1
updated: '2026-11-11'
bug_families:
  existing-case:
    title: Existing inline case
    tasks: [cas-0001]
    journeys: [J1]
# Comment that belongs to the next key.
invariants:
  INV-A: {statement: Arrived, check: Read state}
journeys:
- id: J1
  status: live
"""
OVERVIEW = """# Product Overview

## Pitch

Cassy runs factories.

## Core Concepts

- **Task** - unit of work.

## Primary Journeys

- J1.

<!-- cas:project-overview
watch_dirs = ["src"]
-->
"""
GIT_ENV = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.test",
           "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.test"}


def git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=repo, env=GIT_ENV, capture_output=True, text=True)


class DocFragments(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.repo = Path(scratch.name)
        git(self.repo, "init", "-q", "-b", "main")
        self.write("docs/qa/journeys.yaml", REGISTRY)
        self.write("docs/PRODUCT_OVERVIEW.md", OVERVIEW)
        self.commit("base")

    def write(self, path: str, body: str) -> None:
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(body)

    def commit(self, message: str) -> None:
        git(self.repo, "add", "-A")
        self.assertEqual(git(self.repo, "commit", "-qm", message).returncode, 0)

    def deliver(self, task: str, family: str, concept: str) -> None:
        git(self.repo, "checkout", "-qb", task, "main")
        self.write(f"docs/qa/bug-families/{family}.yaml",
                   f"# docs/qa/bug-families/{family}.yaml\ntitle: Case for {task}\ntasks: [{task}]\n"
                   "invariants: [INV-A]\njourneys: [J1]\n")
        self.write(f"docs/product-overview.d/{task}.md", f"## Core Concepts\n\n- **{concept}** - added by {task}.\n")
        self.commit(task)
        git(self.repo, "checkout", "-q", "main")

    def run_script(self, command: str) -> subprocess.CompletedProcess:
        return subprocess.run([sys.executable, str(SCRIPT), command, "--root", str(self.repo)],
                              capture_output=True, text=True)

    def test_parallel_fragment_deliveries_merge_without_conflict(self):
        self.deliver("cas-aaaa", "first-case", "Alpha")
        self.deliver("cas-bbbb", "second-case", "Beta")
        for branch in ("cas-aaaa", "cas-bbbb"):
            merged = git(self.repo, "merge", "--no-ff", "-qm", f"merge {branch}", branch)
            self.assertEqual(merged.returncode, 0, merged.stdout + merged.stderr)
        self.assertEqual(self.run_script("check").returncode, 0)

    def test_release_compile_folds_fragments_into_shared_docs(self):
        self.deliver("cas-aaaa", "first-case", "Alpha")
        self.deliver("cas-bbbb", "second-case", "Beta")
        for branch in ("cas-aaaa", "cas-bbbb"):
            git(self.repo, "merge", "--no-ff", "-qm", f"merge {branch}", branch)
        expected = {**yaml.safe_load(REGISTRY)["bug_families"]}
        for family in ("first-case", "second-case"):
            expected[family] = yaml.safe_load((self.repo / f"docs/qa/bug-families/{family}.yaml").read_text())

        compiled = self.run_script("compile")
        self.assertEqual(compiled.returncode, 0, compiled.stdout + compiled.stderr)

        registry = yaml.safe_load((self.repo / "docs/qa/journeys.yaml").read_text())
        self.assertEqual(registry["bug_families"], expected)
        self.assertEqual({k: v for k, v in registry.items() if k != "bug_families"},
                         {k: v for k, v in yaml.safe_load(REGISTRY).items() if k != "bug_families"})
        overview = (self.repo / "docs/PRODUCT_OVERVIEW.md").read_text()
        concepts = overview.split("## Core Concepts")[1].split("## Primary Journeys")[0]
        self.assertIn("**Alpha**", concepts)
        self.assertIn("**Beta**", concepts)
        self.assertTrue(overview.endswith(OVERVIEW.split("## Primary Journeys")[1]))
        self.assertFalse(list((self.repo / "docs/qa").glob("bug-families/*.yaml")))
        self.assertFalse(list(self.repo.glob("docs/product-overview.d/*.md")))
        # The compiled tree is itself clean input for the next release.
        self.assertEqual(self.run_script("check").returncode, 0)


if __name__ == "__main__":
    unittest.main()
