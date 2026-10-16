#!/usr/bin/env python3
"""Strict stdio Neon MCP fixture for GH #1239 regression coverage.

The server rejects calls whose ``arguments`` property is omitted or is not a
JSON object. This matches Neon MCP's required tool-call shape and catches the
``None`` argument passed by the old ``list_organizations`` adapter.
"""

import json
import os
import sys


def respond(request_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n")
    sys.stdout.flush()


def fail(request_id, code, message):
    sys.stdout.write(
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": code, "message": message},
            }
        )
        + "\n"
    )
    sys.stdout.flush()


def text_result(payload):
    return {"content": [{"type": "text", "text": json.dumps(payload)}]}


def record_call(tool, arguments):
    path = os.environ.get("NEON_CALL_LOG")
    if path:
        with open(path, "a", encoding="utf-8") as log:
            log.write(json.dumps({"tool": tool, "arguments": arguments}) + "\n")


def handle_call(request_id, params):
    tool = params.get("name")
    arguments = params.get("arguments")
    record_call(tool, arguments)
    if not isinstance(arguments, dict):
        fail(request_id, -32602, "arguments must be an object")
        return

    required = {
        "list_organizations": [],
        "list_projects": ["org_id"],
        "describe_project": ["org_id", "project_id"],
        "describe_branch": ["org_id", "project_id", "branch_id"],
        "list_branches": ["project_id"],
        "create_branch": ["project_id", "name", "parent_id"],
        "get_connection_string": ["project_id", "branch_id"],
        "delete_branch": ["project_id", "branch_id"],
    }
    if tool not in required:
        fail(request_id, -32601, "tool not found")
        return
    missing = [key for key in required[tool] if key not in arguments]
    if missing:
        fail(request_id, -32602, "missing required arguments: " + ", ".join(missing))
        return

    branches = [
        {"id": "br-main-fixture", "name": "main", "default": True},
        {"id": "br-staging-fixture", "name": "staging", "default": False},
        {"id": "br-dev-fixture", "name": "dev", "default": False},
    ]
    if tool == "list_organizations":
        payload = {"organizations": [{"id": "org-fixture", "name": "Fixture org"}]}
    elif tool == "list_projects":
        payload = {"projects": [{"id": "proj-fixture", "name": "fixture-repo"}]}
    elif tool == "describe_project":
        payload = {
            "project": {
                "id": "proj-fixture",
                "name": "fixture-repo",
                "default_database_name": "neondb",
            },
            "branches": branches,
        }
    elif tool == "describe_branch":
        branch_id = arguments["branch_id"]
        branch = next((branch for branch in branches if branch["id"] == branch_id), None)
        payload = {"branch": branch}
    elif tool == "list_branches":
        payload = {"branches": branches}
    elif tool == "create_branch":
        payload = {
            "branch": {
                "id": "br-task-fixture",
                "name": arguments["name"],
                "default": False,
            }
        }
    elif tool == "get_connection_string":
        payload = {"connection_string": "postgresql://fixture:secret@example.invalid/neondb"}
    else:
        payload = {}
    respond(request_id, text_result(payload))


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
                    "serverInfo": {"name": "strict-mock-neon", "version": "0.0.1"},
                },
            )
        elif method == "tools/list":
            respond(
                request_id,
                {
                    "tools": [
                        {
                            "name": name,
                            "description": "Strict fixture Neon tool",
                            "inputSchema": {"type": "object"},
                        }
                        for name in [
                            "list_organizations",
                            "list_projects",
                            "describe_project",
                            "describe_branch",
                            "list_branches",
                            "create_branch",
                            "get_connection_string",
                            "delete_branch",
                        ]
                    ]
                },
            )
        elif method == "tools/call":
            handle_call(request_id, request.get("params", {}))
        elif method in ("ping", "shutdown", "exit"):
            respond(request_id, {})
        else:
            fail(request_id, -32601, "method not found")


if __name__ == "__main__":
    main()
