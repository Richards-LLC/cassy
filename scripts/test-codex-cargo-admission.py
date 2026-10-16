#!/usr/bin/env python3
"""Exercise installed Codex admission with scripted local responses and fake Cargo.

No model, credentials, live MCP server or Rust compiler is used. Pass --codex
explicitly to run the native boundary; ordinary CI without Codex records skips.
The guard rules come from the shipped factory resource, not a test reimplementation.
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
GUARD_RULES = ROOT / "crates/cas-pty/src/codex-cargo-guard.rules"
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


def native_admission(hook_mode, *, absolute=False, rules=True):
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
        command = f"{shlex.quote(str(cargo)) if absolute else 'cargo'} nextest run -p cas --lib -E 'test(cas_2c8e)'"
        hook_started = root / "hook-started"
        capped = root / "capped"
        hook = root / "pre_tool.py"
        hook.write_text(
            "import json,sys,time\nfrom pathlib import Path\njson.load(sys.stdin)\n"
            f"Path({str(hook_started)!r}).write_text('started')\n"
            + {
                "timeout": "time.sleep(3)\n",
                "error": "raise SystemExit(1)\n",
                "invalid": "print('{malformed')\n",
                "rewrite": "print(json.dumps({'hookSpecificOutput':{'hookEventName':'PreToolUse','permissionDecision':'allow','updatedInput':{'command':"
                           + repr(f"printf capped > {shlex.quote(str(capped))}") + "}}}))\n",
                "missing": "",
            }[hook_mode]
        )
        if hook_mode != "missing":
            (home / "hooks.json").write_text(json.dumps({"hooks": {"PreToolUse": [{"matcher": "^Bash$", "hooks": [{"type": "command", "command": f"/usr/bin/python3 {shlex.quote(str(hook))}", "timeout": 1}]}]}}))
        if rules and GUARD_RULES.exists():
            (home / "rules").mkdir()
            shutil.copyfile(GUARD_RULES, home / "rules/cassy-cargo.rules")
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
            self.skipTest("native Codex boundary requires explicit --codex path")

    def test_slow_hook_cannot_run_raw_cargo(self):
        result = native_admission("timeout")
        self.assertTrue(result["hook"], result)
        self.assertFalse(result["raw"], result)
        self.assertIn("GUARD UNAVAILABLE", result["output"])

    def test_error_invalid_or_absent_hook_cannot_run_raw_cargo(self):
        for mode in ("error", "invalid", "missing"):
            with self.subTest(mode=mode):
                result = native_admission(mode)
                self.assertFalse(result["raw"], result)
                self.assertIn("GUARD UNAVAILABLE", result["output"])

    def test_absolute_cargo_is_also_guarded(self):
        result = native_admission("timeout", absolute=True)
        self.assertFalse(result["raw"], result)

    def test_successful_hook_rewrite_still_executes(self):
        result = native_admission("rewrite")
        self.assertFalse(result["raw"], result)
        self.assertTrue(result["capped"], result)

    def test_unprotected_fixture_reproduces_native_timeout_fail_open(self):
        result = native_admission("timeout", rules=False)
        self.assertTrue(result["hook"], result)
        self.assertTrue(result["raw"], result)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--codex", type=Path)
    args, rest = parser.parse_known_args()
    CODEX = args.codex.resolve() if args.codex else None
    unittest.main(argv=[__file__] + rest)
