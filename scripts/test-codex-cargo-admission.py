#!/usr/bin/env python3
"""Exercise installed Codex admission with scripted local responses and fake Cargo.

No model, credentials, live MCP server or Rust compiler is used. Uses installed
Codex or --codex; CI without Codex records an explicit native-boundary skip.
The mitigation budget is read from the shipped hooks config. The one-second
control intentionally documents the native fail-open behavior; no policy or
account projection is installed.
"""
import argparse
import http.server
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[1]
HOOK_CONFIG = ROOT / ".codex/hooks.json"
CODEX = None


class ScriptedResponses(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, command):
        super().__init__(("127.0.0.1", 0), ResponseHandler)
        self.command = command
        self.calls = 0
        self.requests = []


class ResponseHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.requests.append(request)
        self.server.calls += 1
        response_id = f"response-{self.server.calls}"
        events = [{"type": "response.created", "response": {"id": response_id}}]
        if self.server.calls == 1:
            item = {
                "type": "function_call",
                "call_id": "cargo-admission-call",
                "name": "exec_command",
                "arguments": json.dumps({"cmd": self.server.command, "login": False, "yield_time_ms": 1000}),
            }
        else:
            item = {"type": "message", "role": "assistant", "id": "done", "content": [{"type": "output_text", "text": "done"}]}
        events.append({"type": "response.output_item.done", "item": item})
        events.append({"type": "response.completed", "response": {"id": response_id, "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}}})
        body = "".join(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n" for event in events).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def native_admission(hook_mode, *, generous_budget=False):
    with tempfile.TemporaryDirectory(prefix="cas-codex-cargo-") as directory:
        root = Path(directory)
        home, repo, tools = (root / part for part in ("home", "repo", "tools"))
        for path in (home, repo, tools):
            path.mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        raw = root / "raw-cargo"
        cargo = tools / "cargo"
        cargo.write_text(f"#!/bin/sh\nprintf raw > {shlex.quote(str(raw))}\n")
        cargo.chmod(0o755)
        command = f"cargo nextest run -p cas --lib -E 'test(cas_2c8e)'"
        hook_started = root / "hook-started"
        capped = root / "capped"
        hook = root / "pre_tool.py"
        timeout = 1
        if generous_budget:
            timeout = json.loads(HOOK_CONFIG.read_text())["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"]
            assert timeout >= 120, timeout
        hook.write_text(
            "import json,sys,time\nfrom pathlib import Path\njson.load(sys.stdin)\n"
            f"Path({str(hook_started)!r}).write_text('started')\n"
            + {
                "timeout": "time.sleep(3)\n",
                "rewrite": ("time.sleep(3.2)\n" if generous_budget else "") + "print(json.dumps({'hookSpecificOutput':{'hookEventName':'PreToolUse','permissionDecision':'allow','updatedInput':{'command':"
                           + repr(f"printf capped > {shlex.quote(str(capped))}") + "}}}))\n",
            }[hook_mode]
        )
        (home / "hooks.json").write_text(json.dumps({"hooks": {"PreToolUse": [{"matcher": "^Bash$", "hooks": [{"type": "command", "command": f"/usr/bin/python3 {shlex.quote(str(hook))}", "timeout": timeout}]}]}}))
        server = ScriptedResponses(command)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            (home / "config.toml").write_text(f'''model = "gpt-5.2-codex"
model_provider = "fixture"
[features]
hooks = true
[model_providers.fixture]
name = "fixture"
base_url = "http://127.0.0.1:{server.server_port}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
''')
            # Fresh child-only home and explicit environment: do not copy any
            # account auth, provider secrets or factory identity into Codex.
            env = {"HOME": str(home), "CODEX_HOME": str(home), "PATH": f"{tools}:/usr/bin:/bin", "SHELL": "/bin/bash", "TERM": "dumb"}
            executable = str(CODEX)
            argv = [executable]
            with Path(executable).open("rb") as binary:
                node_script = binary.read(50).startswith(b"#!/usr/bin/env node")
            if node_script:
                node = shutil.which("node")
                if node is None:
                    raise AssertionError("the selected Codex script requires node on PATH")
                argv.insert(0, node)
            argv += ["exec", "--dangerously-bypass-approvals-and-sandbox", "--dangerously-bypass-hook-trust", "--skip-git-repo-check", "--json", "-C", str(repo), "Run the prescribed fixture command."]
            result = subprocess.run(argv, env=env, text=True, capture_output=True, timeout=25, stdin=subprocess.DEVNULL)
            if result.returncode != 0 or server.calls != 2:
                raise AssertionError(f"fixture did not complete its scripted tool turn: {result.returncode}, {server.calls} requests, {result.stdout}{result.stderr}")
            return {"exit": result.returncode, "raw": raw.exists(), "capped": capped.exists(), "hook": hook_started.exists(), "calls": server.calls, "output": result.stdout + result.stderr}
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


class NativeCargoAdmission(unittest.TestCase):
    def setUp(self):
        if CODEX is None:
            self.skipTest("Codex is not installed; native boundary requires codex on PATH or --codex")

    def test_generous_budget_survives_delay_exceeding_old_timeout(self):
        result = native_admission("rewrite", generous_budget=True)
        self.assertTrue(result["hook"], result)
        self.assertFalse(result["raw"], result)
        self.assertTrue(result["capped"], result)

    def test_native_timeout_fail_open_reproduction(self):
        result = native_admission("timeout")
        self.assertTrue(result["hook"], result)
        self.assertTrue(result["raw"], result)



if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--codex", type=Path, default=shutil.which("codex"))
    args, rest = parser.parse_known_args()
    CODEX = Path(args.codex).resolve() if args.codex else None
    unittest.main(argv=[__file__] + rest)
