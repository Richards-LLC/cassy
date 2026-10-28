#!/usr/bin/env python3
"""Probe native skill catalog controls in disposable homes against a local model stub.

This is a reversible investigation, not a factory launch policy. No real model,
credentials, shared harness configuration or project files are used. The stub
only returns a final message; skill expansion must be performed by the CLI.
"""

import argparse
import http.server
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import threading
from urllib.parse import urlsplit


BODY = "NATIVE_SKILL_BODY_6d8502f5"
COUNT = 50
KEEP = 6


class Stub(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self):
        super().__init__(("127.0.0.1", 0), Handler)
        self.requests = []


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        path = urlsplit(self.path).path
        if path.endswith("count_tokens"):
            body = b'{"input_tokens":1}'
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.server.requests.append(request)
        item = {
                "type": "message", "role": "assistant", "id": "fixture-message",
                "status": "completed",
                "content": [{"type": "output_text", "text": "fixture done", "annotations": []}],
        }
        response = {"id": "fixture-response", "object": "response", "created_at": 1,
                "status": "completed", "model": "fixture", "output": [item],
                "error": None, "incomplete_details": None, "metadata": {},
                "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0,
                          "input_tokens_details": {"cached_tokens": 0},
                          "output_tokens_details": {"reasoning_tokens": 0}},
        }
        events = [
            {"type": "response.created", "response": {**response, "status": "in_progress", "output": []}},
            {"type": "response.output_item.done", "item": item, "output_index": 0},
            {"type": "response.completed", "response": response},
        ]
        if path.endswith("messages"):
            events = [
                {"type": "message_start", "message": {"id": "msg_fixture", "type": "message",
                    "role": "assistant", "content": [], "model": "claude-sonnet-4-5",
                    "stop_reason": None, "stop_sequence": None,
                    "usage": {"input_tokens": 1, "output_tokens": 0}}},
                {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
                {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "fixture done"}},
                {"type": "content_block_stop", "index": 0},
                {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 1}},
                {"type": "message_stop"},
            ]
            if not request.get("stream"):
                body = json.dumps({"id": "msg_fixture", "type": "message", "role": "assistant",
                    "model": "claude-sonnet-4-5", "content": [{"type": "text", "text": "fixture done"}],
                    "stop_reason": "end_turn", "stop_sequence": None,
                    "usage": {"input_tokens": 1, "output_tokens": 1}}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
        payload = "".join(f"event: {e['type']}\ndata: {json.dumps(dict(e, sequence_number=index))}\n\n" for index, e in enumerate(events)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)


def native_probe(executable, harness, mode, named, capture_dir):
    with tempfile.TemporaryDirectory(prefix="cas-skill-catalog-") as directory:
        root = Path(directory)
        home, repo, profile = (root / name for name in ("home", "repo", "profile"))
        for path in (home, repo, profile):
            path.mkdir()
        skills = home / ".agents/skills" if harness == "codex" else profile / "skills"
        disabled = []
        for index in range(COUNT):
            name = f"fixture-skill-{index:02}"
            skill = skills / name
            skill.mkdir(parents=True)
            description = "Use only for this fixture. " + "Representative catalog description. " * 9
            (skill / "SKILL.md").write_text(
                f"---\nname: {name}\ndescription: {description}\n---\n{BODY}_ORIGINAL {name}\n"
            )
            if index >= KEEP and mode == "explicit-only" and harness == "codex":
                metadata = skill / "agents/openai.yaml"
                metadata.parent.mkdir()
                metadata.write_text("policy:\n  allow_implicit_invocation: false\n")
            elif index >= KEEP and mode == "explicit-only":
                file = skill / "SKILL.md"
                file.write_text(file.read_text().replace("\n---\n", "\ndisable-model-invocation: true\n---\n"))
            if index >= KEEP and mode == "disabled":
                disabled.append(skill / "SKILL.md")
            if mode == "local-root":
                project_skill = repo / {"codex": ".agents", "claude": ".claude", "grok": ".grok"}[harness] / "skills" / name
                shutil.copytree(skill, project_skill)
                file = project_skill / "SKILL.md"
                file.write_text(file.read_text().replace(f"{BODY}_ORIGINAL", f"{BODY}_PROJECT"))
                if index >= KEEP:
                    if harness == "codex":
                        metadata = project_skill / "agents/openai.yaml"
                        metadata.parent.mkdir()
                        metadata.write_text("policy:\n  allow_implicit_invocation: false\n")
                    else:
                        file.write_text(file.read_text().replace("\n---\n", "\ndisable-model-invocation: true\n---\n"))
                if harness == "codex":
                    disabled.append(skill / "SKILL.md")
        original_files = {file: file.read_bytes() for file in skills.rglob("*") if file.is_file()}
        stub = Stub()
        thread = threading.Thread(target=stub.serve_forever, daemon=True)
        thread.start()
        try:
            config = f'''model = "gpt-5.2-codex"
model_provider = "fixture"
[model_providers.fixture]
name = "fixture"
base_url = "http://127.0.0.1:{stub.server_port}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
'''
            for path in disabled:
                config += f'\n[[skills.config]]\npath = {json.dumps(str(path))}\nenabled = false\n'
            if harness == "grok":
                config = f'''[models]
default = "fixture"
[model.fixture]
model = "fixture"
base_url = "http://127.0.0.1:{stub.server_port}/v1"
name = "fixture"
env_key = "XAI_API_KEY"
api_backend = "responses"
supports_backend_search = false
context_window = 200000
'''
            (profile / "config.toml").write_text(config)
            env = {"HOME": str(home), "CODEX_HOME": str(profile), "GROK_HOME": str(profile),
                   "CLAUDE_CONFIG_DIR": str(profile), "PATH": "/usr/bin:/bin",
                   "SHELL": "/bin/bash", "TERM": "dumb"}
            if harness == "claude":
                env.update({"ANTHROPIC_API_KEY": "fixture-only", "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{stub.server_port}",
                            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_AUTOUPDATER": "1"})
            if harness == "grok":
                env["XAI_API_KEY"] = "fixture-only"
            argv = [str(executable)]
            with executable.open("rb") as binary:
                node_script = binary.read(50).startswith(b"#!/usr/bin/env node")
            if node_script:
                argv.insert(0, shutil.which("node"))
            prompt = ("Use $fixture-skill-49." if harness == "codex" else "/fixture-skill-49") if named else "Reply with fixture done."
            if harness == "codex":
                argv += ["exec", "--sandbox", "read-only", "--skip-git-repo-check", "--json", "-C", str(repo), prompt]
            elif harness == "claude":
                argv += ["-p", prompt, "--output-format", "json", "--no-session-persistence", "--strict-mcp-config"]
            else:
                argv += ["-p", prompt, "-m", "fixture", "--cwd", str(repo), "--output-format", "json", "--disable-web-search", "--no-subagents"]
            result = subprocess.run(argv, env=env, cwd=repo, text=True, capture_output=True,
                                    timeout=45, stdin=subprocess.DEVNULL)
            model_requests = [request for request in stub.requests if "input" in request or "messages" in request]
            if result.returncode or not model_requests:
                raise RuntimeError(f"native fixture failed: exit={result.returncode}, requests={len(stub.requests)}\n{result.stdout}\n{result.stderr}")
            texts = list(strings(model_requests[0]))
            catalogs = [text for text in texts if "fixture-skill-00" in text and "fixture-skill-01" in text]
            catalog = "\n".join(catalogs)
            # Normalize only the disposable prefix, preserving the same renderer
            # and skill paths across baseline and treatments.
            raw_catalog_bytes = len(catalog.encode())
            catalog = catalog.replace(str(root), "<fixture>")
            rows = [line for line in catalog.splitlines() if re.search(r"fixture-skill-\d\d", line)]
            names = set(re.findall(r"fixture-skill-\d\d", catalog))
            body_loaded = any(BODY in text for text in texts)
            unchanged = all(file.read_bytes() == value for file, value in original_files.items())
            if not unchanged or body_loaded != (named and mode != "disabled"):
                raise AssertionError(f"invocation/source control failed: {harness}/{mode}/{named}: body={body_loaded}, unchanged={unchanged}")
            if mode in ("baseline", "explicit-only") and len(names) != (COUNT if mode == "baseline" else KEEP):
                raise AssertionError(f"catalog extraction/control failed: {harness}/{mode}: {len(names)} names")
            capture = capture_dir / f"{harness}-{mode}-named-{str(named).lower()}.txt"
            capture.write_text(catalog + "\n")
            return {
                "mode": mode, "named": named, "exit": result.returncode,
                "requests": len(model_requests), "fixture_rows": len(names),
                "row_count": len(rows), "containing_text_bytes_raw": raw_catalog_bytes,
                "fixture_row_bytes_normalized": len("\n".join(rows).encode()),
                "containing_text_bytes_normalized": len(catalog.encode()),
                "native_body_loaded": body_loaded,
                "native_body_source": "project" if any(f"{BODY}_PROJECT" in text for text in texts) else
                                      "original" if any(f"{BODY}_ORIGINAL" in text for text in texts) else None,
                "original_files_unchanged": unchanged,
                "request_fixture_count": len(set(re.findall(r"fixture-skill-\d\d", "\n".join(texts)))),
                "named_skill_catalogued": "fixture-skill-49" in names,
            }
        finally:
            stub.shutdown()
            stub.server_close()
            thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", required=True, type=Path)
    parser.add_argument("--harness", required=True, choices=["codex", "claude", "grok"])
    parser.add_argument("--mode", choices=["baseline", "explicit-only", "disabled", "local-root"])
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if args.mode == "disabled" and args.harness != "codex":
        parser.error("the per-path disabled control is Codex-only")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    cases = [(args.mode, True)] if args.mode else [("baseline", False), ("explicit-only", False),
                                                  ("explicit-only", True), ("local-root", True)]
    results = [native_probe(args.executable.resolve(), args.harness, mode, named, args.output.parent) for mode, named in cases]
    report = {"harness": subprocess.check_output([str(args.executable), "--version"], text=True).strip(),
              "fixture_skills": COUNT, "role_skills": KEEP, "results": results}
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
