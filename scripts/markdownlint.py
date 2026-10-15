#!/usr/bin/env python3
"""Run the repository's pinned markdownlint-cli2. The one way every check finds it.

The linter is a devDependency in the repository root package.json, locked by
package-lock.json. This wrapper runs `<root>/node_modules/.bin/markdownlint-cli2`.
When the binary is missing (a fresh checkout or CI runner) it runs `npm ci` at the
root once, under a per-checkout lock, from the committed lockfile. It never uses
PATH, a global install or an npx cache, so a scrubbed HOME or a bare runner
resolves the same version as a developer checkout (cas-a677: five strategies,
two missing-linter failures on 2026-10-14).

MARKDOWNLINT_CLI2 names a different executable. Tests set it to a stub; nothing
else should.

Usage: scripts/markdownlint.py [markdownlint-cli2 arguments...]
       scripts/markdownlint.py --resolve   # print the executable path only
Exit: the linter's status; 3 when the linter cannot be installed.
"""
from __future__ import annotations

import fcntl
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "node_modules" / ".bin" / "markdownlint-cli2"
UNAVAILABLE = 3


def lock_path() -> Path:
    git_path = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "--git-path", "cas-markdownlint-install.lock"],
        capture_output=True, text=True,
    )
    if git_path.returncode == 0 and git_path.stdout.strip():
        path = Path(git_path.stdout.strip())
        return path if path.is_absolute() else ROOT / path
    # Not a Git checkout (an extracted archive): lock beside the manifest.
    return ROOT / ".markdownlint-install.lock"


def resolve() -> Path:
    override = os.environ.get("MARKDOWNLINT_CLI2")
    if override:
        return Path(override)
    if BIN.exists():
        return BIN
    npm = shutil.which("npm")
    if npm is None:
        raise RuntimeError(
            "npm is not on PATH, so the pinned markdownlint-cli2 cannot be installed; "
            "FIX: install Node.js, then run `npm ci` at the repository root"
        )
    lock = lock_path()
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open("w") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        if not BIN.exists():
            print("markdownlint: installing the pinned linter (npm ci at the repository root)", file=sys.stderr)
            result = subprocess.run(
                [npm, "ci", "--no-audit", "--no-fund", "--silent"],
                cwd=ROOT, stdout=sys.stderr,
            )
            if result.returncode != 0 or not BIN.exists():
                raise RuntimeError("`npm ci` at the repository root did not install markdownlint-cli2")
    return BIN


def main(argv: list[str]) -> int:
    try:
        linter = resolve()
    except (RuntimeError, OSError) as error:
        print(f"markdownlint: {error}", file=sys.stderr)
        return UNAVAILABLE
    if argv == ["--resolve"]:
        print(linter)
        return 0
    sys.stdout.flush()
    return subprocess.run([str(linter), *argv]).returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
