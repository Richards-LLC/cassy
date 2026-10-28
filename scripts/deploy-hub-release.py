#!/usr/bin/env python3
"""Upload the published tag's Commander bytes, verify, promote, and receipt them.

Runs only after publication. No build, git-sourced deployment, or token arguments.
The first run discovers the manual deployment through Vercel's source-file tree;
later runs use the same path rather than trusting a locally pinned release.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
from http.client import HTTPException
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import tempfile
import sys
import time
from urllib.parse import quote
from urllib.request import HTTPRedirectHandler, Request, build_opener


PROJECT = "prj_tk3Jeh4OZ4YYoP7jbjiERqzrOXey"  # cas-hub-static
TEAM = "team_oYRRKRaKgk6MHCVvkwCePax5"  # richards-llc
ORIGIN = "https://hub.petrastella.io"
SCHEMA = "https://openapi.vercel.sh/vercel.json"


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


HTTP = build_opener(NoRedirect)


def command(args, **kwargs):
    result = subprocess.run(args, capture_output=True, text=True, timeout=900, **kwargs)
    if result.returncode:
        # Neither CLI tokens nor API error bodies belong in a CI log.
        detail = result.stderr
        for key in ("VERCEL_HUB_TOKEN", "VERCEL_TOKEN", "GH_TOKEN"):
            if os.environ.get(key):
                detail = detail.replace(os.environ[key], "[REDACTED]")
        raise RuntimeError(f"{args[0]} {args[1]} failed ({result.returncode}): {detail.strip()}")
    return result.stdout.strip()


def fetch(url, token=None, timeout=60):
    headers = {"Cache-Control": "no-cache"}
    if token:
        headers["Authorization"] = "Bearer " + token
    try:
        # No redirect following or bypass credentials on static-file requests.
        with HTTP.open(Request(url, headers=headers), timeout=timeout) as response:
            if response.status != 200:
                raise RuntimeError(f"expected anonymous HTTP 200: {url}")
            return response.read()
    except (HTTPException, OSError) as error:
        raise RuntimeError(f"HTTP verification failed: {url} ({getattr(error, 'code', 'network error')})") from None


class Vercel:
    def __init__(self):
        self.token = os.environ.get("VERCEL_HUB_TOKEN")
        if not self.token:
            raise RuntimeError("VERCEL_HUB_TOKEN is required")

    def api(self, path):
        return json.loads(fetch("https://api.vercel.com" + path + "?slug=richards-llc", self.token))

    def cli(self, *args):
        # CLI 59 supports VERCEL_TOKEN. The secret never appears on argv.
        # Keep Node's runtime context; GitHub credentials and other job secrets
        # must not cross into the third-party deployment process.
        env = {key: os.environ[key] for key in ("PATH", "HOME", "TMPDIR", "CI") if key in os.environ}
        env.update(VERCEL_TOKEN=self.token, VERCEL_ORG_ID=TEAM,
                   VERCEL_PROJECT_ID=PROJECT, VERCEL_TELEMETRY_DISABLED="1")
        return command(["vercel", *args, "--scope", "richards-llc"], env=env)

    def deployment(self, identifier):
        data = self.api("/v13/deployments/" + quote(identifier, safe=""))
        if data.get("projectId") != PROJECT:
            raise RuntimeError("deployment does not belong to cas-hub-static")
        if not re.fullmatch(r"dpl_[A-Za-z0-9]+", data["id"]):
            raise RuntimeError("invalid deployment id")
        data["origin"] = deployment_origin(data["url"])
        return data


def deployment_origin(url):
    url = url.removeprefix("https://").rstrip("/")
    if not re.fullmatch(r"[a-zA-Z0-9-]+\.vercel\.app", url):
        raise RuntimeError("invalid immutable deployment URL")
    return "https://" + url


def git_tree(snapshot):
    """Git tree identity from live bytes and executable bits, without a git index."""
    def digest(kind, data):
        return hashlib.sha1(kind + b" " + str(len(data)).encode() + b"\0" + data).digest()

    root = {}
    for path, (data, mode) in snapshot.items():
        node = root
        parts = PurePosixPath(path).parts
        for part in parts[:-1]:
            node = node.setdefault(part, {})
        node[parts[-1]] = (data, mode)

    def tree(node):
        entries = []
        for name, value in node.items():
            directory = isinstance(value, dict)
            mode = "40000" if directory else value[1]
            oid = tree(value) if directory else digest(b"blob", value[0])
            entries.append(((name + ("/" if directory else "")).encode(),
                            mode.encode() + b" " + name.encode() + b"\0" + oid))
        return digest(b"tree", b"".join(entry for _, entry in sorted(entries)))

    return tree(root).hex()


def source_dist(repo, tag, scratch):
    sha = command(["git", "-C", str(repo), "rev-parse", tag + "^{commit}"])
    tree = command(["git", "-C", str(repo), "rev-parse", tag + ":hub-web/dist"])
    archive = subprocess.run(["git", "-C", str(repo), "archive", tag, "hub-web/dist"],
                             capture_output=True, check=True).stdout
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        for member in tar.getmembers():
            if not (member.isfile() or member.isdir()):
                raise RuntimeError("release dist contains a symlink or non-regular file")
        tar.extractall(scratch, filter="data")
    dist = scratch / "hub-web/dist"
    snapshot = {p.relative_to(dist).as_posix(): (p.read_bytes(), "100755" if p.stat().st_mode & 0o111 else "100644")
                for p in dist.rglob("*") if p.is_file()}
    for required in ("index.html", "app.js", "app.css"):
        if not snapshot.get(required, (b"",))[0]:
            raise RuntimeError(f"release dist is missing {required}")
    if git_tree(snapshot) != tree:
        raise RuntimeError("extracted dist does not match the tag's git tree")
    return sha, tree, dist, snapshot


def live_snapshot(vercel, deployment):
    entries = vercel.api("/v6/deployments/" + deployment["id"] + "/files")
    candidates = []

    def walk(nodes, parent=()):
        for entry in nodes:
            name = entry["name"]
            if not name or "/" in name or name in (".", ".."):
                raise RuntimeError("unsafe deployment file name")
            path = (*parent, name)
            if entry["type"] == "directory":
                if name == "commander" and parent and parent[-1] in ("public", "static"):
                    candidates.append(entry["children"])
                walk(entry.get("children", []), path)

    walk(entries)
    if len(candidates) != 1:
        raise RuntimeError("expected one public/commander or output/static/commander in live deployment")
    files = {}

    def collect(nodes, prefix=""):
        for entry in nodes:
            path = prefix + entry["name"]
            if entry["type"] == "directory":
                collect(entry["children"], path + "/")
            elif entry["type"] == "file":
                if path in files:
                    raise RuntimeError("duplicate live Commander file")
                files[path] = ("100755" if int(entry["mode"]) & 0o111 else "100644", entry["uid"])
            else:
                raise RuntimeError("live Commander contains a non-static file")

    collect(candidates[0])
    if not {"index.html", "app.js", "app.css"} <= files.keys():
        raise RuntimeError("live deployment lacks required Commander files")

    def download(item):
        name, (mode, uid) = item
        data = fetch(deployment["origin"] + "/commander/" + quote(name))
        if hashlib.sha1(data).hexdigest() != uid:
            raise RuntimeError(f"previous deployment bytes do not match Vercel source: {name}")
        return name, (data, mode)

    with ThreadPoolExecutor(max_workers=8) as pool:
        return dict(pool.map(download, files.items()))


def verify_origin(origin, snapshot, deadline=None):
    expected = {"/commander/" + quote(name): data for name, (data, _) in snapshot.items()}
    expected.update({"/": snapshot["index.html"][0], "/commander/": snapshot["index.html"][0]})

    def verify(item):
        path, data = item
        timeout = 60 if deadline is None else min(60, deadline - time.monotonic())
        if timeout <= 0:
            raise RuntimeError(f"verification deadline exceeded: {origin}")
        if fetch(origin + path, timeout=timeout) != data:
            raise RuntimeError(f"byte mismatch (including possible SSO page): {origin}{path}")

    with ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(verify, expected.items()))
    print(f"verified anonymous HTTP 200 and bytes: {origin} ({len(expected)} paths)")


def verify_after_promote(snapshot):
    # A successful promotion can precede alias propagation. Retry strict byte
    # checks for at most a minute, including their HTTP time, before rollback.
    deadline = time.monotonic() + 60
    while True:
        try:
            verify_origin(ORIGIN, snapshot, deadline=deadline)
            return
        except RuntimeError:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise
            print("hosted hub has not verified yet; retrying after alias propagation", file=sys.stderr)
            time.sleep(min(5, remaining))


def assemble_site(dist, scratch):
    site = scratch / "site"
    public = site / "public"
    shutil.copytree(dist, public / "commander")
    shutil.copyfile(dist / "index.html", public / "index.html")
    command([str(Path(__file__).with_name("verify-hub-dist.sh"))],
            env=dict(os.environ, DIST_DIR=str(public / "commander"), ORIGIN_INDEX=str(public / "index.html")))
    (site / "vercel.json").write_text(json.dumps({"$schema": SCHEMA}) + "\n")
    (site / ".vercel").mkdir()
    (site / ".vercel/project.json").write_text(json.dumps({"orgId": TEAM, "projectId": PROJECT}))
    return site


def upload(vercel, site, tag, sha, tree):
    # Match the proven manual static upload: no package, source checkout or build.
    url = vercel.cli("deploy", "--cwd", str(site), "--yes", "--prod", "--skip-domain",
                     "--meta", "hubDistTree=" + tree, "--meta", "hubSourceTag=" + tag,
                     "--meta", "hubSourceSha=" + sha)
    # Non-interactive CLI 59 emits JSON; interactive/older output is the URL.
    if url.startswith("{"):
        result = json.loads(url)
        url = result.get("deployment", result)["url"]
    return vercel.deployment(deployment_origin(url).removeprefix("https://"))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    if args.dry_run or args.tag.startswith("dryrun-") or os.environ.get("DRY_RUN") == "true":
        print("dry run: no hosted Commander requests")
        return 0
    receipt = None
    try:
        release = json.loads(command(["gh", "release", "view", args.tag, "--repo",
                                      os.environ["GITHUB_REPOSITORY"], "--json", "isDraft,isPrerelease"]))
        if release["isDraft"] or release["isPrerelease"]:
            print("draft/prerelease: no hosted Commander requests")
            return 0
        if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", args.tag):
            raise RuntimeError("expected a stable release tag")
        vercel = Vercel()
        with tempfile.TemporaryDirectory(prefix="hub-release-") as tmp:
            scratch = Path(tmp)
            sha, tree, dist, snapshot = source_dist(args.repo, args.tag, scratch)
            # One verifier run, against the assembled root/Commander pair,
            # including releases whose live tree turns out to be unchanged.
            site = assemble_site(dist, scratch)
            alias = vercel.api("/v4/aliases/hub.petrastella.io")
            if alias.get("projectId") != PROJECT:
                raise RuntimeError("hub domain does not belong to cas-hub-static")
            previous = vercel.deployment(alias["deploymentId"])
            old = live_snapshot(vercel, previous)
            previous_tree = git_tree(old)
            receipt = {"tag": args.tag, "source_sha": sha, "dist_tree": tree,
                       "previous_dist_tree": previous_tree, "previous_deployment_id": previous["id"],
                       "deployment_id": previous["id"], "immutable_url": previous["origin"],
                       "app_js_md5": hashlib.md5(snapshot["app.js"][0]).hexdigest(),
                       "app_css_md5": hashlib.md5(snapshot["app.css"][0]).hexdigest(),
                       "status": "failed"}
            if previous_tree == tree:
                verify_origin(ORIGIN, snapshot)
                receipt["status"] = "unchanged"
            else:
                candidate = upload(vercel, site, args.tag, sha, tree)
                receipt.update(deployment_id=candidate["id"], immutable_url=candidate["origin"])
                verify_origin(candidate["origin"], snapshot)
                # Rollback covers a failed/timeout promotion too: it may have moved aliases.
                try:
                    vercel.cli("promote", candidate["id"], "--yes")
                    verify_after_promote(snapshot)
                except Exception:
                    print(f"promotion/verification failed; restoring {previous['id']}", file=sys.stderr)
                    try:
                        vercel.cli("promote", previous["id"], "--yes")
                        verify_origin(ORIGIN, old)
                        receipt["status"] = "rolled_back"
                        receipt["rollback_verified_at"] = datetime.now(timezone.utc).isoformat()
                    except Exception as restore_error:
                        receipt["status"] = "rollback_failed"
                        raise RuntimeError(f"CRITICAL: restore {previous['id']} failed: {restore_error}") from None
                    raise
                receipt["status"] = "deployed"
            receipt["verified_at"] = datetime.now(timezone.utc).isoformat()
        return 0
    except Exception as error:
        print(f"::error::hosted Commander deploy failed; release remains published: {error}", file=sys.stderr)
        return 1
    finally:
        if receipt is not None:
            receipt["recorded_at"] = datetime.now(timezone.utc).isoformat()
            args.receipt.parent.mkdir(parents=True, exist_ok=True)
            args.receipt.write_text(json.dumps(receipt, indent=2) + "\n")
            print(json.dumps(receipt, indent=2))
            # Failure receipts retain the rollback id even when the hub check failed.
            command(["gh", "release", "upload", args.tag, str(args.receipt), "--clobber",
                     "--repo", os.environ["GITHUB_REPOSITORY"]])


if __name__ == "__main__":
    sys.exit(main())
