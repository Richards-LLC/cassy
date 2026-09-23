#!/usr/bin/env python3
"""Turn a journey-suite run into cas-qa-craft evidence bundles.

Usage: journey-bundles.py <artifact-dir> <dist-tree> <commit> <suite-exit> <hub-web-dir> <playwright-version>

Called by scripts/journey-eval.sh after the `journeys` project ran with
JOURNEY_RECEIPTS=<artifact-dir>/journeys. For every <ID>/result.json it:
  - copies the test's trace.zip into the bundle
  - writes trace-actions.txt (`npx playwright trace actions`)
  - writes bundle.json (producer "journey"; contract: cas-qa-craft evidence bundle v1)
Then writes <artifact-dir>/journeys/JOURNEYS.md, the run summary the evaluator reads.
"""

from __future__ import annotations

import datetime
import json
import shutil
import subprocess
import sys
from pathlib import Path


def order(result: Path) -> tuple[str, int]:
    prefix, _, number = result.parent.name.rpartition("J")
    return prefix, int(number) if number.isdigit() else 0


def trace_actions(bundle: Path, hub: str) -> str:
    def npx(*args: str) -> subprocess.CompletedProcess:
        return subprocess.run(["npx", "--prefix", hub, "playwright", "trace", *args],
                              cwd=bundle, capture_output=True, text=True)
    npx("open", "trace.zip")
    actions = npx("actions")
    npx("close")
    return actions.stdout + actions.stderr


def main(argv: list[str]) -> int:
    artifacts, tree, commit, status, hub, pw_version = Path(argv[0]), argv[1], argv[2], int(argv[3]), argv[4], argv[5]
    journeys = artifacts / "journeys"
    created = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    rows = []
    for result in sorted(journeys.glob("*/result.json"), key=order):
        bundle = result.parent
        data = json.loads(result.read_text())
        stages = data.get("stages", [])
        files: dict[str, object] = {
            "receipt": "receipt.webm",
            "aria_yaml": "final.aria.yml",
            "aria_json": "final.aria.json",
            "cells": [stage["screenshot"] for stage in stages],
        }
        trace = Path(data.get("output_dir", "")) / "trace.zip"
        if trace.is_file():
            shutil.copyfile(trace, bundle / "trace.zip")
            (bundle / "trace-actions.txt").write_text(trace_actions(bundle, hub))
            files["trace"] = "trace.zip"
            files["trace_actions"] = "trace-actions.txt"
        (bundle / "bundle.json").write_text(json.dumps({
            "schema": 1,
            "task_id": artifacts.name,
            "producer": "journey",
            "journey_id": data["id"],
            "verdict": data["status"],
            "label": data.get("label"),
            "head_sha": commit,
            "hub_web_dist": tree,
            "build_url": "hub-web/dist at /commander/ with the hub protocol double",
            "playwright_version": pw_version,
            "created_at": created,
            "visual_change": False,
            "visual_qa_status": "unavailable",
            "files": files,
        }, indent=2) + "\n")
        total = sum(stage.get("ms", 0) for stage in stages)
        slow = max(stages, key=lambda stage: stage.get("ms", 0)) if stages else {"title": "-", "ms": 0}
        rows.append((data["id"], data["title"], data["status"], total, slow))

    lines = [
        f"# Journey suite run — hub-web/dist {tree[:8]}",
        "",
        f"- hub_web_dist: {tree}",
        f"- evaluated_commit: {commit}",
        f"- suite_exit: {status}",
        f"- journeys: {len(rows)}, PASS {sum(1 for row in rows if row[2] == 'PASS')}",
        "- label: real-bundle, protocol-double",
        "",
        "| ID | Journey | Run | Total | Slowest stage |",
        "|---|---|---|---|---|",
    ]
    for ident, title, run, total, slow in rows:
        lines.append(f"| {ident} | {title} | {run} | {total / 1000:.1f}s | {slow['title']} ({slow['ms'] / 1000:.1f}s) |")
    (journeys / "JOURNEYS.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
