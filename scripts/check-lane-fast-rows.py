#!/usr/bin/env python3
"""Prove a merge preview before moving either branch or touching either checkout.

Projects opt in by shipping scripts/release-gate.sh and this runner. A detached
preview includes the target's other lanes, so checks cannot miss a regression
introduced by their composition. The temporary commit never advances a ref.
"""

import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

from importlib.util import module_from_spec, spec_from_file_location


def git(repo, *args, **kwargs):
    return subprocess.check_output(["git", "-C", str(repo), *args], **kwargs).decode().strip()


def check_merge(repo, target, source):
    repo = Path(repo).resolve()
    target_sha = git(repo, "rev-parse", "--verify", f"{target}^{{commit}}")
    source_sha = git(repo, "rev-parse", "--verify", f"{source}^{{commit}}")
    tree = git(repo, "merge-tree", "--write-tree", target_sha, source_sha).splitlines()[0]
    env = dict(os.environ, GIT_AUTHOR_NAME="Lane preflight", GIT_AUTHOR_EMAIL="lane@example.invalid",
               GIT_COMMITTER_NAME="Lane preflight", GIT_COMMITTER_EMAIL="lane@example.invalid")
    commit = git(repo, "commit-tree", tree, "-p", target_sha, "-p", source_sha,
                 input=b"No-build lane merge preview\n", env=env)
    with tempfile.TemporaryDirectory(prefix="cas-fast-rows-") as scratch:
        preview = Path(scratch) / "preview"
        try:
            git(repo, "worktree", "add", "--detach", str(preview), commit, stderr=subprocess.STDOUT)
            gate = preview / "scripts/release-gate.sh"
            if not gate.is_file():
                raise ValueError("fast-rows: merged tree removed scripts/release-gate.sh")
            # Start a separate session so a timeout kills the row's descendants
            # as well as Bash. No Cargo identity/cache probes run in fast mode.
            with subprocess.Popen(
                ["bash", str(gate), "--fast-rows", "--base", target_sha],
                cwd=preview, start_new_session=True,
            ) as process:
                try:
                    status = process.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    raise ValueError("fast-rows: exceeded 30s; merge refused") from None
            if status:
                raise ValueError(f"fast-rows: named row(s) above failed (exit {status}); merge refused")
            compiler = Path(__file__).with_name("check-lane-compile.py")
            if not compiler.is_file():
                raise ValueError("lane compile verifier missing; merge refused")
            spec = spec_from_file_location("lane_compile", compiler)
            module = module_from_spec(spec)
            spec.loader.exec_module(module)
            module.require(repo, target_sha, source_sha, tree)
            if git(repo, "rev-parse", target) != target_sha or git(repo, "rev-parse", source) != source_sha:
                raise ValueError("fast-rows: source or target moved during validation; retry preflight")
            print(f"PASS lane fast rows: target={target_sha} source={source_sha}")
            return 0
        finally:
            if preview.exists():
                git(repo, "worktree", "remove", "--force", str(preview))


if __name__ == "__main__":
    try:
        if len(sys.argv) != 4:
            raise ValueError("usage: check-lane-fast-rows.py <repo> <target> <source>")
        sys.exit(check_merge(*sys.argv[1:]))
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"lane preflight: {error}", file=sys.stderr)
        sys.exit(1)
