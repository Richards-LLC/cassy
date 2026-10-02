#!/usr/bin/env python3
"""Prove a merge preview before moving either branch or touching either checkout.

Projects opt in by shipping scripts/release-gate.sh and this runner. A detached
preview includes the target's other lanes, so checks cannot miss a regression
introduced by their composition. The temporary commit never advances a ref.
"""

import argparse
import csv
import math
import os
from pathlib import Path
import signal
import shlex
import subprocess
import sys
import tempfile

from importlib.util import module_from_spec, spec_from_file_location


def git(repo, *args, **kwargs):
    return subprocess.check_output(["git", "-C", str(repo), *args], **kwargs).decode().strip()


def fast_rows_budget(timeout_secs=None):
    """Allow contention, but keep both automatic and explicit retries bounded."""
    if timeout_secs is not None:
        if type(timeout_secs) is not int or not 1 <= timeout_secs <= 1800:
            raise ValueError("--timeout-secs must be a whole number between 1 and 1800")
        return timeout_secs
    cores = max(1, os.cpu_count() or 1)
    try:
        load = os.getloadavg()[0]
    except (AttributeError, OSError):
        load = 0
    load = max(0, load) if math.isfinite(load) else 0
    return min(600, math.ceil(60 * (1 + load / cores)))


def unfinished_rows(row_dir):
    plan = row_dir / "plan.txt"
    if not plan.is_file():
        return ["gate-initialization (row plan not yet available)"]
    planned = plan.read_text().splitlines()
    timing = row_dir / "timing.tsv"
    completed = set()
    if timing.is_file():
        with timing.open() as stream:
            completed = {row.get("row") for row in csv.DictReader(stream, delimiter="\t") if row.get("row")}
    return [name for name in planned if name and name not in completed]


def check_merge(repo, target, source, timeout_secs=None):
    budget = fast_rows_budget(timeout_secs)
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
            row_dir = Path(scratch) / "rows"
            gate_env = dict(os.environ, CAS_RELEASE_GATE_LOG_DIR=str(row_dir))
            origin = "load-aware default" if timeout_secs is None else "explicit retry"
            print(f"fast-rows: wall budget={budget}s ({origin})", flush=True)
            # Start a separate session so a timeout kills the row's descendants
            # as well as Bash. No Cargo identity/cache probes run in fast mode.
            with subprocess.Popen(
                ["bash", str(gate), "--fast-rows", "--base", target_sha],
                cwd=preview, start_new_session=True, env=gate_env,
            ) as process:
                try:
                    status = process.wait(timeout=budget)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    pending = ", ".join(unfinished_rows(row_dir)) or "gate-finalization"
                    if budget < 1800:
                        retry = shlex.join(["python3", str(Path(__file__).resolve()), str(repo), target, source,
                                            "--timeout-secs", str(min(1800, budget * 2))])
                    else:
                        retry = "reduce factory load, then rerun preflight (1800s maximum)"
                    raise ValueError(f"fast-rows: exceeded {budget}s; unfinished rows: {pending}; "
                                     f"retry: {retry}; merge refused") from None
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
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument("repo")
        parser.add_argument("target")
        parser.add_argument("source")
        parser.add_argument("--timeout-secs", type=int, help="bounded supervisor retry (1–1800 seconds)")
        args = parser.parse_args()
        sys.exit(check_merge(args.repo, args.target, args.source, args.timeout_secs))
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"lane preflight: {error}", file=sys.stderr)
        sys.exit(1)
