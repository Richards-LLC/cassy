#!/usr/bin/env python3
"""Reject GitHub expressions inside workflow shell source.

Action expressions in env values are safe to expand as data; expressions in a
run block can turn commit messages or step outputs into shell program text.
"""

import re
import sys
from pathlib import Path


def run_expressions(path: Path):
    lines = path.read_text().splitlines()
    run_indent = None
    for number, line in enumerate(lines, 1):
        stripped = line.lstrip()
        indent = len(line) - len(stripped)
        if run_indent is not None and stripped and indent <= run_indent:
            run_indent = None
        match = re.match(r"(?:-\s+)?run:\s*(.*)$", stripped)
        if match and (run_indent is None):
            run_indent = indent if match.group(1) in ("|", "|-", ">", ">-") else None
            if "${{" in match.group(1):
                yield number
        elif run_indent is not None and "${{" in line:
            yield number


def main() -> int:
    paths = [Path(arg) for arg in sys.argv[1:]]
    if not paths:
        paths = sorted(Path(__file__).resolve().parents[1].glob(".github/workflows/*.yml"))
    failures = [(path, line) for path in paths for line in run_expressions(path)]
    for path, line in failures:
        print(f"{path}:{line}: GitHub expression inside run; pass it through env instead", file=sys.stderr)
    if not paths:
        print("No workflows found", file=sys.stderr)
        return 1
    print(f"Checked {len(paths)} workflow(s): {len(failures)} unsafe run expression(s)")
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
