#!/usr/bin/env python3
"""Owned-PR resume and exact-pushed-head CI fixtures for the release train."""

import json
import os
import runpy
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent
TRAIN = ROOT / "release-train.sh"
COMPATIBILITY = json.loads((ROOT.parent / "crates/cas-types/src/violet-compatibility.json").read_text())
LEGACY_ENV_PREFIX = COMPATIBILITY["legacy_token_prefix"].split("_", 1)[0] + "_"
FAKE_GH = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
state = Path(os.environ["TRAIN_TEST_STATE"])
args = sys.argv[1:]
with (state / "calls.log").open("a") as log:
    log.write(" ".join(args) + "\n")
head = os.environ["TRAIN_TEST_HEAD"]
branch = "release/9.99.8"
def rows(name):
    return json.loads((state / name).read_text())
def output(value):
    print(json.dumps(value))
if args[:2] == ["pr", "list"]:
    prs = rows("prs.json")
    if "--head" in args:
        prs = [row for row in prs if row["headRefName"] == args[args.index("--head") + 1]]
    output(prs)
elif args[:2] == ["pr", "create"]:
    (state / "prs.json").write_text(json.dumps([
        {"number": 1049, "title": "Release 9.99.8", "headRefName": branch}]))
    print("https://example.invalid/pull/1049")
elif args[:2] == ["pr", "comment"]:
    sys.stdin.read()
elif args[:2] == ["pr", "checks"]:
    output([{"name": name, "bucket": os.environ.get("TRAIN_TEST_CHECKS", "pass")}
            for name in ("Fast Validation", "macOS Check")])
elif args[:2] == ["pr", "view"]:
    output({"headRefOid": head, "id": "PR_1049", "mergeable": "MERGEABLE",
            "statusCheckRollup": [{"name": "Fast Validation"}, {"name": "macOS Check"}],
            "state": "MERGED" if (state / "ready").exists() else "OPEN",
            "mergeCommit": {"oid": head}})
elif args[:2] == ["run", "list"] and "pull_request" in args:
    counter = state / "run-polls.txt"
    count = int(counter.read_text()) + 1 if counter.exists() else 1
    counter.write_text(str(count))
    mode = os.environ.get("TRAIN_TEST_VISIBILITY", "current")
    if mode == "api-error":
        sys.exit(1)
    if mode == "malformed":
        output({})
    elif mode == "missing" or (mode == "delayed" and count <= 2):
        output([])
    else:
        output([{"headSha": "a" * 40 if mode == "stale" else head}])
elif args[:2] == ["run", "list"]:
    output([] if (state / "ready").exists() else [
        {"databaseId": 77, "status": "completed", "conclusion": "failure", "createdAt": "9999-01-01T00:00:00Z"}])
elif args[:2] == ["api", "graphql"]:
    query = next(arg for arg in args if arg.startswith("query="))
    if "enqueuePullRequest" in query:
        (state / "enqueued").touch()
        output({"data": {"enqueuePullRequest": {"mergeQueueEntry": {"state": "QUEUED"}}}})
    elif "mergeQueue(branch:" in query:
        output({"data": {"repository": {"mergeQueue": {"entries": {"nodes": [
            {"pullRequest": row} for row in rows("queue.json")]}}}}})
    else:
        output({"data": {"repository": {"pullRequest": {"mergeQueueEntry": {"state": "QUEUED"}}}}})
else:
    sys.exit("unexpected gh call: " + " ".join(args))
