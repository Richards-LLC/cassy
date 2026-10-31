#!/usr/bin/env python3
"""Measure release-stage budgets and file breaches through standalone helpers."""
from __future__ import annotations

import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest

SCRIPTS = Path(__file__).resolve().parent

FAKE_CAS = textwrap.dedent('''\
    #!/usr/bin/env python3
    import json, os, sys
    assert sys.argv[1:] == ["serve"], sys.argv
    for line in sys.stdin:
        message = json.loads(line)
        if message.get("method") == "initialize":
            print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {}}), flush=True)
            # A server-to-client request must be answered, not mistaken for a reply.
            print(json.dumps({"jsonrpc": "2.0", "id": "srv-1", "method": "roots/list"}), flush=True)
        elif message.get("method") == "tools/call":
            with open(os.environ["FAKE_CAS_LOG"], "a") as log:
                log.write(json.dumps(message["params"]) + "\\n")
            text = "Created task: cas-f00d - " + message["params"]["arguments"]["title"] + " (P0)"
            print(json.dumps({"jsonrpc": "2.0", "id": message["id"],
                              "result": {"content": [{"type": "text", "text": text}]}}), flush=True)
''')

# The 2026-10-14 cut: gate 3029 s and publish 1526 s.
STAGE_EVENTS = """\
1791997675\tpreflight\tstart
1791997712\tpreflight\tdone
1791997729\tgate\tstart
1792000758\tgate\tdone
1792000758\tpipeline\tstart
1792001610\tpipeline\tdone
1792001610\tpublish\tstart
1792003136\tpublish\tdone
"""


class BudgetBreach(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.run_dir = self.tmp / "run"
        self.run_dir.mkdir()
        self.cas = self.tmp / "cas"
        self.cas.write_text(FAKE_CAS)
        self.cas.chmod(0o755)
        self.log = self.tmp / "cas-calls.jsonl"
        self.gh = self.tmp / "gh"
        self.gh.write_text(textwrap.dedent('''\
            #!/usr/bin/env bash
            case "$1 $2" in
              "run list") printf '[{"databaseId":7,"status":"completed","conclusion":"success","event":"push","headBranch":"main","headSha":"%s","createdAt":"2026-10-14T15:10:00Z"}]\\n' "$(cat "$LANDED")" ;;
              "release view") printf '2026-10-14T15:12:16Z\\n' ;;
              "api repos"*) printf '{"workflow_runs":[{"id":7,"created_at":"2026-10-14T15:10:00Z"}]}\\n' ;;
              *) exit 2 ;;
            esac
            '''))
        self.gh.chmod(0o755)
        # Keep the release's real annotated tag and landed-main identity.
        def git(*args):
            return subprocess.run(["git", "-C", str(self.tmp), *args], check=True,
                                  capture_output=True, text=True).stdout.strip()
        git("init", "-q", "-b", "main")
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-qm", "release")
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "-c", "tag.gpgsign=false", "tag", "-am", "release", "v9.99.8")
        (self.run_dir / "landed-main.sha").write_text(git("rev-parse", "HEAD") + "\n")

    def file_breach(self, events: str) -> subprocess.CompletedProcess:
        (self.run_dir / "stage-events.tsv").write_text(events)
        env = dict(os.environ, FAKE_CAS_LOG=str(self.log),
                   LANDED=str(self.run_dir / "landed-main.sha"), GH_BIN=str(self.gh))
        measured = subprocess.run(
            ["bash", str(SCRIPTS / "release-latency-receipt.sh"), "v9.99.8",
             "--run-dir", str(self.run_dir)], cwd=self.tmp, env=env, capture_output=True, text=True)
        self.assertEqual(measured.returncode, 0, measured.stdout + measured.stderr)
        return subprocess.run(
            [sys.executable, str(SCRIPTS / "release-budget-breach.py"), "v9.99.8",
             str(self.run_dir), "--cas-bin", str(self.cas)],
            cwd=self.tmp, env=env, capture_output=True, text=True)

    def calls(self) -> list[dict]:
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def test_over_budget_release_files_one_p0_in_the_budget_epic(self):
        result = self.file_breach(STAGE_EVENTS)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        receipt = (self.run_dir / "release-latency.receipt").read_text()
        self.assertIn("BUDGET_BREACHES=gate:3029/900,publish:1526/1200\n", receipt)
        [call] = self.calls()
        arguments = call["arguments"]
        epic = json.loads((SCRIPTS / "release-stage-budgets.json").read_text())["epic"]
        self.assertEqual((call["name"], arguments["action"], arguments["priority"], arguments["epic"]),
                         ("task", "create", 0, epic))
        self.assertIn("gate 50m 29s > 15m 00s", arguments["title"])
        self.assertIn("- publish: measured 1526 s, budget 1200 s (326 s over)", arguments["description"])
        self.assertIn(str(self.run_dir / "release-latency.receipt"), arguments["description"])
        # pin: resumed runs consume the newline-delimited task ID marker written from the MCP reply.
        self.assertEqual((self.run_dir / "budget-breach.task").read_text(), "cas-f00d\n")
        # A resumed helper never files the same breach twice.
        self.assertEqual(self.file_breach(STAGE_EVENTS).returncode, 0)
        self.assertEqual(len(self.calls()), 1)

    def test_release_within_budget_files_nothing(self):
        fast = "1791997729\tgate\tstart\n1791998329\tgate\tdone\n"
        result = self.file_breach(fast)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        # pin: the receipt protocol uses BUDGET_BREACHES=none to suppress task creation.
        self.assertIn("BUDGET_BREACHES=none\n", (self.run_dir / "release-latency.receipt").read_text())
        self.assertEqual(self.calls(), [])
        self.assertFalse((self.run_dir / "budget-breach.task").exists())

    def test_a_failed_filing_keeps_the_stage_resumable(self):
        self.cas.write_text("#!/usr/bin/env bash\nexit 3\n")
        result = self.file_breach(STAGE_EVENTS)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cas serve exited", result.stderr)
        self.assertFalse((self.run_dir / "budget-breach.task").exists())



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
