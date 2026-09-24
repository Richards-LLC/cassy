#!/usr/bin/env python3
"""Minimal stdio MCP fixture standing in for the Neon server (GH #988).

`run_sql` answers with a JSON-RPC error carrying a Postgres schema error, the
shape a real Neon call returns for a bad column. `list_projects` succeeds.
"""

import json
import sys

SCHEMA_ERROR = 'column "nme" of relation "users" does not exist'


def respond(request_id, result):
    sys.stdout.write(
        json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n"
    )
    sys.stdout.flush()


def fail(request_id, code, message, data=None):
    error = {"code": code, "message": message}
    if data is not None:
        error["data"] = data
    sys.stdout.write(
        json.dumps({"jsonrpc": "2.0", "id": request_id, "error": error}) + "\n"
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
                    "serverInfo": {"name": "mock-neon", "version": "0.0.1"},
                },
            )
        elif method == "tools/list":
            respond(
                request_id,
                {
                    "tools": [
                        {
                            "name": "run_sql",
                            "description": "Fixture SQL runner",
                            "inputSchema": {"type": "object"},
                        },
                        {
                            "name": "list_projects",
                            "description": "Fixture project list",
                            "inputSchema": {"type": "object"},
                        },
                    ]
                },
            )
        elif method == "tools/call":
            name = request.get("params", {}).get("name")
            if name == "run_sql":
                fail(request_id, -32602, SCHEMA_ERROR, {"sqlstate": "42703"})
            else:
                respond(
                    request_id,
                    {"content": [{"type": "text", "text": '["fixture-project"]'}]},
                )
        elif method in ("ping", "shutdown", "exit"):
            respond(request_id, {})
        else:
            fail(request_id, -32601, "method not found")


if __name__ == "__main__":
    main()