'''


class Pipeline(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.worktree = self.root / "release"
        self.worktree.mkdir()
        self.state = self.root / "state"
        self.state.mkdir()
        self.run = self.root / "artifacts/v9.99.8-release"
        self.write_rows("prs.json", [])
        self.write_rows("queue.json", [])
        gh = self.root / "gh"
        gh.write_text(FAKE_GH)
        gh.chmod(0o755)
        gate = self.root / "gate"
        gate.write_text("#!/bin/sh\nprintf 'PASS fixture\\n'\n")
        gate.chmod(0o755)
        self.env = {name: value for name, value in os.environ.items()
                    if not name.startswith(("CAS_", "VIOLET_", LEGACY_ENV_PREFIX))}
        self.env.update(
            CAS_RELEASE_ARTIFACTS_ROOT=str(self.root / "artifacts"),
            CAS_RELEASE_TRAIN_CAS="/usr/bin/false",
            CAS_RELEASE_TRAIN_GH=str(gh),
            CAS_RELEASE_TRAIN_DATE="2099-01-02",
            CAS_RELEASE_TRAIN_PREFLIGHT_SKIP_TOOLCHAIN="1",
            CAS_RELEASE_ENV_FILE=str(self.worktree / "release.env"),
            CAS_RELEASE_GATE_HOME_DIR=str(self.root / "scratch"),
            CAS_RELEASE_TRAIN_ASSEMBLE_CMD="true",
            CAS_RELEASE_TRAIN_PREP_CMD="true",
            CAS_RELEASE_TRAIN_LEDGER_CMD="true",
            CAS_RELEASE_TRAIN_PR_BODY_CMD="true",
            CAS_RELEASE_TRAIN_ANNOUNCE_CMD="true",
            CAS_RELEASE_TRAIN_GATE_CMD=str(gate),
            CAS_RELEASE_TRAIN_POLL_SECS="0",
            CAS_RELEASE_TRAIN_CHECK_TRIES="4",
            CAS_RELEASE_TRAIN_MERGEABILITY_TRIES="2",
            CAS_RELEASE_TRAIN_MERGEABILITY_POLL_SECS="0",
            CAS_RELEASE_TRAIN_WATCH_TRIES="2",
            CAS_RELEASE_TRAIN_CUT_STOP_AFTER="pipeline",
            TRAIN_TEST_STATE=str(self.state),
        )
        self.git("init", "-q", "-b", "release/9.99.8")
        self.git("config", "user.name", "Pipeline Test")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "core.hooksPath", "/dev/null")
        (self.worktree / "release.env").write_text("")
        (self.worktree / ".gitignore").write_text(".cas/locks/\n")
        (self.worktree / "CHANGELOG.md").write_text("# Changelog\n\n## [9.99.8] - 2099-01-02\n\n- fixture\n")
        draft = self.worktree / "docs/release-notes/2099-01-02-v9.99.8-slack.md"
        draft.parent.mkdir(parents=True)
        bodies = ("*Live on production — User — Cassy v9.99.8*\nWas: slow. → Now: faster.",
                  "• *Checks* — Was: slow. → Now: faster.",
                  "*Live on production — Dev — Cassy v9.99.8*\nWas: slow. → Now: faster.",
                  "• *Checks* — Was: slow. → Now: faster.")
        draft.write_text("\n\n".join(f"```text\n{body}\n```" for body in bodies))
        zig = self.worktree / ".context/zig/zig"
        zig.parent.mkdir(parents=True)
        zig.write_text("#!/bin/sh\nexit 0\n")
        zig.chmod(0o755)
        self.env["ZIG"] = str(zig)
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "seed")
        base = self.git("rev-parse", "HEAD").strip()
        remote = self.root / "remote.git"
        subprocess.run(["git", "init", "-q", "--bare", str(remote)], check=True)
        self.git("remote", "add", "origin", str(remote))
        self.git("push", "-q", "origin", "HEAD:refs/heads/main")
        integration = self.worktree / ".cas/merge-sweeps/integration.json"
        integration.parent.mkdir(parents=True)
        integration.write_text(json.dumps({"status": "PASSED", "base": base, "tip": base, "epics": [],
            "no_build": {"tip": base, "rows": {row: "PASS" for row in
                runpy.run_path(str(Path(__file__).with_name("release-integration-gates.py")))["REQUIRED_ROWS"]}}}))
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "receipt")
        self.env["TRAIN_TEST_HEAD"] = self.git("rev-parse", "HEAD").strip()

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.worktree), *args],
                                       stderr=subprocess.PIPE, text=True)

    def write_rows(self, name, rows):
        (self.state / name).write_text(json.dumps(rows))

    def invoke(self, *args):
        return subprocess.run(["bash", str(TRAIN), "9.99.8", str(self.worktree), *args],
                              env=self.env, capture_output=True, text=True, timeout=30)

    def preflight_competitors(self):
        self.run.mkdir(parents=True, exist_ok=True)
        return subprocess.run(["bash", "-c", '''
