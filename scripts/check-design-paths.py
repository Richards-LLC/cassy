#!/usr/bin/env python3
"""Reject machine-specific home paths in committed design documents and fixtures."""

from pathlib import Path
import re
import sys

HOME = re.compile(r"(?<![\w.-])/(?:home|Users)/|[A-Za-z]:[\\/](?:Users|Documents and Settings)[\\/]")


def check(root):
    if not root.is_dir():
        print(f"design-doc-paths: missing directory: {root}", file=sys.stderr)
        return 1
    failures = 0
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for number, line in enumerate(text.splitlines(), 1):
            if HOME.search(line):
                print(f"{path}:{number}: absolute home path; use <artifacts>/<task-id>/<file> or <home>")
                failures += 1
    print(f"design-doc-paths: {failures} absolute home path(s)")
    return int(bool(failures))


if __name__ == "__main__":
    try:
        sys.exit(check(Path("docs/design")))
    except OSError as error:
        print(f"design-doc-paths: {error}", file=sys.stderr)
        sys.exit(1)
