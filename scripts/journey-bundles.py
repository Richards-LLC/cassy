#!/usr/bin/env python3
"""Turn a journey-suite run into cas-qa-craft evidence bundles.

Usage: journey-bundles.py <artifact-dir> <dist-tree> <commit> <suite-exit> <hub-web-dir> <playwright-version>

Called by scripts/journey-eval.sh after the `journeys` project ran with
JOURNEY_RECEIPTS=<artifact-dir>/journeys. For every <ID>/result.json it:
  - moves a failed or incomplete test's trace.zip into the bundle once
  - writes trace-actions.txt for a retained trace
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


def read_result(result: Path, ident: str, suite_failed: bool = False) -> dict:
    data = json.loads(result.read_text())
    if data["id"] != ident:
        raise ValueError(f"{ident}: mixed receipt id {data['id']} in {result}")
    screenshots = [stage["screenshot"] for stage in data.get("stages", [])]
    actual = {path.name for path in result.parent.glob("J*.png")}
    if len(screenshots) != len(set(screenshots)) or set(screenshots) != actual:
        expected = [f"J{index:02}.png" for index in range(1, len(screenshots) + 1)]
        extra = actual - set(screenshots)
        pending = f"J{len(screenshots) + 1:02}.png"
        # journey-eval archives prior runs and the receipt owner clears files
        # on retries. A lone next screenshot beside a failed current result is
        # therefore the capture interrupted before journey.ts appended a stage.
        if (suite_failed and data.get("status") == "FAIL" and screenshots == expected
                and set(screenshots) <= actual and extra == {pending}):
            data["incomplete_stages"] = [{
                "screenshot": pending,
                "status": "INCOMPLETE",
                "reason": "suite timed out after screenshot write and before stage receipt",
            }]
            data.setdefault("stages", []).append({
                "title": "Incomplete stage screenshot (timeout before stage receipt)",
                "slug": "incomplete-stage-capture",
                "ms": 0,
                "screenshot": pending,
                "status": "INCOMPLETE",
            })
        else:
            raise ValueError(f"{ident}: stale or mixed stage receipts in {result.parent}; "
                             f"declared={sorted(screenshots)}, actual={sorted(actual)}")
    return data


def should_keep_trace(data: dict) -> bool:
    incomplete = data.get("incomplete_stages", [])
    incomplete = incomplete or [
        stage for stage in data.get("stages", [])
        if stage.get("status") == "INCOMPLETE"
    ]
    return data.get("status") != "PASS" or bool(incomplete)


def bundle_trace(data: dict, bundle: Path, hub: str) -> dict[str, str]:
    """Keep a failed/incomplete trace once, at its journey-bundle path."""
    output_dir = data.get("output_dir")
    source = Path(output_dir) / "trace.zip" if output_dir else None
    trace = bundle / "trace.zip"
    actions = bundle / "trace-actions.txt"
    if not should_keep_trace(data):
        for path in (source, trace):
            if path is not None:
                path.unlink(missing_ok=True)
        actions.unlink(missing_ok=True)
        return {}

    if source is not None and source.is_file() and source.resolve() != trace.resolve():
        trace.unlink(missing_ok=True)
        shutil.move(str(source), str(trace))
    if not trace.is_file():
        actions.unlink(missing_ok=True)
        return {}

    actions.write_text(trace_actions(bundle, hub))
    return {"trace": "trace.zip", "trace_actions": "trace-actions.txt"}


def main(argv: list[str]) -> int:
    artifacts, tree, commit, status, hub, pw_version = Path(argv[0]), argv[1], argv[2], int(argv[3]), argv[4], argv[5]
    journeys = artifacts / "journeys"
    created = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    rows = []
    # A journey run as several tests (cas-1f7e) keeps its extra tests'
    # receipts under <ID>/parts/<slug>/. Each part gets its own trace; the
    # <ID> bundle folds every part's stages, cells and verdict into one.
    ids = sorted({path.parent.name for path in journeys.glob("*/result.json")}
                 | {path.parent.parent.parent.name for path in journeys.glob("*/parts/*/result.json")},
                 key=lambda name: order(journeys / name / "result.json"))
    for ident in ids:
        bundle = journeys / ident
        result = bundle / "result.json"
        parts = sorted(bundle.glob("parts/*/result.json"))
        data = read_result(result, ident, suite_failed=status != 0) if result.is_file() else None
        stages = [dict(stage) for stage in (data or {}).get("stages", [])]
        incomplete_stages = [dict(stage) for stage in (data or {}).get("incomplete_stages", [])]
        files: dict[str, object] = {"cells": [stage["screenshot"] for stage in stages]}
        if data is not None:
            files.update({"receipt": "receipt.webm", "aria_yaml": "final.aria.yml", "aria_json": "final.aria.json"})
            files.update(bundle_trace(data, bundle, hub))
        verdicts = [data["status"]] if data is not None else []
        folded = []
        for part_result in parts:
            part_dir = part_result.parent
            rel = part_dir.relative_to(bundle).as_posix()
            part = read_result(part_result, ident, suite_failed=status != 0)
            verdicts.append(part["status"])
            part_files = {"receipt": f"{rel}/receipt.webm", "aria_yaml": f"{rel}/final.aria.yml", "aria_json": f"{rel}/final.aria.json"}
            part_files.update({key: f"{rel}/{path}" for key, path in bundle_trace(part, part_dir, hub).items()})
            for stage in part.get("stages", []):
                stage = dict(stage, screenshot=f"{rel}/{stage['screenshot']}")
                stages.append(stage)
                files["cells"].append(stage["screenshot"])
            incomplete_stages.extend(
                dict(stage, screenshot=f"{rel}/{stage['screenshot']}")
                for stage in part.get("incomplete_stages", [])
            )
            folded.append({"title": part.get("title"), "project": part.get("project"), "title_path": part.get("title_path"), "verdict": part["status"], "label": part.get("label", "real-bundle, protocol-double"), **part_files})
        if folded:
            files["parts"] = folded
        if data is None:
            data = {"id": ident, "title": json.loads(parts[0].read_text()).get("title", ident)}
        labels = {item.strip() for row in [data, *folded] for item in row.get("label", "" if row is data and not result.is_file() else "real-bundle, protocol-double").split(",") if item.strip()}
        data["label"] = ", ".join(sorted(labels))
        data["status"] = "PASS" if verdicts and all(verdict == "PASS" for verdict in verdicts) else "FAIL"
        (bundle / "bundle.json").write_text(json.dumps({
            "schema": 1,
            "task_id": artifacts.name,
            "producer": "journey",
            "journey_id": data["id"],
            "journey_receipt": "../../journey-receipt.json",
            "verdict": data["status"],
            "label": data.get("label"),
            "head_sha": commit,
            "hub_web_dist": tree,
            "build_url": "hub-web/dist at /commander/; transport: " + data["label"],
            "playwright_version": pw_version,
            "created_at": created,
            "visual_change": False,
            "visual_qa_status": "unavailable",
            "files": files,
            "incomplete_stages": incomplete_stages,
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
        "- labels: see each bundle and part for its actual transport",
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
