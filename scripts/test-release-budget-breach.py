#!/usr/bin/env python3
"""A release over its stage budget files one P0 task (cas-2bef).

Drives the real post-publication stage with a recorded stage-events.tsv: the
real latency receipt (release-metrics.py) names the breach, and the breach
filer creates the task through a fake `cas serve` that speaks MCP stdio.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
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

# 2026-10-14's 3.52.0 cut: gate 3029 s and publish 1526 s.
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
              "run list") printf '[{"databaseId":7,"status":"completed","conclusion":"success","headBranch":"v9.99.8","headSha":"%s","createdAt":"2026-10-14T15:10:00Z"}]\\n' "$(cat "$LANDED")" ;;
              "release view") printf '2026-10-14T15:12:16Z\\n' ;;
              "api repos"*) printf '{"workflow_runs":[{"id":7,"created_at":"2026-10-14T15:10:00Z"}]}\\n' ;;
              *) exit 2 ;;
            esac
            '''))
        self.gh.chmod(0o755)
        self.published = self.tmp / "published.sh"
        self.published.write_text("#!/usr/bin/env bash\nprintf 'TAG=%s\\n' \"$1\"\n")
        self.published.chmod(0o755)
        (self.run_dir / "landed-main.sha").write_text("a" * 40 + "\n")

    def post_publication(self, events: str) -> subprocess.CompletedProcess:
        (self.run_dir / "stage-events.tsv").write_text(events)
        script = f'''
            source "{SCRIPTS}/release-train.d/post-publication.sh"
            version=9.99.8 worktree="{self.tmp}" run_dir="{self.run_dir}" script_dir="{SCRIPTS}"
            release_train_post_publication
        '''
        env = dict(os.environ, FAKE_CAS_LOG=str(self.log), LANDED=str(self.run_dir / "landed-main.sha"),
                   CAS_RELEASE_TRAIN_GH=str(self.gh), CAS_RELEASE_TRAIN_CAS=str(self.cas),
                   CAS_RELEASE_TRAIN_POST_PUBLICATION_POLL_SECS="0",
                   CAS_RELEASE_TRAIN_POST_PUBLICATION_TRIES="1",
                   CAS_RELEASE_TRAIN_PUBLISHED_RECEIPT_CMD=str(self.published),
                   CAS_RELEASE_TRAIN_LATENCY_RECEIPT_CMD=str(SCRIPTS / "release-latency-receipt.sh"))
        return subprocess.run(["bash", "-c", script], cwd=self.tmp, env=env, capture_output=True, text=True)

    def calls(self) -> list[dict]:
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def test_over_budget_release_files_one_p0_in_the_budget_epic(self):
        result = self.post_publication(STAGE_EVENTS)
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
        self.assertEqual((self.run_dir / "budget-breach.task").read_text(), "cas-f00d\n")
        # A resumed post-publication stage never files the same breach twice.
        self.assertEqual(self.post_publication(STAGE_EVENTS).returncode, 0)
        self.assertEqual(len(self.calls()), 1)

    def test_release_within_budget_files_nothing(self):
        fast = "1791997729\tgate\tstart\n1791998329\tgate\tdone\n"
        result = self.post_publication(fast)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("BUDGET_BREACHES=none\n", (self.run_dir / "release-latency.receipt").read_text())
        self.assertEqual(self.calls(), [])
        self.assertFalse((self.run_dir / "budget-breach.task").exists())

    def test_a_failed_filing_keeps_the_stage_resumable(self):
        self.cas.write_text("#!/usr/bin/env bash\nexit 3\n")
        result = self.post_publication(STAGE_EVENTS)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("could not file the stage-budget breach", result.stderr)
        self.assertFalse((self.run_dir / "budget-breach.task").exists())


if __name__ == "__main__":
    unittest.main()
