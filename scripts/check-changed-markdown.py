#!/usr/bin/env python3
"""Lint changed Markdown with the repository policy, without downloading tools."""

from pathlib import Path
import shutil
import subprocess
import sys


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else "HEAD^"
    if not subprocess.run(["git", "rev-parse", "--verify", base], capture_output=True).returncode == 0:
        # HEAD^ is absent on an initial commit; an explicit invalid base fails.
        if base != "HEAD^" or len(subprocess.run(
            ["git", "rev-list", "--parents", "-n", "1", "HEAD"],
            capture_output=True, text=True,
        ).stdout.split()) != 1:
            raise ValueError(f"cannot resolve Markdown comparison base: {base}")
        base = subprocess.check_output(["git", "hash-object", "-t", "tree", "--stdin"], input=b"").decode().strip()
    paths = subprocess.check_output(
        ["git", "diff", "--name-only", "--diff-filter=ACMR", "-z", base, "HEAD"],
    ).decode().split("\0")
    paths = [path for path in paths if path.endswith(".md") and Path(path).is_file()]
    if not paths:
        print("No changed Markdown files")
        return 0
    if shutil.which("markdownlint-cli2"):
        command = ["markdownlint-cli2"]
    elif shutil.which("npx"):
        command = ["npx", "--no-install", "markdownlint-cli2@0.18.1"]
    else:
        raise ValueError("no Markdown linter on PATH; FIX: npm install --global markdownlint-cli2@0.18.1 --no-audit --no-fund (the version CI pins), then rerun")
    sys.stdout.flush()
    status = subprocess.run(command + ["--config", ".markdownlint-cli2.jsonc", *paths]).returncode
    if status:
        # The exact repair, so a park refusal never needs a CI round to explain.
        print("FIX: run `npx --yes markdownlint-cli2@0.18.1 --fix --config .markdownlint-cli2.jsonc "
              + " ".join(paths) + "`, then hand-fix what remains (MD034: wrap bare URLs in <>; "
              "MD036: make bold-only lines headings) and commit", flush=True)
    return status


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"markdown-lint: {error}", file=sys.stderr)
        sys.exit(1)
