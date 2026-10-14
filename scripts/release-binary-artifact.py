#!/usr/bin/env python3
"""Record the gate's audited release binary and let the publisher reuse it.

One local release build per release tree (cas-c096). Before this, the gate's
release-binary-isa row built and audited the x86_64 release binary (614 s),
then release.sh cold-compiled the same tree again in the tag worktree (11 min
of a 24 min publish stage).

  record <repo> --target T --binary B --license L [--audit NAME ...]
      Called by release-gate.sh after every audit passed. Packages B and L
      exactly as release.sh does (cas, LICENSE) and stores the archive with a
      manifest in <store>/<target>/<tree>/.
  verify <repo> --target T [--version V]
      Called by release.sh. Exit 0 prints `REUSE ...` and `archive=<path>`;
      exit 1 prints one `MISS key=<input> ...` line naming the first input
      that differs, and the publisher rebuilds.
  store <repo>
      Print the store directory.

The manifest binds the git tree of a clean checkout, the version that tree
declares, the target, the build identity (rustc -vV, cargo-zigbuild and Zig
versions and the rustflags environment) and the archive and binary digests.
A mismatch in any of them is a rebuild, never an error: the archive is an
accelerator for the publisher's local audit, never the shipped bytes.

The store defaults to <main checkout>/.cas/release-binary/artifacts, the
checkout that owns the gate's epic worktree and the publisher's tag worktree;
CAS_RELEASE_BINARY_STORE overrides it.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import time

SCHEMA = 1
KEEP_PER_TARGET = 6
IDENTITY_ENV = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS")


class Miss(Exception):
    def __init__(self, key, reason):
        super().__init__(f"MISS key={key} reason={reason}")
        self.key = key


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], check=True,
                          capture_output=True, text=True).stdout.strip()


def store_root(repo):
    configured = os.environ.get("CAS_RELEASE_BINARY_STORE")
    if configured:
        return Path(configured)
    common = Path(git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    return common.parent / ".cas/release-binary/artifacts"


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def tree_state(repo):
    """The HEAD tree, refusing a checkout whose tracked files differ from it."""
    if git(repo, "status", "--porcelain", "--untracked-files=no"):
        raise Miss("tree", "tracked_files_modified")
    tree = git(repo, "rev-parse", "HEAD^{tree}")
    manifest = git(repo, "show", "HEAD:cas-cli/Cargo.toml")
    match = re.search(r'^version = "([^"]+)"', manifest, re.M)
    if not match:
        raise Miss("version", "cas-cli/Cargo.toml_declares_none")
    return tree, git(repo, "rev-parse", "HEAD"), match.group(1)


def probe(command, repo):
    try:
        result = subprocess.run(command, cwd=repo, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise Miss("build_identity", f"{Path(command[0]).name}_unavailable:{type(error).__name__}")
    if result.returncode:
        raise Miss("build_identity", f"{Path(command[0]).name}_failed")
    return result.stdout.strip()


def build_identity(repo):
    """Every tool and flag that changes the bytes zigbuild produces."""
    identity = {
        "rustc": probe([os.environ.get("RUSTC", "rustc"), "-vV"], repo),
        "cargo_zigbuild": probe(["cargo-zigbuild", "--version"], repo),
        "zig": probe([os.environ.get("ZIG") or "zig", "version"], repo),
    }
    for name in IDENTITY_ENV:
        identity["env:" + name] = os.environ.get(name)
    return identity


def identity_digest(identity):
    return hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()[:16]


def entry_dir(repo, target, tree, identity):
    """One entry per tree and build identity, so a toolchain round trip
    (upgrade, then revert) finds the earlier archive again."""
    return store_root(repo) / target / f"{tree}-{identity_digest(identity)}"


def first_identity_difference(recorded, current):
    if not isinstance(recorded, dict):
        return "absent"
    for name in sorted(set(recorded) | set(current)):
        if recorded.get(name) != current.get(name):
            return f"{name}_differs"
    return None


def explain_entry_miss(entry, tree, current):
    """A stored archive of this tree under another build identity names the
    component that differs; otherwise the tree has no archive at all."""
    for sibling in sorted(entry.parent.glob(f"{tree}-*")):
        try:
            recorded = json.loads((sibling / "manifest.json").read_text()).get("build_identity")
        except (OSError, ValueError, AttributeError):
            continue
        difference = first_identity_difference(recorded, current)
        if difference:
            return Miss("build_identity", difference)
    return Miss("tree", f"no_artifact_for_tree tree={tree} store={entry.parent}")


def package(binary, license_path, archive):
    """The release.sh archive layout: cas and LICENSE at the root."""
    with tarfile.open(archive, "w:gz") as bundle:
        for source, name in ((binary, "cas"), (license_path, "LICENSE")):
            info = bundle.gettarinfo(str(source), arcname=name)
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            with open(source, "rb") as stream:
                bundle.addfile(info, stream)


def archived_binary_digest(archive):
    with tarfile.open(archive, "r:gz") as bundle:
        names = sorted(bundle.getnames())
        if names != ["LICENSE", "cas"]:
            raise Miss("archive", "unexpected_members")
        member = bundle.getmember("cas")
        if not member.isfile():
            raise Miss("archive", "binary_is_not_a_regular_file")
        stream = bundle.extractfile(member)
        digest = hashlib.sha256()
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
        return digest.hexdigest()


def prune(target_root, keep):
    entries = []
    for path in target_root.iterdir():
        manifest = path / "manifest.json"
        if path.is_dir() and not path.name.startswith(".") and manifest.is_file():
            entries.append((manifest.stat().st_mtime, path))
        elif path.is_dir() and path.name.startswith(".tmp-") and time.time() - path.stat().st_mtime > 86400:
            shutil.rmtree(path, ignore_errors=True)
    for _, path in sorted(entries, reverse=True)[keep:]:
        shutil.rmtree(path, ignore_errors=True)


def record(args):
    repo = args.repo.resolve()
    tree, commit, version = tree_state(repo)
    identity = build_identity(repo)
    destination = entry_dir(repo, args.target, tree, identity)
    destination.parent.mkdir(parents=True, exist_ok=True)
    staging = destination.parent / f".tmp-{tree}-{os.getpid()}"
    shutil.rmtree(staging, ignore_errors=True)
    staging.mkdir()
    try:
        archive_name = f"cas-{args.target}.tar.gz"
        package(args.binary, args.license, staging / archive_name)
        manifest = {
            "schema": SCHEMA,
            "tree": tree,
            "source_commit": commit,
            "version": version,
            "target": args.target,
            "profile": "release",
            "build_identity": identity,
            "archive": archive_name,
            "archive_sha256": sha256(staging / archive_name),
            "binary_sha256": sha256(args.binary),
            "audits": args.audit,
            "recorded_epoch": int(time.time()),
        }
        (staging / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        retired = destination.parent / f".tmp-retired-{tree}-{os.getpid()}"
        if destination.exists():
            destination.rename(retired)
        staging.rename(destination)
        shutil.rmtree(retired, ignore_errors=True)
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    prune(destination.parent, KEEP_PER_TARGET)
    print(f"recorded release binary tree={tree} version={version} target={args.target} "
          f"source_sha={commit} sha256={manifest['archive_sha256']} entry={destination}")
    return 0


def verify(args):
    repo = args.repo.resolve()
    tree, commit, version = tree_state(repo)
    if args.version is not None and args.version != version:
        raise Miss("version", f"tree_declares_{version}_not_{args.version}")
    current = build_identity(repo)
    entry = entry_dir(repo, args.target, tree, current)
    manifest_path = entry / "manifest.json"
    if not manifest_path.is_file():
        raise explain_entry_miss(entry, tree, current)
    try:
        manifest = json.loads(manifest_path.read_text())
    except (OSError, ValueError):
        raise Miss("manifest", "unreadable")
    if not isinstance(manifest, dict) or manifest.get("schema") != SCHEMA:
        raise Miss("manifest", "unknown_schema")
    if manifest.get("tree") != tree:
        raise Miss("tree", f"manifest_tree_{manifest.get('tree')}_not_{tree}")
    if manifest.get("version") != version:
        raise Miss("version", f"artifact_{manifest.get('version')}_not_{version}")
    if manifest.get("target") != args.target:
        raise Miss("target", f"artifact_{manifest.get('target')}_not_{args.target}")
    difference = first_identity_difference(manifest.get("build_identity"), current)
    if difference:
        raise Miss("build_identity", difference)
    archive = entry / str(manifest.get("archive"))
    if manifest.get("archive") != f"cas-{args.target}.tar.gz" or not archive.is_file():
        raise Miss("archive", "missing")
    if sha256(archive) != manifest.get("archive_sha256"):
        raise Miss("archive_sha256", "differs")
    try:
        binary_digest = archived_binary_digest(archive)
    except (OSError, tarfile.TarError):
        raise Miss("archive", "unreadable")
    if binary_digest != manifest.get("binary_sha256"):
        raise Miss("binary_sha256", "differs")
    print(f"REUSE release binary tree={tree} version={version} target={args.target} "
          f"built_from={manifest.get('source_commit')} tag_commit={commit} "
          f"sha256={manifest['archive_sha256']} audits={','.join(manifest.get('audits') or [])}")
    print(f"archive={archive}")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="action", required=True)
    store = sub.add_parser("store")
    store.add_argument("repo", type=Path)
    rec = sub.add_parser("record")
    rec.add_argument("repo", type=Path)
    rec.add_argument("--target", required=True)
    rec.add_argument("--binary", required=True, type=Path)
    rec.add_argument("--license", required=True, type=Path)
    rec.add_argument("--audit", action="append", default=[])
    ver = sub.add_parser("verify")
    ver.add_argument("repo", type=Path)
    ver.add_argument("--target", required=True)
    ver.add_argument("--version")
    args = parser.parse_args()
    try:
        if args.action == "store":
            print(store_root(args.repo.resolve()))
            return 0
        return record(args) if args.action == "record" else verify(args)
    except Miss as miss:
        print(str(miss))
        return 1
    except (OSError, subprocess.CalledProcessError, tarfile.TarError) as error:
        print(f"MISS key=artifact reason=unavailable:{type(error).__name__}: {error}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
