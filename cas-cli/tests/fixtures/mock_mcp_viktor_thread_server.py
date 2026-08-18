#!/usr/bin/env python3
"""Deterministic Viktor thread/run MCP fixture for inbound-watch tests."""

import json
import sys


TOOLS = [
    {"name": "create_thread", "description": "start", "inputSchema": {"type": "object"}},
    {"name": "get_run", "description": "status", "inputSchema": {"type": "object"}},
    {"name": "get_run_result", "description": "result", "inputSchema": {"type": "object"}},
]


def respond(request_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n")
    sys.stdout.flush()


def tool_result(payload):
    return {"content": [{"type": "text", "text": json.dumps(payload)}]}


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
                "protocolVersion": request.get("params", {}).get("protocolVersion", "2024-11-05"),
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": "mock-viktor-thread", "version": "0.0.1"},
            },
        )
    elif method == "tools/list":
        respond(request_id, {"tools": TOOLS})
    elif method == "tools/call":
        name = request.get("params", {}).get("name")
        if name == "create_thread":
            payload = {
                "thread": {"id": "thread-fixture-1"},
                "message": {"id": "message-fixture-1"},
                "run": {"id": "run-fixture-1"},
            }
        elif name == "get_run":
            payload = {"run_id": "run-fixture-1", "status": "completed"}
        elif name == "get_run_result":
            payload = {"markdown": "fixture Viktor reply", "json": {"answer": 42}}
        else:
            payload = {"error": "unsupported", "message": name}
        respond(request_id, tool_result(payload))
    elif method in ("ping", "shutdown", "exit"):
        respond(request_id, {})
    else:
        respond(request_id, tool_result({"error": "unsupported", "message": method}))
