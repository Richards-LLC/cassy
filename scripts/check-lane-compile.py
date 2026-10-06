#!/usr/bin/env python3
"""Require a capped compile receipt for the combined Rust merge tree.

Verification never starts Cargo. --prove is a supervisor operation: two existing
capped worker-check invocations cover libraries and tests without broadening the
worker runner's command contract. Receipts live beside the common Git metadata,
so deleting a disposable preview does not discard its proof.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import subprocess
import sys
import tempfile
import tomllib


def git(repo, *args, **kwargs):
    return subprocess.check_output(["git", "-C", str(repo), *args], **kwargs).decode().strip()


def common_dir(repo):
    return Path(git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir")).resolve()


def resolve_zig(repo, tree):
    # Ignore unrelated Rust projects. The preview may introduce Ghostty, so
    # inspect the candidate tree rather than only the source checkout.
    if subprocess.run(["git", "-C", str(repo), "cat-file", "-e",
                       f"{tree}:crates/ghostty_vt_sys/build.rs"],
                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
        return None
    candidates = []
    configured = os.environ.get("ZIG", "")
    if configured:
        candidates.append(Path(configured))
    if "PATH" in os.environ:
        candidates.extend(Path(directory) / "zig" for directory in os.environ["PATH"].split(os.pathsep))
    candidates.extend((repo / ".context/zig/zig", common_dir(repo).parent / ".context/zig/zig"))
    for candidate in candidates:
        candidate = candidate if candidate.is_absolute() else repo / candidate
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate.resolve()
    raise ValueError("missing Zig compiler in ZIG, PATH or source/main checkout .context.\n"
                     "Run ./scripts/bootstrap-zig.sh in the source repo, or set an absolute ZIG.")


def package_at(repo, tree, directory):
    path = str(directory / "Cargo.toml")
    result = subprocess.run(["git", "-C", str(repo), "show", f"{tree}:{path}"],
                            capture_output=True, text=True)
    if result.returncode:
        return None
    name = tomllib.loads(result.stdout).get("package", {}).get("name")
    if name is not None and not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]*", name):
        raise ValueError(f"invalid package name in {path}")
    return name


def required_packages(repo, base, tree):
    paths = git(repo, "diff", "--no-renames", "--name-only", "-z", base, tree).split("\0")
    rust = [path for path in paths if path.endswith(".rs")]
    packages = set()
    for path in rust:
        # Check the old manifest as well, so deleted/renamed Rust paths cannot
        # disappear from compile scope. Unowned Rust fails closed.
        directory = PurePosixPath(path).parent
        for candidate in (directory, *directory.parents):
            name = package_at(repo, tree, candidate) or package_at(repo, base, candidate)
            if name:
                packages.add(name)
                break
        else:
            raise ValueError(f"Rust path has no owning Cargo package: {path}; merge refused")
    return sorted(packages)


def merged_tree(repo, target, source):
    target = git(repo, "rev-parse", "--verify", f"{target}^{{commit}}")
    source = git(repo, "rev-parse", "--verify", f"{source}^{{commit}}")
    tree = git(repo, "merge-tree", "--write-tree", target, source).splitlines()[0]
    return target, source, tree


def receipt_path(repo, tree):
    return common_dir(repo) / "lane-compile" / f"{tree}.json"


def verify(repo, tree, packages):
    path = receipt_path(repo, tree)
    try:
        proof = json.loads(path.read_text())
    except (OSError, ValueError):
        return False
    return (isinstance(proof, dict) and proof.get("version") == 1
            and proof.get("result") == "PASS" and proof.get("tree") == tree
            and proof.get("git_common_dir") == str(common_dir(repo))
            and proof.get("targets") == ["--lib", "--tests"]
            and proof.get("packages") == packages
            and proof.get("capped_runner") == "cas factory worker-check")


def require(repo, target, source, tree=None):
    repo = Path(repo).resolve()
    if tree is None:
        base, _, tree = merged_tree(repo, target, source)
    else:
        base = git(repo, "rev-parse", "--verify", f"{target}^{{commit}}")
        tree = git(repo, "rev-parse", "--verify", f"{tree}^{{tree}}")
    packages = required_packages(repo, base, tree)
    if not packages:
        print("PASS lane compile: not required (no Rust delta)")
        return
    if not verify(repo, tree, packages):
        # The actual-merge verifier may run in a disposable checkout. Its
        # remediation must still work after that checkout has been removed.
        command = (shlex.join(["git", "-C", str(repo), "show", f"{tree}:scripts/check-lane-compile.py"])
                   + " | " + shlex.join(["python3", "-", str(repo), target, source, "--prove"]))
        package_args = [arg for package in packages for arg in ("-p", package)]
        checks = "; ".join(shlex.join(["cargo", "check", *package_args, selector])
                           for selector in ("--lib", "--tests"))
        raise ValueError(f"LANE COMPILE REQUIRED: no matching capped --lib/--tests PASS "
                         f"for merged tree {tree}, packages={','.join(packages)}; merge refused.\n"
                         f"Checks (through the capped runner): {checks}\nRun: {command}")
    print(f"PASS lane compile: tree={tree} packages={','.join(packages)} receipt={receipt_path(repo, tree)}")


def prove(repo, target, source):
    repo = Path(repo).resolve()
    base, source_sha, tree = merged_tree(repo, target, source)
    packages = required_packages(repo, base, tree)
    if not packages:
        return require(repo, target, source, tree)
    root = common_dir(repo).parent / ".cas"
    if not root.is_dir():
        raise ValueError(f"capped lane check requires existing Cassy root: {root}")
    previews = root / "worktrees"
    previews.mkdir(exist_ok=True)
    path = receipt_path(repo, tree)
    path.parent.mkdir(exist_ok=True)
    # Serialize retries for this tree; a failed retry must erase an earlier PASS.
    import fcntl
    with path.with_suffix(".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path.unlink(missing_ok=True)
        zig = resolve_zig(repo, tree)
        env = dict(os.environ, GIT_AUTHOR_NAME="Lane compile", GIT_AUTHOR_EMAIL="lane@example.invalid",
                   GIT_COMMITTER_NAME="Lane compile", GIT_COMMITTER_EMAIL="lane@example.invalid")
        if zig is not None:
            env["ZIG"] = str(zig)
        commit = git(repo, "commit-tree", tree, "-p", base, "-p", source_sha,
                     input=b"Capped lane compile preview\n", env=env)
        with tempfile.TemporaryDirectory(prefix="lane-compile-", dir=previews) as scratch:
            # The installed target owner accepts only direct worktrees children.
            # Keep metadata outside the clean checkout, in its unique sibling.
            preview = Path(scratch).with_name(Path(scratch).name + "-preview")
            # Durable provenance plus an OS lifetime lock lets explicit GC
            # distinguish a crashed preview from a live proof. Naming alone
            # must never authorize checkout removal.
            owner = (Path(scratch) / ".cas-lane-compile.lock").open("a")
            fcntl.flock(owner, fcntl.LOCK_EX)
            (Path(scratch) / ".cas-lane-compile.json").write_text(json.dumps({
                "version": 1, "git_common_dir": str(common_dir(repo)), "head": commit,
                "worktree": str(preview),
            }) + "\n")
            try:
                git(repo, "worktree", "add", "--detach", str(preview), commit, stderr=subprocess.STDOUT)
                package_args = [arg for package in packages for arg in ("-p", package)]
                for index, selector in enumerate(("--lib", "--tests")):
                    command = [os.environ.get("CAS_LANE_CHECK_CAS", "cas"), "factory", "worker-check",
                               "--cas-root", str(root), "--", *package_args, selector]
                    print("Capped check: " + shlex.join(command), flush=True)
                    # cas-f616: a later step continues the PASS just recorded at this
                    # commit, so the load its predecessor raised does not refuse it.
                    # worker-check verifies that PASS; the builder cap still applies.
                    step_env = dict(env, CAS_WORKER_CHECK_CONTINUES_PASS="1") if index else env
                    subprocess.run(command, cwd=preview, env=step_env, check=True)
                    key = hashlib.sha256(os.fsencode(preview.resolve())).hexdigest()
                    worker_receipt = root / "worker-checks" / key / f"{commit}.json"
                    try:
                        checked = json.loads(worker_receipt.read_text())
                    except (OSError, ValueError):
                        raise ValueError(f"{selector}: capped worker PASS receipt missing") from None
                    if (not isinstance(checked, dict) or checked.get("head") != commit
                            or checked.get("repo") != str(preview.resolve())
                            or checked.get("packages") != packages or checked.get("test") is not None):
                        raise ValueError(f"{selector}: capped worker PASS receipt does not match preview")
                if (git(preview, "status", "--porcelain", "--untracked-files=all")
                        or git(preview, "rev-parse", "HEAD") != commit
                        or git(preview, "rev-parse", "HEAD^{tree}") != tree):
                    raise ValueError("compile preview changed; no merged-tree PASS recorded")
                if (git(repo, "rev-parse", target) != base
                        or git(repo, "rev-parse", source) != source_sha):
                    raise ValueError("source or target moved during compile; retry proof")
                proof = {"version": 1, "result": "PASS", "git_common_dir": str(common_dir(repo)),
                         "tree": tree, "base": base, "source": source_sha, "head": commit,
                         "packages": packages, "targets": ["--lib", "--tests"],
                         "capped_runner": "cas factory worker-check"}
                temporary = path.with_suffix(".partial")
                temporary.write_text(json.dumps(proof, sort_keys=True) + "\n")
                temporary.replace(path)
            finally:
                try:
                    if preview.exists():
                        git(repo, "worktree", "remove", "--force", str(preview))
                finally:
                    fcntl.flock(owner, fcntl.LOCK_UN)
                    owner.close()
        require(repo, base, source_sha, tree)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo")
    parser.add_argument("target")
    parser.add_argument("source")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--prove", action="store_true")
    group.add_argument("--tree", help="verify the actual detached candidate tree before ref advancement")
    args = parser.parse_args()
    if args.prove:
        prove(args.repo, args.target, args.source)
    else:
        require(args.repo, args.target, args.source, args.tree)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"lane compile: {error}", file=sys.stderr)
        sys.exit(1)
