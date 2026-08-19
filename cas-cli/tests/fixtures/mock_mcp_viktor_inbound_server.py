#!/usr/bin/env python3
"""Deterministic Viktor MCP fixture for provider-originated inbound tests."""

import json
import sys


TOOLS = [
    {"name": "list_threads", "description": "list", "inputSchema": {"type": "object"}},
    {"name": "list_messages", "description": "messages", "inputSchema": {"type": "object"}},
    {"name": "send_message", "description": "reply", "inputSchema": {"type": "object"}},
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
                "serverInfo": {"name": "mock-viktor-inbound", "version": "0.0.1"},
            },
        )
    elif method == "tools/list":
        respond(request_id, {"tools": TOOLS})
    elif method == "tools/call":
        name = request.get("params", {}).get("name")
        arguments = request.get("params", {}).get("arguments", {})
        if name == "list_threads":
            # Viktor's MCP route returns the public API array directly.
            payload = [
                {"id": "thread-viktor-originated", "status": "waiting"},
                {"id": "thread-cas-opened", "status": "completed"},
            ]
        elif name == "list_messages":
            thread_id = arguments.get("thread_id")
            payload = [
                {
                    "id": "message-viktor-question",
                    "thread_id": thread_id,
                    "role": "user",
                    "content": "Can Cassy answer this question on-thread?",
                }
            ]
        elif name == "send_message":
            payload = {
                "thread": {"id": arguments.get("thread_id")},
                "message": {"id": "message-cassy-reply"},
                "run": {"id": "run-cassy-reply"},
            }
        else:
            payload = {"error": "unsupported", "message": name}
        respond(request_id, tool_result(payload))
    elif method in ("ping", "shutdown", "exit"):
        respond(request_id, {})
    else:
        respond(request_id, tool_result({"error": "unsupported", "message": method}))
