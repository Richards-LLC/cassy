#!/usr/bin/env python3
"""Exercise the release deployer with real git/tag bytes and fake service edges."""
import contextlib
import hashlib
from http.client import IncompleteRead
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import unquote, urlparse


SCRIPT = Path(__file__).with_name("deploy-hub-release.py")
SPEC = importlib.util.spec_from_file_location("deploy_hub", SCRIPT)
hub = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(hub)
REAL_RUN = subprocess.run


class Response(io.BytesIO):
    status = 200

    def __init__(self, data, read_error=None):
        super().__init__(data)
        self.read_error = read_error

    def read(self, *args):
        if self.read_error:
            raise self.read_error
        return super().read(*args)


class DeployTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        self.env = os.environ.copy()
        for key in REAL_RUN(["git", "rev-parse", "--local-env-vars"], capture_output=True,
                            text=True, check=True).stdout.split():
            self.env.pop(key, None)
        self.env.update(VERCEL_HUB_TOKEN="fixture-secret", GITHUB_REPOSITORY="example/cassy",
                        GH_TOKEN="fixture-gh-token", GITHUB_TOKEN="fixture-github-token",
                        HUB_UNRELATED_SECRET="unrelated-secret")
        self.git("init", "-q")
        self.git("config", "user.email", "fixture@example.com")
        self.git("config", "user.name", "Fixture")
        self.old = {"index.html": b"<html>Commander</html>", "app.js": b"old js",
                    "app.css": b"css", "manifest.webmanifest": b"{}"}
        dist = self.repo / "hub-web/dist"
        dist.mkdir(parents=True)
        for name, data in self.old.items():
            (dist / name).write_bytes(data)
        self.git("add", ".")
        self.git("commit", "-qm", "fixture old dist")
        self.git("tag", "v9.99.1")
        self.old_tree = self.git("rev-parse", "HEAD:hub-web/dist")
        self.new = dict(self.old)
        # A nested asset change with identical app.js must still deploy.
        self.new["chunks/lazy.js"] = b"new lazy chunk"
        (dist / "chunks").mkdir()
        (dist / "chunks/lazy.js").write_bytes(self.new["chunks/lazy.js"])
        self.git("add", ".")
        self.git("commit", "-qm", "fixture new dist")
        self.git("tag", "v9.99.2")
        self.tree = self.git("rev-parse", "HEAD:hub-web/dist")
        self.sha = self.git("rev-parse", "HEAD")
        self.calls = []
        self.aliased = "previous"
        self.fail_at = None
        self.prerelease = False
        self.previous_files = self.old
        self.receipt = self.repo / "hub-deployment.json"
        self.assets = []
        self.verifications = []
        self.clock = 0
        self.sleeps = []
        self.patches = [patch.dict(os.environ, self.env, clear=True),
                        patch.object(hub.subprocess, "run", side_effect=self.command),
                        patch.object(hub.HTTP, "open", side_effect=self.http),
                        patch.object(hub, "time", SimpleNamespace(monotonic=lambda: self.clock,
                                                                  sleep=self.sleep))]
        for p in self.patches:
            p.start()
            self.addCleanup(p.stop)

    def git(self, *args):
        return REAL_RUN(["git", "-C", str(self.repo), *args], env=self.env,
                        capture_output=True, text=True, check=True).stdout.strip()

    def command(self, args, **kwargs):
        if args[0].endswith("verify-hub-dist.sh"):
            self.verifications.append(kwargs["env"])
        if args[0] not in ("gh", "vercel"):
            return REAL_RUN(args, **kwargs)
        self.calls.append(args)
        self.assertNotIn("fixture-secret", " ".join(args))
        if args[0] == "vercel":
            for key in ("GH_TOKEN", "GITHUB_TOKEN", "HUB_UNRELATED_SECRET", "VERCEL_HUB_TOKEN"):
                self.assertNotIn(key, kwargs["env"])
            self.assertEqual(kwargs["env"]["VERCEL_TOKEN"], "fixture-secret")
            self.assertEqual(kwargs["env"]["PATH"], self.env["PATH"])
        else:
            self.assertEqual(os.environ["GH_TOKEN"], "fixture-gh-token")
        if args[:3] == ["gh", "release", "view"]:
            data = json.dumps({"isDraft": False, "isPrerelease": self.prerelease})
        elif args[:3] == ["gh", "release", "upload"]:
            self.assets.append(json.loads(self.receipt.read_text()))
            data = ""
        elif args[1] == "deploy":
            self.assertEqual(kwargs["env"]["VERCEL_TOKEN"], "fixture-secret")
            self.assertTrue({"--prod", "--skip-domain", "--yes"} <= set(args))
            self.assertNotIn("--prebuilt", args)
            site = Path(args[args.index("--cwd") + 1])
            self.assertEqual(json.loads((site / "vercel.json").read_text()),
                             {"$schema": "https://openapi.vercel.sh/vercel.json"})
            self.assertEqual((site / "public/index.html").read_bytes(), self.new["index.html"])
            static = site / "public"
            expected = {"index.html": self.new["index.html"],
                        **{"commander/" + p: b for p, b in self.new.items()}}
            self.assertEqual({p.relative_to(static).as_posix(): p.read_bytes()
                              for p in static.rglob("*") if p.is_file()}, expected)
            self.assertEqual({p.name for p in site.iterdir()}, {".vercel", "vercel.json", "public"})
            # CLI 59 emits structured deployment output in non-interactive mode.
            data = json.dumps({"status": "ok", "deployment": {
                "id": "dpl_candidate", "url": "https://candidate.vercel.app", "readyState": "READY"}})
        elif args[1] == "promote":
            target = args[2]
            self.aliased = "previous" if target == "dpl_previous" else "candidate"
            data = ""
        else:
            self.fail(f"unexpected CLI call: {args}")
        return subprocess.CompletedProcess(args, 0, stdout=data, stderr="")

    def http(self, request, **kwargs):
        url = urlparse(request.full_url)
        if url.hostname == "api.vercel.com":
            self.assertEqual(request.get_header("Authorization"), "Bearer fixture-secret")
            if url.path.startswith("/v4/aliases/"):
                data = {"deploymentId": "dpl_previous", "projectId": hub.PROJECT,
                        "deployment": {"id": "dpl_previous", "url": "previous.vercel.app"}}
            elif url.path == "/v13/deployments/dpl_previous":
                data = {"id": "dpl_previous", "url": "previous.vercel.app", "projectId": hub.PROJECT}
            elif url.path == "/v13/deployments/candidate.vercel.app":
                data = {"id": "dpl_candidate", "url": "candidate.vercel.app", "projectId": hub.PROJECT}
            elif url.path == "/v6/deployments/dpl_previous/files":
                def directory(name, children):
                    return {"name": name, "type": "directory", "mode": 16893, "children": children}
                entries = [{"name": n, "type": "file", "mode": 33204,
                            "uid": hashlib.sha1(b).hexdigest()} for n, b in self.previous_files.items()]
                data = [directory("src", [directory("public", [directory("commander", entries)])])]
            else:
                self.fail(f"unexpected API URL: {request.full_url}")
            return Response(json.dumps(data).encode())
        self.assertIsNone(request.get_header("Authorization"), "asset checks must be anonymous")
        variant = (self.aliased if url.hostname == "hub.petrastella.io"
                   else "candidate" if url.hostname == "candidate.vercel.app" else "previous")
        files = self.new if variant == "candidate" else self.previous_files
        path = unquote(url.path)
        name = "index.html" if path in ("/", "/commander/") else path.removeprefix("/commander/")
        # A protected production deployment may return a 200 SSO page.
        if self.fail_at == "live" and variant == "candidate" and url.hostname == "hub.petrastella.io":
            return Response(b"<html>Log in to Vercel</html>")
        if (self.fail_at == "live_once" and variant == "candidate" and
                url.hostname == "hub.petrastella.io" and path == "/"):
            self.fail_at = None
            return Response(self.old["index.html"] + b"stale alias")
        if (isinstance(self.fail_at, Exception) and variant == "candidate" and
                url.hostname == "hub.petrastella.io" and path == "/"):
            error, self.fail_at = self.fail_at, None
            return Response(b"", read_error=error)
        if self.fail_at == "immutable" and url.hostname == "candidate.vercel.app":
            raise HTTPError(request.full_url, 302, "SSO redirect", {}, None)
        return Response(files[name])

    def deploy(self, tag, *extra):
        before = len(self.verifications)
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            result = hub.main([tag, "--repo", str(self.repo), "--receipt", str(self.receipt), *extra])
        verifications = self.verifications[before:]
        expected = 0 if "--dry-run" in extra or self.prerelease else 1
        self.assertEqual(len(verifications), expected, "one dist verifier per real run")
        for env in verifications:
            dist = Path(env["DIST_DIR"])
            self.assertEqual(dist.name, "commander")
            self.assertEqual(Path(env["ORIGIN_INDEX"]), dist.parent / "index.html")
        return result

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.clock += seconds

    def test_unchanged_live_tree_skips_upload_and_dry_runs_send_nothing(self):
        self.assertEqual(self.deploy("dryrun-v9.99.2-123", "--dry-run"), 0)
        self.assertEqual(self.calls, [])
        self.prerelease = True
        self.assertEqual(self.deploy("v9.99.2"), 0)
        self.assertEqual(len(self.calls), 1)  # Only reads the release; no Vercel/asset writes.
        self.calls.clear()
        self.prerelease = False
        # Same live tree at a newer release tag: no hard-coded previous-tag pin.
        self.assertEqual(self.deploy("v9.99.1"), 0)
        self.assertFalse(any(c[0] == "vercel" for c in self.calls))
        receipt = self.assets[-1]
        self.assertEqual(receipt["status"], "unchanged")
        self.assertEqual(receipt["dist_tree"], self.old_tree)
        self.assertEqual(receipt["deployment_id"], "dpl_previous")
        self.assertTrue(receipt["verified_at"])

    def test_changed_dist_is_verified_before_alias_and_failed_live_check_restores_previous(self):
        self.fail_at = "immutable"
        self.assertEqual(self.deploy("v9.99.2"), 1)
        self.assertFalse(any(c[1] == "promote" for c in self.calls if c[0] == "vercel"))
        self.calls.clear()
        self.fail_at = "live"
        self.assertEqual(self.deploy("v9.99.2"), 1)
        promotions = [c[2] for c in self.calls if c[:2] == ["vercel", "promote"]]
        self.assertEqual(promotions, ["dpl_candidate", "dpl_previous"])
        self.assertEqual(self.aliased, "previous")
        self.assertEqual(self.assets[-1]["status"], "rolled_back")
        self.assertEqual(self.assets[-1]["previous_deployment_id"], "dpl_previous")
        self.assertGreaterEqual(sum(self.sleeps), 55, "allow about a minute for alias propagation")
        self.assertLessEqual(sum(self.sleeps), 60, "persistent mismatch must exhaust a bounded retry")
        # Identical app.js but a new lazy asset still reaches upload/promotion.
        self.calls.clear()
        self.fail_at = None
        self.assertEqual(self.deploy("v9.99.2"), 0)
        self.assertEqual([c[2] for c in self.calls if c[:2] == ["vercel", "promote"]], ["dpl_candidate"])
        receipt = self.assets[-1]
        self.assertEqual(receipt["status"], "deployed")
        self.assertEqual(receipt["dist_tree"], self.tree)
        self.assertEqual(receipt["source_sha"], self.sha)
        self.assertEqual(receipt["deployment_id"], "dpl_candidate")

    def test_alias_propagation_mismatch_retries_then_keeps_the_verified_deployment(self):
        self.fail_at = "live_once"
        self.assertEqual(self.deploy("v9.99.2"), 0)
        self.assertEqual(self.assets[-1]["status"], "deployed")
        self.assertEqual(self.aliased, "candidate")
        self.assertEqual([c[2] for c in self.calls if c[:2] == ["vercel", "promote"]], ["dpl_candidate"])
        self.assertEqual(len(self.sleeps), 1)
        self.assertGreater(self.sleeps[0], 0)

    def test_dropped_live_response_body_retries_without_rolling_back(self):
        for error in (IncompleteRead(b"partial", 10), ConnectionResetError("fixture connection reset")):
            with self.subTest(error=type(error).__name__):
                self.calls.clear()
                self.sleeps.clear()
                self.aliased = "previous"
                self.fail_at = error
                result = self.deploy("v9.99.2")
                self.assertEqual(result, 0, self.assets[-1])
                self.assertEqual(self.assets[-1]["status"], "deployed")
                self.assertEqual(self.aliased, "candidate")
                self.assertEqual([c[2] for c in self.calls if c[:2] == ["vercel", "promote"]],
                                 ["dpl_candidate"])
                self.assertEqual(len(self.sleeps), 1)


if __name__ == "__main__":
    unittest.main()
