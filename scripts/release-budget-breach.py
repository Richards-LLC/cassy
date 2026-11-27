#!/usr/bin/env python3
"""Report release budget misses without creating tasks.

Usage: release-budget-breach.py <tag> <run-dir> [--cas-bin <legacy-unused-path>]
"""
from pathlib import Path
import sys


def main(argv: list[str]) -> int:
    if len(argv) == 4 and argv[2] == "--cas-bin":
        argv = argv[:2]
    if len(argv) != 2:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    tag, run_dir = argv[0], Path(argv[1])
    values = dict(line.split("=", 1) for line in
                  (run_dir / "release-latency.receipt").read_text().splitlines() if "=" in line)
    breaches = values.get("BUDGET_BREACHES", "unavailable")
    print(f"budget: {tag} {breaches}; measured misses belong in the nightly scorecard")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
