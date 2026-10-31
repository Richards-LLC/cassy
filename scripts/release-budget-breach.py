#!/usr/bin/env python3
"""File one P0 task when a release stage overran its budget (cas-2bef).

Reads BUDGET_BREACHES from <run-dir>/release-latency.receipt, which
release-metrics.py computes from stage-events.tsv and
release-stage-budgets.json. On a breach it creates a P0 task in the budget
file's epic, through `cas serve` (there is no task CLI), and records the task
in <run-dir>/budget-breach.task so a resumed stage never files twice.

Usage: release-budget-breach.py <tag> <run-dir> [--cas-bin <path>]
"""

from __future__ import annotations

import json
from pathlib import Path
import re
import select
import subprocess
import sys
import time

BUDGETS = Path(__file__).with_name("release-stage-budgets.json")
TIMEOUT_SECS = 60


def receipt_values(path: Path) -> dict[str, str]:
    values = {}
    for line in path.read_text().splitlines():
        key, sep, value = line.partition("=")
        if sep:
            values[key] = value
    return values


def minutes(secs: int) -> str:
    return f"{secs // 60}m {secs % 60:02d}s"


def call_task_create(cas_bin: str, cwd: Path, arguments: dict) -> str:
    # Unbuffered pipes: select() watches the fd, so a buffered readline could
    # hold a reply that arrived in the same write as an earlier line.
    proc = subprocess.Popen([cas_bin, "serve"], cwd=cwd, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
    deadline = time.monotonic() + TIMEOUT_SECS

    def send(message: dict) -> None:
        proc.stdin.write((json.dumps(message) + "\n").encode())

    def response(request_id: int) -> dict:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([proc.stdout], [], [], remaining)[0]:
                raise TimeoutError("cas serve did not answer within the deadline")
            line = proc.stdout.readline()
            if not line:
                raise RuntimeError(f"cas serve exited ({proc.poll()})")
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if "method" in message and "id" in message:
                # Server-to-client requests (roots/list, ping) need an answer.
                send({"jsonrpc": "2.0", "id": message["id"],
                      "result": {"roots": []} if message["method"] == "roots/list" else {}})
            elif message.get("id") == request_id:
                return message

    try:
        send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "release-budget-breach", "version": "1"}}})
        response(1)
        send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
        send({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
              "params": {"name": "task", "arguments": arguments}})
        reply = response(2)
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
    result = reply.get("result") or {}
    text = " ".join(item.get("text", "") for item in result.get("content", []) if isinstance(item, dict))
    if "error" in reply or result.get("isError"):
        raise RuntimeError(f"task create failed: {reply.get('error') or text}")
    match = re.search(r"Created task: ([A-Za-z0-9-]+)", text)
    if not match:
        raise RuntimeError(f"task create returned no task id: {text[:300]}")
    return match.group(1)


def main(argv: list[str]) -> int:
    cas_bin = "cas"
    if len(argv) == 4 and argv[2] == "--cas-bin":
        cas_bin = argv[3]
        argv = argv[:2]
    if len(argv) != 2:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    tag, run_dir = argv[0], Path(argv[1])
    receipt = run_dir / "release-latency.receipt"
    marker = run_dir / "budget-breach.task"
    breaches = receipt_values(receipt).get("BUDGET_BREACHES", "unavailable")
    if breaches in ("none", "unavailable", ""):
        print(f"budget: {tag} stages within budget ({breaches})")
        return 0
    if marker.is_file():
        print(f"budget: {tag} breach already filed as {marker.read_text().strip()}")
        return 0
    rows = []
    for item in breaches.split(","):
        stage, _, measured = item.partition(":")
        secs, _, budget = measured.partition("/")
        rows.append((stage, int(secs), int(budget)))
    epic = json.loads(BUDGETS.read_text())["epic"]
    title = f"Release {tag} over stage budget: " + ", ".join(
        f"{stage} {minutes(secs)} > {minutes(budget)}" for stage, secs, budget in rows)
    description = "\n".join([
        f"The {tag} release train overran its versioned stage budgets "
        f"(scripts/release-stage-budgets.json):",
        *(f"- {stage}: measured {secs} s, budget {budget} s ({secs - budget} s over)"
          for stage, secs, budget in rows),
        "",
        f"Receipts: {receipt} and {run_dir / 'stage-events.tsv'}.",
        "Find the cause and bring the stage back under budget; loosening a budget is a reviewed change.",
    ])
    task_id = call_task_create(cas_bin, Path.cwd(), {
        "action": "create", "title": title, "description": description, "priority": 0,
        "task_type": "bug", "epic": epic, "risk": "none", "labels": "pipeline,release-latency"})
    marker.write_text(task_id + "\n")
    print(f"budget: {tag} over budget ({breaches}); filed {task_id} in {epic}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
