#!/usr/bin/env python3
"""Reject retired hub vocabulary outside published history and exact manifest lines."""
from __future__ import annotations
import fnmatch
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
ALLOWLIST = ROOT / "scripts/violet-reference-allowlist.json"
MANIFEST = ROOT / "crates/cas-types/src/violet-compatibility.json"


def violations(root: Path, paths: list[str], allowlist: dict, pattern: re.Pattern) -> list[str]:
    failures = []
    for relative in paths:
        if any(fnmatch.fnmatchcase(relative, glob) for glob in allowlist["published_history"]):
            continue
        path = root / relative
        if not path.is_file():
            continue
        text = path.read_bytes().decode("utf-8", errors="replace")
        approved = allowlist["exact_lines"].get(relative, {})
        for number, line in enumerate(text.splitlines(), 1):
            if pattern.search(line) and approved.get(str(number)) != hashlib.sha256(line.encode()).hexdigest():
                failures.append(f"{relative}:{number}: {line}")
    return failures


def main() -> int:
    allowlist = json.loads(ALLOWLIST.read_text())
    manifest = json.loads(MANIFEST.read_text())
    pattern = re.compile(manifest["retired_reference_pattern"], re.IGNORECASE)
    paths = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT).decode().split("\0")
    failures = violations(ROOT, paths, allowlist, pattern)
    if failures:
        print("Violet reference retirement FAILED:", file=sys.stderr)
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(f"Violet reference retirement: PASS ({len([p for p in paths if p])} files checked)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
