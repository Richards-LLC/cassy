#!/usr/bin/env python3
"""Build an exact-tip QA binary with the release environment and owned scratch."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

import release_env
import release_scratch


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run(scope, command, **kwargs):
    print("candidate: " + " ".join(map(str, command)), file=sys.stderr, flush=True)
    scope.run(list(map(str, command)), check=True, **kwargs)


def verify_candidate_version(output, declared, revision):
    match = re.fullmatch(r"cas (\S+) \(([0-9a-f]{7}) (\d{4}-\d{2}-\d{2})\)", output)
    if not match:
        raise ValueError(f"candidate version has unexpected format: {output!r}")
    if match.group(1) != declared or not revision.startswith(match.group(2)):
        raise ValueError(
            f"candidate version {output!r} differs from revision's cas {declared} ({revision[:7]})"
        )


def build(repo, revision, candidate_dir):
    repo = repo.resolve()
    resolved = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "--verify", "--end-of-options", f"{revision}^{{commit}}"],
        text=True,
    ).strip()
    if revision != resolved:
        raise ValueError(f"--rev must be the full commit SHA ({resolved})")
    candidate_dir = candidate_dir.absolute()
    scratch = release_env.resolve_scratch(repo, durable=True, create=True)
    if scratch.problem or not scratch.base:
        raise ValueError(f"release scratch unavailable: {scratch.problem}; {scratch.fix}")
    with release_scratch.ChildScope() as scope:
        with (
            release_scratch.OwnedDirectory(
                Path(scratch.base).name + ".qa-candidate-", Path(scratch.base).parent,
            ) as owned,
            release_scratch.BoundedCache(owned / "target", os.environ, repo) as target,
        ):
            # Private, size/age bounded output lives only for this candidate.
            # Legacy supervisor/main targets remain review-only.
            checkout = owned / "checkout"
            run(scope, ["git", "-C", repo, "worktree", "add", "--detach", checkout, revision])
            try:
                zig = release_env.resolve_zig(checkout, link=True)
                if not zig.path:
                    raise ValueError(f"Zig unavailable: {'; '.join(zig.tried)}; {zig.fix}")
                env = dict(os.environ, ZIG=zig.path, CARGO_TARGET_DIR=str(target),
                           PATH=str(Path(zig.path).parent) + os.pathsep + os.environ.get("PATH", ""))
                run(scope, ["cargo", "build", "-p", "cas", "--bin", "cas", "--locked"],
                    cwd=checkout, env=env, stdout=sys.stderr, stderr=sys.stderr)
                binary = target / "debug" / "cas"
                if not binary.is_file():
                    raise ValueError(f"candidate build produced no binary at {binary}")
                declared = tomllib.loads((checkout / "cas-cli/Cargo.toml").read_text())["package"]["version"]
                task_dir = candidate_dir.parent
                task_dir.mkdir(parents=True, exist_ok=True)
                if task_dir.is_symlink() or candidate_dir.is_symlink():
                    raise ValueError("candidate artifact path must not be a symlink")
                stage = Path(tempfile.mkdtemp(prefix=".candidate-", dir=task_dir))
                try:
                    shutil.copy2(binary, stage / "cas")
                    version = subprocess.check_output([str(stage / "cas"), "--version"], text=True).strip()
                    verify_candidate_version(version, declared, revision)
                    (stage / "VERSION.txt").write_text(f"{version}\nrevision={revision}\n")
                    binary_hash = sha256(stage / "cas")
                    version_hash = sha256(stage / "VERSION.txt")
                    receipt = {"revision": revision, "version": version,
                               "sha256": binary_hash, "zig": zig.path,
                               "zig_source": zig.source, "cargo_target": str(target)}
                    (stage / "RECEIPT.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
                    receipt_hash = sha256(stage / "RECEIPT.json")
                    (stage / "SHA256SUMS").write_text(
                        f"{binary_hash}  cas\n{version_hash}  VERSION.txt\n"
                        f"{receipt_hash}  RECEIPT.json\n")
                    retired = task_dir / f".candidate-retired-{os.getpid()}"
                    if candidate_dir.exists():
                        if retired.exists():
                            raise ValueError(f"old candidate backup already exists at {retired}")
                        candidate_dir.rename(retired)
                    try:
                        stage.rename(candidate_dir)
                    except BaseException:
                        if retired.exists():
                            retired.rename(candidate_dir)
                        raise
                    if retired.exists():
                        shutil.rmtree(retired)
                finally:
                    if stage.exists():
                        shutil.rmtree(stage)
            finally:
                run(scope, ["git", "-C", repo, "worktree", "remove", "--force", checkout])
    return {"candidate_dir": str(candidate_dir), "revision": revision,
            "version": version, "sha256": binary_hash}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--rev", required=True)
    parser.add_argument("--candidate-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(build(args.repo, args.rev, args.candidate_dir), sort_keys=True))
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"candidate: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
