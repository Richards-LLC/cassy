#!/usr/bin/env python3
"""Count release rescues from evidence, independently of invocation identity."""
from pathlib import Path
import re
import sys

STAGES = set("preflight assemble prep ledger gate pr-body pipeline publish "
             "post-publication announce report receipts host-update".split())


def read(path):
    return path.read_text(encoding="utf-8").splitlines() if path.is_file() else []


def metrics(run_dir):
    stages, resumed = {}, set()
    manual = 0
    for row in read(run_dir / "interventions.log"):
        fields = dict(token.split("=", 1) for token in row.split() if "=" in token)
        blockers = [name for name in fields.get("blockers", "").split(",") if name in STAGES]
        if fields.get("resume") == "true" and blockers:
            resumed.update(blockers)
            stages.update(dict.fromkeys(blockers))
        elif fields.get("kind") == "manual":
            manual += 1
        if fields.get("kind") == "manual":
            stage = fields.get("stage")
            if stage in STAGES:
                stages.setdefault(stage, None)
            stages.update(dict.fromkeys(blockers))

    # Old or interrupted runs may have blocker evidence but no resume log.
    # They must never be reported as needing zero interventions.
    blocked = set()
    blocker_rows = read(run_dir / "blockers.log")
    for row in blocker_rows:
        for name in row.split()[0].split(",") if row.split() else []:
            if name in STAGES:
                blocked.add(name)
                stages.setdefault(name, None)
    for pattern in ("stage.*.blocked", "blocker.*"):
        for path in sorted(run_dir.glob(pattern)):
            name = (path.name[6:-8] if pattern.startswith("stage.") else path.name[8:])
            if name in STAGES:
                blocked.add(name)
                stages.setdefault(name, None)
    notes = sum(bool(re.match(r"^\s*(?:[-*+] |\d+[.)] )\S", row))
                for row in read(run_dir / "supervisor-interventions.md"))
    count = manual + len(resumed) + notes
    if not count and blocked:
        count = len(blocked)
    if (run_dir / "blockers.log").is_file() and (run_dir / "blockers.log").stat().st_size:
        count = max(1, count)
    return count, ",".join(stages) or "none"


if __name__ == "__main__":
    mode, directory = sys.argv[1:]
    # An omitted run directory means unavailable evidence, not filesystem root.
    count, stages = metrics(Path(directory)) if directory else (0, "none")
    print(count if mode == "count" else stages)
