#!/usr/bin/env python3
"""Rule-175 completion gate: published bytes, clean install, host and main coverage.

Usage: release-completion.py <version> <run-dir> <release-worktree>
This is a post-publication/host-update check, never a build or CI lane.
Announcement receipts deliberately are not inputs: an explicit embargo cannot
waive publication or install proof, and cannot prevent either from completing.
"""
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib


class Blocker(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise Blocker(message)


def command(args, *, cwd=None, env=None):
    result = subprocess.run(args, cwd=cwd, env=env, capture_output=True,
                            text=True, timeout=120)
    require(result.returncode == 0, f"{args[0]} {args[1]} failed (exit {result.returncode})")
    return result.stdout.strip()


def fields(path):
    result = {}
    for line in path.read_text().splitlines():
        key, separator, value = line.partition("=")
        require(separator and key not in result, f"malformed or duplicate field in {path.name}")
        result[key] = value
    return result


def digest(data):
    return hashlib.sha256(data).hexdigest()


def verify(version, run, worktree, evidence):
    require(re.fullmatch(r"\d+\.\d+\.\d+", version), "expected a stable X.Y.Z version")
    tag = f"v{version}"
    landed = (run / "landed-main.sha").read_text().strip()
    require(re.fullmatch(r"[0-9a-f]{40}", landed), "missing landed-main SHA")
    git = lambda *args: command(["git", "-C", str(worktree), *args])
    # Refresh main and the tag instead of trusting an old release-run snapshot.
    git("fetch", "--tags", "--no-recurse-submodules", "origin", "main")
    main = git("rev-parse", "refs/remotes/origin/main^{commit}")
    released = git("rev-parse", f"refs/tags/{tag}^{{commit}}")
    require(released == landed, "release tag does not contain the landed tree")
    require(git("merge-base", main, released) == released,
            "published runtime tree has not landed on main")
    require(git("merge-base", main, released) == main,
            "main has merged changes absent from the published runtime; publish a new version")
    # All changes since the previous release, including earlier session merges,
    # are covered by this interval; never accept a hand-picked list of commits.
    older = []
    for name in git("tag", "--merged", f"{landed}^", "--list", "v*").splitlines():
        match = re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)", name)
        if match:
            older.append((tuple(map(int, match.groups())), name))
    require(older, "previous release tag is missing; cannot prove a version bump")
    previous_version, previous_tag = max(older)
    require(tuple(map(int, version.split("."))) > previous_version,
            "version was not bumped beyond the previous release")
    previous = git("rev-parse", f"refs/tags/{previous_tag}^{{commit}}")
    require(previous != landed, "release contains no changes since the previous release")
    manifest = tomllib.loads(git("show", f"{landed}:cas-cli/Cargo.toml"))
    require(isinstance(manifest.get("package"), dict)
            and manifest["package"].get("version") == version, "landed runtime manifest version differs")
    workflow = json.loads((run / "release-workflow.json").read_text())
    require(isinstance(workflow, dict) and workflow.get("headSha") == landed and workflow.get("headBranch") == tag
            and workflow.get("status") == "completed" and workflow.get("conclusion") == "success",
            "missing successful publication workflow for the landed SHA/tag")
    publication = fields(run / "release-published.receipt")
    require(publication.get("TAG") == tag and publication.get("PUBLISHED_AT"),
            "missing published-runtime receipt for this version")
    host = json.loads((run / "host-update.json").read_text())
    require(isinstance(host, dict) and host.get("status") == "PASS" and host.get("version") == version
            and host.get("hub_running") is True
            and all(host.get(key) == version for key in
                    ("cas_version", "hub_version", "refresh_binary_version")),
            "missing successful host install/update convergence receipt")
    target = {("Linux", "x86_64"): ("x86_64-unknown-linux-gnu", "LINUX"),
              ("Darwin", "arm64"): ("aarch64-apple-darwin", "MACOS")}.get(
                  (platform.system(), platform.machine()))
    require(target, "clean install probe requires a supported release host")
    triple, prefix = target
    asset = f"cas-{triple}.tar.gz"
    expected_digest = publication.get(f"{prefix}_SHA256", "")
    require(publication.get(f"{prefix}_ASSET") == asset
            and re.fullmatch(r"[0-9a-f]{64}", expected_digest), "missing published asset digest")
    gh = os.environ.get("CAS_RELEASE_TRAIN_GH", "gh")
    repo = os.environ.get("CAS_RELEASE_TRAIN_REPO", "Richards-LLC/cassy")
    live = json.loads(command([gh, "release", "view", tag, "--repo", repo,
                               "--json", "isDraft,publishedAt,assets"]))
    require(isinstance(live, dict) and live.get("isDraft") is False
            and live.get("publishedAt") == publication["PUBLISHED_AT"],
            "runtime is not published or publication receipt is stale")
    require(isinstance(live.get("assets"), list) and all(isinstance(item, dict) for item in live["assets"]),
            "published asset metadata is malformed")
    for system in ("LINUX", "MACOS"):
        published_digest = publication.get(f"{system}_SHA256", "")
        require(re.fullmatch(r"[0-9a-f]{64}", published_digest)
                and any(item.get("name") == publication.get(f"{system}_ASSET")
                        and item.get("digest") == f"sha256:{published_digest}"
                        for item in live.get("assets", [])),
                f"{system} published asset receipt differs from GitHub")
    with tempfile.TemporaryDirectory(prefix="cas-clean-install-") as temporary:
        root = Path(temporary)
        command([gh, "release", "download", tag, "--repo", repo,
                 "--dir", str(root), "--pattern", asset])
        archive = root / asset
        require(digest(archive.read_bytes()) == expected_digest, "published archive digest differs")
        with tarfile.open(archive, "r:gz") as bundle:
            binaries = [member for member in bundle.getmembers() if member.name in ("cas", "./cas")]
            require(len(binaries) == 1 and binaries[0].isfile(),
                    "published archive must contain one regular cas binary")
            binary_bytes = bundle.extractfile(binaries[0]).read()
        home = root / "home"
        installed = home / ".local" / "bin" / "cas"
        installed.parent.mkdir(parents=True)
        installed.write_bytes(binary_bytes)
        installed.chmod(0o755)
        output = command([str(installed), "--version"], cwd=root,
                         env={"HOME": str(home), "PATH": f"{installed.parent}:{os.defpath}",
                              "LANG": "C", "XDG_CONFIG_HOME": str(home / ".config"),
                              "XDG_DATA_HOME": str(home / ".local/share")})
        build = git("rev-parse", "--short=7", landed)
        require(re.fullmatch(rf"cas {re.escape(version)} \({build} \d{{4}}-\d{{2}}-\d{{2}}\)", output),
                "clean installed binary version/build differs from the landed clean tree")
        host_command = os.environ.get("CAS_RELEASE_TRAIN_CAS", "cas")
        host_path = shutil.which(host_command)
        require(host_path and digest(Path(host_path).read_bytes()) == digest(binary_bytes),
                "updated host binary differs from the clean installed published binary")
    # Asset download can outlast another merge. Do not complete from the main
    # snapshot taken before the isolated install finished.
    git("fetch", "--no-tags", "origin", "main")
    require(git("rev-parse", "refs/remotes/origin/main^{commit}") == main,
            "main changed during completion; retry against the new merged changes")
    embargo = os.environ.get("CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO")
    if embargo is None and (run / "announcement-embargo.txt").exists():
        embargo = (run / "announcement-embargo.txt").read_text().strip()
    evidence.update(tag=tag, landed_sha=landed, required_main_sha=main,
                    previous_tag=previous_tag, previous_sha=previous,
                    covered_commits=git("rev-list", f"{previous}..{landed}").splitlines(),
                    published_asset=asset, archive_sha256=expected_digest,
                    binary_sha256=digest(binary_bytes), clean_install_version=output,
                    host_binary=str(Path(host_path).resolve()),
                    publication_receipt_sha256=digest((run / "release-published.receipt").read_bytes()),
                    host_receipt_sha256=digest((run / "host-update.json").read_bytes()),
                    announcement_embargo=embargo or None)


def main():
    if len(sys.argv) != 4:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    version, run, worktree = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
    evidence = {"schema": 1, "stage": "delivery-completion", "version": version,
                "checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}
    try:
        verify(version, run, worktree, evidence)
        evidence["status"] = "PASS"
    except (OSError, ValueError, KeyError, tarfile.TarError, subprocess.SubprocessError) as exc:
        evidence.update(status="FAIL", blocker=str(exc))
    run.mkdir(parents=True, exist_ok=True)
    receipt = run / "delivery-completion.json"
    temporary = receipt.with_suffix(f".tmp.{os.getpid()}")
    temporary.write_text(json.dumps(evidence, indent=2) + "\n")
    temporary.replace(receipt)
    if evidence["status"] == "FAIL":
        (run / "stage.host-update.done").unlink(missing_ok=True)
    print(f"{evidence['status']} delivery-completion: {evidence.get('blocker', version)}; receipt {receipt}",
          file=sys.stderr if evidence["status"] == "FAIL" else sys.stdout)
    return 0 if evidence["status"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
