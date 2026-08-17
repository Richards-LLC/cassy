#!/usr/bin/env python3
"""Minimal stdio MCP fixture for the receipted Viktor gateway integration test."""

import json
import sys


def respond(request_id, result):
    sys.stdout.write(
        json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n"
    )
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        request = json.loads(line)
        request_id = request.get("id")
        if request_id is None:
            continue
        method = request.get("method")
        if method == "initialize":
            respond(
                request_id,
                {
                    "protocolVersion": request.get("params", {}).get(
                        "protocolVersion", "2024-11-05"
                    ),
                    "capabilities": {"tools": {"listChanged": False}},
                    "serverInfo": {"name": "mock-viktor", "version": "0.0.1"},
                },
            )
        elif method == "tools/list":
            respond(
                request_id,
                {
                    "tools": [
                        {
                            "name": "ask_viktor",
                            "description": "Fixture run starter",
                            "inputSchema": {"type": "object"},
                        },
                        {
                            "name": "wait_for_run",
                            "description": "Fixture run waiter",
                            "inputSchema": {"type": "object"},
                        },
                    ]
                },
            )
        elif method == "tools/call":
            check = {
                "name": "health",
                "expected": "ready",
                "observed": "ready",
                "evidence": "fixture://health",
            }
            payload = {
                "run_id": "run-fixture-1",
                "json": {"verdict": "pass", "checks": [check], "limitations": []},
            }
            respond(
                request_id,
                {"content": [{"type": "text", "text": json.dumps(payload)}]},
            )
        elif method in ("ping", "shutdown", "exit"):
            respond(request_id, {})
        else:
            respond(
                request_id,
                {"content": [{"type": "text", "text": '{"error":"unsupported"}'}]},
            )


if __name__ == "__main__":
    main()
