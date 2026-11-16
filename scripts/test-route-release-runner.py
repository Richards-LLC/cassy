#!/usr/bin/env python3
"""Each release prebuild route picks its self-hosted runner only when it is live.

Runs scripts/route-release-runner.sh with the env each route job in
release-prebuild.yml passes, against a fake gh whose probe run reports the
probe job that workflow actually defines for that probe input.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import yaml

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = yaml.safe_load((ROOT / ".github/workflows/release-prebuild.yml").read_text())
GH = """#!/usr/bin/env python3
import json, os, sys
log = os.environ['GH_LOG']
open(log, 'a').write(' '.join(sys.argv[1:]) + '\\n')
args = sys.argv[1:]
if any('/dispatches' in a for a in args):
    print(os.environ['FAKE_RUN_ID'])
elif args[-1].startswith('.jobs[]'):
    jobs = json.loads(os.environ['FAKE_JOBS'])
    name = args[-1].split('"')[1]
    for job in jobs:
        if job['name'] == name: print(f"{job['status']}:{job['conclusion']}")
"""


def route_jobs():
    return {k: j for k, j in WORKFLOW["jobs"].items()
            if any("route-release-runner.sh" in s.get("run", "") for s in j.get("steps", []))}


def probe_job_name(probe):
    # The probe job is the one gated on exactly this platform input.
    [name] = [j["name"] for j in WORKFLOW["jobs"].values()
              if j.get("if", "").strip() == f"inputs.platform == '{probe}'"]
    return name


class RouteReleaseRunnerTest(unittest.TestCase):
    def route(self, job, event, ref, probe_state):
        step = next(s for s in job["steps"] if s.get("id") == "route")
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            (tmp / "gh").write_text(GH)
            (tmp / "gh").chmod(0o755)
            env = {k: str(v) for k, v in step["env"].items() if "${{" not in str(v)}
            jobs = [] if probe_state is None else [
                {"name": probe_job_name(env["PROBE"]), "status": probe_state[0], "conclusion": probe_state[1]}]
            env.update(PATH=f"{tmp}:{os.environ['PATH']}", GH_LOG=str(tmp / "gh.log"),
                       GITHUB_OUTPUT=str(tmp / "out"), FAKE_RUN_ID="77", FAKE_JOBS=json.dumps(jobs),
                       PROBE_WAIT_SECONDS="1", PROBE_POLL_SECONDS="0.2", REPO="Richards-LLC/cassy",
                       EVENT_NAME=event, REF=ref, REF_NAME=ref.removeprefix("refs/heads/"), PLATFORM="")
            subprocess.run(["bash", "-c", step["run"]], cwd=ROOT, env=env, check=True, capture_output=True)
            out = dict(line.split("=", 1) for line in (tmp / "out").read_text().splitlines())
            calls = (tmp / "gh.log").read_text() if (tmp / "gh.log").exists() else ""
            return env, out, calls

    def test_trusted_route_uses_live_self_hosted_runner_else_hosted(self):
        jobs = route_jobs()
        self.assertEqual({"linux-route", "macos-route"}, set(jobs))
        queue = ("merge_group", "refs/heads/gh-readonly-queue/main/pr-1-abc")
        for key, job in jobs.items():
            with self.subTest(key, probe="live"):
                env, out, calls = self.route(job, *queue, ("in_progress", "null"))
                self.assertEqual(out, {"runner": env["SELF_HOSTED_RUNNER"], "mode": "self-hosted"})
                self.assertIn(f"inputs[platform]={env['PROBE']}", calls)
            with self.subTest(key, probe="offline"):
                env, out, calls = self.route(job, *queue, None)
                self.assertEqual(out, {"runner": env["HOSTED_RUNNER"], "mode": "hosted"})
                self.assertIn("actions/runs/77/cancel", calls)
            with self.subTest(key, probe="untrusted ref"):
                env, out, calls = self.route(job, "push", "refs/heads/factory/x", ("in_progress", "null"))
                self.assertEqual(out, {"runner": env["HOSTED_RUNNER"], "mode": "hosted"})
                self.assertEqual(calls, "")


if __name__ == "__main__":
    unittest.main()
