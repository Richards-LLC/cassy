#!/usr/bin/env python3
"""Run the script-tier tests a lane's changes touch (no build).

`make -C cas-cli test-ci-tiers` runs every script test and takes several
minutes, so merge admission runs only the entries whose subject changed:

- `scripts/test-<stem>.*` itself changed, or
- a file named `scripts/<stem>.*` changed (e.g. `scripts/check-foo.py`, its
  fixture `scripts/check-foo.json`), for the test `scripts/test-<stem>.*`.

Entries in HEAVY take longer than lane admission's budget allows (about 90 s
for every fast row together). They are reported as deferred to the full
`ci-script-tests` row, which the integration sweep and the cut still run.

Exit 0 when every selected test passes, 1 when one fails, 2 when the tier
list or the diff cannot be read.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import shlex
import subprocess
import sys
import time

MAKEFILE = "cas-cli/Makefile"
TARGET = "test-ci-tiers:"
# Measured on the factory host (2026-10-10, idle): each takes over 20 s alone,
# more than lane admission can spend on one row. Every other entry measured
# under 15 s (most under 2 s).
HEAVY = {
    "scripts/test-codemap-latency-receipt.sh": 126,
    "scripts/test-fast-release-rows.py": 70,
    "scripts/test-worker-memory.py": 21,
}


def tier_commands(repo: Path) -> list[str]:
    """The test-ci-tiers recipe; empty for a checkout without the tier."""
    if not (repo / MAKEFILE).is_file():
        return []
    commands, inside = [], False
    for line in (repo / MAKEFILE).read_text().splitlines():
        if line.startswith(TARGET):
            inside = True
            continue
        if inside:
            if not line.startswith("\t"):
                break
            commands.append(line.strip().removeprefix("cd .. && "))
    return commands


def test_script(command: str) -> str | None:
    for token in shlex.split(command):
        token = token.removeprefix("./")
        if re.fullmatch(r"scripts/test-[\w.-]+", token):
            return token
    return None


def changed_paths(repo: Path, base: str) -> list[str]:
    out = subprocess.run(["git", "-C", str(repo), "diff", "--name-only", base, "--", "scripts"],
                         capture_output=True, text=True)
    if out.returncode:
        raise ValueError(f"cannot diff scripts against {base}: {out.stderr.strip()}")
    return [line for line in out.stdout.splitlines() if line]


def stem(path: str) -> str:
    name = Path(path).name
    return name.split(".", 1)[0]


def select(commands: list[str], changed: list[str]) -> list[tuple[str, str]]:
    changed_stems = {stem(path) for path in changed}
    selected = []
    for command in commands:
        test = test_script(command)
        if not test:
            continue
        subject = stem(test).removeprefix("test-")
        if test in changed or subject in changed_stems or stem(test) in changed_stems:
            selected.append((test, command))
    return selected


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--base", required=True)
    parser.add_argument("--repo", default=".")
    parser.add_argument("--dry-run", action="store_true", help="print the selection only")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    try:
        selected = select(tier_commands(repo), changed_paths(repo, args.base))
    except (OSError, ValueError) as error:
        print(f"ci-script-tests-changed: {error}", file=sys.stderr)
        return 2
    if not selected:
        print("ci-script-tests-changed: no script test covers this lane's changes")
        return 0
    failed = []
    for test, command in selected:
        if test in HEAVY:
            print(f"ci-script-tests-changed: DEFERRED {test} (~{HEAVY[test]}s) to the full "
                  f"ci-script-tests row")
            continue
        if args.dry_run:
            print(f"ci-script-tests-changed: would run {command}")
            continue
        started = time.monotonic()
        result = subprocess.run(command, shell=True, cwd=repo)
        took = time.monotonic() - started
        status = "PASS" if result.returncode == 0 else f"FAIL (exit {result.returncode})"
        print(f"ci-script-tests-changed: {status} {command} ({took:.1f}s)", flush=True)
        if result.returncode:
            failed.append(test)
    if failed:
        print(f"ci-script-tests-changed: failed: {', '.join(failed)}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