script_dir="$1"
worktree="$2"
run_dir="$3"
version=9.99.8
source "$script_dir/release-train.d/preflight.sh"
cut_stage_file() { printf '%s/stage.%s.done\\n' "$run_dir" "$1"; }
cut_preflight_check_competing_release
''', "bash", str(ROOT), str(self.worktree), str(self.run)],
                              env=self.env, capture_output=True, text=True, timeout=10)

    def seed_pipeline(self):
        self.run.mkdir(parents=True, exist_ok=True)
        (self.run / "gate.done").write_text("0\n")
        (self.run / "gate.full.sha").write_text(self.env["TRAIN_TEST_HEAD"] + "\n")
        (self.run / "gate.log").write_text("PASS fixture\n")
        (self.run / "pr-body.md").write_text("fixture\n")

    def test_number_and_branch_exclusions_apply_to_both_competitor_lists(self):
        self.run.mkdir(parents=True)
        (self.run / "pr-number.txt").write_text("1049\n")
        own = [{"number": 1049, "title": "Release 9.99.8", "headRefName": "release/old-name"},
               {"number": 222, "title": "Release 9.99.8", "headRefName": "release/9.99.8"}]
        self.write_rows("prs.json", own)
        self.write_rows("queue.json", own)
        result = self.preflight_competitors()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("api graphql", (self.state / "calls.log").read_text())

    def test_other_session_still_blocks_in_either_list(self):
        self.run.mkdir(parents=True)
        (self.run / "pr-number.txt").write_text("1049\n")
        own = {"number": 1049, "title": "Release 9.99.8", "headRefName": "release/9.99.8"}
        other = {"number": 1050, "title": "Release 9.99.8", "headRefName": "release/other-session"}
        for source, blocker in (("prs.json", "an open release PR"), ("queue.json", "in the merge queue")):
            with self.subTest(source=source):
                self.write_rows("prs.json", [own])
                self.write_rows("queue.json", [own])
                self.write_rows(source, [own, other])
                result = self.preflight_competitors()
                self.assertEqual(result.returncode, 1)
                self.assertIn("BLOCKER competing-release", result.stderr)
                self.assertIn(blocker, result.stderr)

    def test_ordinary_branch_does_not_exempt_matching_pr(self):
        self.git("switch", "-c", "feature/change")
        self.write_rows("prs.json", [{"number": 1049, "title": "Release 9.99.8",
                                      "headRefName": "feature/change"}])
        result = self.preflight_competitors()
        self.assertEqual(result.returncode, 1)
        self.assertIn("BLOCKER competing-release", result.stderr)

    def test_cut_resume_after_queue_failure_excludes_owned_pr(self):
        first = self.invoke("--cut")
        self.assertIn("QUEUE_RUN_FAILED", first.stdout + first.stderr)
        self.assertEqual((self.run / "pr-number.txt").read_text().strip(), "1049")
        # Reassembly rewrites the release tip, invalidating old stage receipts.
        (self.worktree / "fixed.txt").write_text("fix\n")
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "--amend", "-qm", "fixed")
        self.env["TRAIN_TEST_HEAD"] = self.git("rev-parse", "HEAD").strip()
        self.write_rows("queue.json", json.loads((self.state / "prs.json").read_text()))
        (self.state / "ready").touch()
        resumed = self.invoke("--cut", "--resume")
        output = resumed.stdout + resumed.stderr
        self.assertIn("stage preflight: start", output)
        self.assertNotIn("BLOCKER competing-release", output)
        self.assertIn("stopped after stage pipeline", output)
        self.assertEqual((self.run / "pipeline.done").read_text().strip(), "MERGED")

    def test_stale_green_rollup_waits_for_pushed_head_run(self):
        self.env["TRAIN_TEST_VISIBILITY"] = "delayed"
        (self.state / "ready").touch()
        self.seed_pipeline()
        result = self.invoke("--pipeline")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = (self.state / "calls.log").read_text().splitlines()
        first_checks = next(i for i, line in enumerate(calls) if line.startswith("pr checks"))
        polls_before_checks = calls[:first_checks]
        self.assertEqual(sum("pull_request" in line for line in polls_before_checks), 3)
        self.assertIn("--commit " + self.env["TRAIN_TEST_HEAD"], "\n".join(polls_before_checks))
        self.assertIn("--branch release/9.99.8 --event pull_request", "\n".join(polls_before_checks))

    def test_unproven_head_never_reads_green_rollup_or_enqueues(self):
        self.seed_pipeline()
        (self.state / "ready").touch()
        for mode in ("missing", "stale", "api-error", "malformed"):
            with self.subTest(mode=mode):
                self.env["TRAIN_TEST_VISIBILITY"] = mode
                (self.state / "calls.log").write_text("")
                result = self.invoke("--pipeline")
                self.assertEqual(result.returncode, 1)
                self.assertEqual((self.run / "pipeline.done").read_text().strip(), "CHECKS_FAILED")
                calls = (self.state / "calls.log").read_text()
                self.assertNotIn("pr checks", calls)
                self.assertNotIn("enqueuePullRequest", calls)

    def test_visible_run_does_not_bypass_failed_checks(self):
        self.seed_pipeline()
        self.env["TRAIN_TEST_CHECKS"] = "fail"
        result = self.invoke("--pipeline")
        self.assertEqual(result.returncode, 1)
        self.assertEqual((self.run / "pipeline.done").read_text().strip(), "CHECKS_FAILED")
        calls = (self.state / "calls.log").read_text()
        self.assertIn("pr checks", calls)
        self.assertNotIn("enqueuePullRequest", calls)


if __name__ == "__main__":
    unittest.main()
