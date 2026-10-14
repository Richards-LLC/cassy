#!/usr/bin/env python3
"""End-to-end release metrics from a release-train run directory (cas-a629).

Tag-to-published latency (about 3 minutes) hid hours of blocker recovery
before the tag. This reports the whole wall clock and what cost it:

- REQUEST_TO_PUBLISHED_SECS: release.request.epoch (CAS_RELEASE_TRAIN_REQUESTED_AT
  at the first cut) to publication. REQUEST_SOURCE says where the start came
  from: `recorded`, or `cut-start` when no request time was given.
- CUT_TO_PUBLISHED_SECS: cut.start.epoch (the first `--cut`) to publication.
- BLOCKER_COUNT, BLOCKER_COSTS and BLOCKED_SECS: each blocked stage from
  stage-events.tsv, priced from the block to that stage's next completion (to
  publication if it never completed: `<stage>:<secs>+`).
- STAGE_SECS: each completed stage's last attempt, start to done.

Runs recorded before stage events existed report BLOCKER_COUNT from
blockers.log and their costs as unavailable.

Usage:
  release-metrics.py <run-dir> <published-epoch>
  release-metrics.py --normalize-request <epoch|ISO-8601>
"""

from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path
import sys
import time

UNAVAILABLE = "unavailable"


def epoch_file(path: Path) -> int | None:
    try:
        text = path.read_text().strip()
    except OSError:
        return None
    return int(text) if text.isdigit() else None


def normalize_request(value: str, now: float | None = None) -> int:
    value = value.strip()
    if value.isdigit():
        epoch = int(value)
    else:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        if parsed.tzinfo is None:
            raise ValueError("ISO request time needs a UTC offset")
        epoch = int(parsed.astimezone(timezone.utc).timestamp())
    if epoch > (now if now is not None else time.time()):
        raise ValueError("request time is in the future")
    return epoch


def events(run_dir: Path) -> list[tuple[int, str, str]]:
    rows = []
    try:
        lines = (run_dir / "stage-events.tsv").read_text().splitlines()
    except OSError:
        return rows
    for line in lines:
        fields = line.split("\t")
        if len(fields) == 3 and fields[0].isdigit() and fields[2] in ("start", "done", "blocked"):
            rows.append((int(fields[0]), fields[1], fields[2]))
    rows.sort(key=lambda row: row[0])
    return rows


def blockers(rows: list[tuple[int, str, str]], published: int) -> list[tuple[str, int, bool]]:
    """(stage, seconds, resolved) per block. Repeated blocked rows for a stage
    before its next start or completion are one blocker."""
    open_blocks: dict[str, int] = {}
    priced: list[tuple[str, int, bool]] = []
    order: list[str] = []
    for epoch, stage, event in rows:
        if event == "blocked":
            if stage not in open_blocks:
                open_blocks[stage] = epoch
                order.append(stage)
        elif event == "start" and stage in open_blocks:
            continue  # a retry; the block lasts until the stage completes
        elif event == "done" and stage in open_blocks:
            started = open_blocks.pop(stage)
            priced.append((stage, epoch - started, True))
            order.remove(stage)
    for stage in order:
        priced.append((stage, max(0, published - open_blocks[stage]), False))
    return priced


def stage_seconds(rows: list[tuple[int, str, str]]) -> list[tuple[str, int]]:
    started: dict[str, int] = {}
    durations: dict[str, int] = {}
    for epoch, stage, event in rows:
        if event == "start":
            started[stage] = epoch
        elif event == "done" and stage in started:
            durations[stage] = epoch - started.pop(stage)
    return list(durations.items())


def metrics(run_dir: Path | None, published: int) -> list[tuple[str, str]]:
    if run_dir is None or not run_dir.is_dir():
        return [(key, UNAVAILABLE) for key in (
            "REQUESTED_AT_EPOCH", "REQUEST_SOURCE", "CUT_STARTED_AT_EPOCH", "REQUEST_TO_PUBLISHED_SECS",
            "CUT_TO_PUBLISHED_SECS", "BLOCKER_COUNT", "BLOCKER_COSTS", "BLOCKED_SECS", "STAGE_SECS")]
    cut = epoch_file(run_dir / "cut.start.epoch")
    request = epoch_file(run_dir / "release.request.epoch")
    source = "recorded" if request is not None else ("cut-start" if cut is not None else UNAVAILABLE)
    request = request if request is not None else cut

    def since(start: int | None) -> str:
        return str(published - start) if start is not None and published >= start else UNAVAILABLE

    rows = events(run_dir)
    if rows:
        priced = blockers(rows, published)
        count = str(len(priced))
        costs = ",".join(f"{stage}:{secs}{'' if resolved else '+'}" for stage, secs, resolved in priced) or "none"
        blocked = str(sum(secs for _, secs, _ in priced))
        stages = ",".join(f"{stage}:{secs}" for stage, secs in stage_seconds(rows)) or "none"
    else:
        try:
            legacy = [line.strip() for line in (run_dir / "blockers.log").read_text().splitlines() if line.strip()]
        except OSError:
            legacy = []
        count = str(len(dict.fromkeys(legacy)))
        costs = blocked = stages = UNAVAILABLE
    return [
        ("REQUESTED_AT_EPOCH", str(request) if request is not None else UNAVAILABLE),
        ("REQUEST_SOURCE", source),
        ("CUT_STARTED_AT_EPOCH", str(cut) if cut is not None else UNAVAILABLE),
        ("REQUEST_TO_PUBLISHED_SECS", since(request)),
        ("CUT_TO_PUBLISHED_SECS", since(cut)),
        ("BLOCKER_COUNT", count),
        ("BLOCKER_COSTS", costs),
        ("BLOCKED_SECS", blocked),
        ("STAGE_SECS", stages),
    ]


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[0] == "--normalize-request":
        try:
            print(normalize_request(argv[1]))
        except ValueError as error:
            print(f"release-metrics: {error}", file=sys.stderr)
            return 2
        return 0
    if len(argv) != 2 or not argv[1].isdigit():
        print("usage: release-metrics.py <run-dir> <published-epoch> | --normalize-request <time>",
              file=sys.stderr)
        return 2
    run_dir = Path(argv[0]) if argv[0] else None
    for key, value in metrics(run_dir, int(argv[1])):
        print(f"{key}={value}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
