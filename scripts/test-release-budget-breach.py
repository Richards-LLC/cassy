#!/usr/bin/env python3
"""Measure park-to-main release latency from real Git and task events."""
from __future__ import annotations

import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest

SCRIPTS = Path(__file__).resolve().parent


class CommitToMain(unittest.TestCase):
    """Park (task event) to the main commit that landed the task's epic merge."""

    def git(self, *args: str, at: int | None = None) -> str:
        env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t",
                   GIT_COMMITTER_EMAIL="t@t")
        if at is not None:
            env.update(GIT_AUTHOR_DATE=f"@{at} +0000", GIT_COMMITTER_DATE=f"@{at} +0000")
        return subprocess.run(["git", *args], cwd=self.repo, env=env, check=True,
                              capture_output=True, text=True).stdout.strip()

    def park(self, task: str, at: int) -> None:
        stamp = time.strftime("%Y-%m-%dT%H:%M:%S.000000000+00:00", time.gmtime(at))
        self.db.execute("INSERT INTO events (event_type, entity_type, entity_id, summary, created_at) "
                        "VALUES ('task_blocked', 'task', ?, 'Task awaiting merge: x', ?)", (task, stamp))
        self.db.commit()

    def test_worst_task_is_budgeted_from_its_last_park_before_the_epic_merge(self):
        tmp = Path(tempfile.mkdtemp())
        self.repo = tmp / "repo"
        self.repo.mkdir()
        self.db = sqlite3.connect(tmp / "cas.db")
        self.db.execute("CREATE TABLE events (event_type TEXT, entity_type TEXT, entity_id TEXT, "
                        "summary TEXT, created_at TEXT)")
        base = 1_791_000_000
        self.git("init", "-q", "-b", "main")
        self.git("commit", "-q", "--allow-empty", "-m", "release 1.0.0", at=base)
        self.git("tag", "v1.0.0")
        self.git("checkout", "-q", "-b", "epic")
        for name, task, subject, merged in (
                ("factory/fast-fox", "cas-aaaa", "Merge factory/fast-fox (cas-aaaa): quick fix", base + 600),
                ("factory/slow-owl", "cas-bbbb", "Merge branch 'factory/slow-owl' into HEAD", base + 3000)):
            self.git("checkout", "-q", "-b", name, "v1.0.0")
            self.git("commit", "-q", "--allow-empty", "-m", f"feat: work ({task})", at=merged - 60)
            self.git("checkout", "-q", "epic")
            self.git("merge", "-q", "--no-ff", "-m", subject, name, at=merged)
        self.git("checkout", "-q", "main")
        self.git("merge", "-q", "--no-ff", "-m", "Merge pull request #1 from epic", "epic", at=base + 4000)
        run_dir = tmp / "run"
        run_dir.mkdir()
        (run_dir / "landed-main.sha").write_text(self.git("rev-parse", "HEAD") + "\n")
        self.park("cas-aaaa", base + 300)
        # cas-bbbb was parked, reopened for changes, and parked again; the
        # delivery that merged is the second park. A park after its merge is
        # some later delivery and does not count.
        self.park("cas-bbbb", base + 100)
        self.park("cas-bbbb", base + 2000)
        self.park("cas-bbbb", base + 3500)
        result = subprocess.run([sys.executable, str(SCRIPTS / "release-metrics.py"), str(run_dir),
                                 str(base + 5000)], cwd=self.repo, capture_output=True, text=True,
                                env=dict(os.environ, CAS_RELEASE_TASK_DB=str(tmp / "cas.db")))
        values = dict(line.split("=", 1) for line in result.stdout.splitlines())
        self.assertEqual(values["COMMIT_TO_MAIN"], "tasks:2,median:2850,worst:cas-aaaa:3700", result.stderr)
        self.assertEqual(values["STAGE_SECS"].split(",")[0], "commit-to-main:3700")
        self.assertIn("commit-to-main:3700/1800", values["BUDGET_BREACHES"])


if __name__ == "__main__":
    unittest.main()
