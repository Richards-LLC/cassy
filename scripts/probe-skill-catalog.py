#!/usr/bin/env python3
"""Probe native skill catalog controls in disposable homes against a local model stub.

This is a reversible investigation, not a factory launch policy. No real model,
credentials, shared harness configuration or project files are used. The stub
only returns a final message; skill expansion must be performed by the CLI.
"""

import argparse
import http.server
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
from urllib.parse import urlsplit


BODY = "NATIVE_SKILL_BODY_6d8502f5"
COUNT = 50
KEEP = 6
REPO = Path(__file__).resolve().parents[1]
CLAUDE_BUNDLED = ["dataviz", "update-config", "keybindings-help", "code-review", "simplify",
                  "fewer-permission-prompts", "loop", "claude-api", "workflow-authoring", "run",
                  "plugin-authoring", "init", "security-review"]


def role_skills(audience):
    """The shipped worker list, read from source so the probe cannot drift."""
    if audience != "worker":
        return {"cas-qa-craft"}
    text = (REPO / "crates/cas-pty/src/pty.rs").read_text()
    block = text[text.index("pub const WORKER_ROLE_SKILLS"):]
    return set(re.findall(r'"([\w-]+)"', block[:block.index("];")]))


def public_builtin_inventory(harness):
    """Read only the repository's embedded, public builtin inventory."""
    source = REPO / "cas-cli/src"
    text = (source / "builtins.rs").read_text()
    constant = {"claude": "BUILTIN_SKILLS", "codex": "CODEX_BUILTIN_SKILLS", "grok": "GROK_BUILTIN_SKILLS"}[harness]
    start = text.index(f"pub const {constant}:")
    section = text[start:text.index("\n];", start)]
    files = re.findall(r'path:\s*"(skills/[^"\n]+)",\s*content:\s*include_str!\("([^"\n]+)"\)', section)
    if not files:
        raise AssertionError(f"no embedded inventory parsed for {harness}")
    return [(Path(relative).relative_to("skills"), (source / included).read_bytes()) for relative, included in files]


def explicit_metadata(skill, harness):
    if harness == "codex":
        metadata = skill / "agents/openai.yaml"
        metadata.parent.mkdir(exist_ok=True)
        if metadata.exists():
            if "policy:\n  allow_implicit_invocation: false" in metadata.read_text():
                return
            raise AssertionError("fixture adapter cannot represent an existing native policy safely")
        metadata.write_text("policy:\n  allow_implicit_invocation: false\n")
    else:
        file = skill / "SKILL.md"
        content = file.read_text()
        if re.search(r"^disable-model-invocation:", content, re.M):
            content = re.sub(r"^disable-model-invocation:.*$", "disable-model-invocation: true", content, flags=re.M)
        else:
            content = content.replace("\n---\n", "\ndisable-model-invocation: true\n---\n", 1)
        file.write_text(content)


def native_catalog_blocks(texts, harness, allow_absent=False):
    """Measure complete native skills sections, including bundled entries.

    These delimiters are native wire contracts, not a token/row estimate. Fail
    when a recorded renderer changes rather than silently count owned rows.
    """
    blocks = []
    for text in texts:
        if harness == "claude":
            marker = "The following skills are available for use with the Skill tool:"
            if marker in text:
                tail = text[text.index(marker):]
                ends = [tail.index(end) for end in ("\n<total_tokens>", "\nToday's date is") if end in tail]
                if not ends:
                    raise AssertionError("Claude native catalog end delimiter changed")
                blocks.append(tail[:min(ends)])
        elif harness == "codex":
            blocks.extend(re.findall(r"<skills_instructions>.*?</skills_instructions>", text, re.S))
        else:
            marker = "The following skills are available for use:"
            if marker in text:
                tail = text[text.index(marker):]
                end = "\nThe following workflows are available:"
                if end not in tail and "</system-reminder>" not in tail:
                    raise AssertionError("Grok native catalog end delimiter changed")
                blocks.append(tail.split(end, 1)[0].split("</system-reminder>", 1)[0])
    if not blocks and not allow_absent:
        raise AssertionError(f"no complete native skill section observed for {harness}")
    return blocks


def refresh_link_control(root):
    """Empty synthetic canaries only: model rename routing, never read secrets."""
    selected = root / "selected-route-canary"
    overlay = root / "overlay-route-canary"
    replacement = root / "replacement-route-canary"
    selected.touch()
    original_inode = selected.stat().st_ino
    overlay.symlink_to(selected)
    replacement.touch()
    os.replace(replacement, overlay)
    split = not overlay.is_symlink() and overlay.stat().st_ino != selected.stat().st_ino
    if not split or selected.stat().st_ino != original_inode:
        raise AssertionError("atomic rename link control did not exercise the expected routing")
    return {"atomic_rename_replaces_link": split, "credential_contents_read_or_copied": False,
            "native_auth_refresh_verified": False}


def contains_settings(expected, actual):
    """Compare only generated, public fixture settings; no profile discovery."""
    if isinstance(expected, dict):
        return isinstance(actual, dict) and all(key in actual and contains_settings(value, actual[key]) for key, value in expected.items())
    return expected == actual


class Stub(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self):
        super().__init__(("127.0.0.1", 0), Handler)
        self.requests = []
        self.spawn = None


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
        if path.endswith("chat/completions"):
            chunks = [{"id": "c", "object": "chat.completion.chunk", "created": 1, "model": "fixture",
                       "choices": [{"index": 0, "delta": {"role": "assistant", "content": "fixture done"}, "finish_reason": None}]},
                      {"id": "c", "object": "chat.completion.chunk", "created": 1, "model": "fixture",
                       "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                       "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}]
            payload = ("".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        spawn = self.server.spawn if len(self.server.requests) == 1 else None
        if spawn and path.endswith("messages") and request.get("stream"):
            events = [
                {"type": "message_start", "message": {"id": "msg_spawn", "type": "message", "role": "assistant",
                    "content": [], "model": "claude-sonnet-4-5", "stop_reason": None, "stop_sequence": None,
                    "usage": {"input_tokens": 1, "output_tokens": 0}}},
                {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_spawn", "name": spawn["name"], "input": {}}},
                {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": json.dumps(spawn["input"])}},
                {"type": "content_block_stop", "index": 0},
                {"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": None}, "usage": {"output_tokens": 1}},
                {"type": "message_stop"},
            ]
            payload = "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        item = {
                "type": "message", "role": "assistant", "id": "fixture-message",
                "status": "completed",
                "content": [{"type": "output_text", "text": "fixture done", "annotations": []}],
        }
        if spawn and not path.endswith("messages"):
            item = {"type": "function_call", "id": "fc_spawn", "call_id": "call_spawn", "status": "completed",
                    "name": spawn["name"], "arguments": json.dumps(spawn["input"])}
            if spawn.get("namespace"):
                item["namespace"] = spawn["namespace"]
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


VERIFIER_MARKER = "verification gatekeeper"
SPAWN = {
    "claude": {"name": "Agent", "input": {"subagent_type": "task-verifier", "description": "verify", "prompt": "Verify fixture task."}},
    "codex": {"namespace": "multi_agent_v1", "name": "spawn_agent", "input": {"agent_type": "task-verifier", "message": "Verify fixture task."}},
    "grok": {"name": "spawn_subagent", "input": {"agent": "task-verifier", "subagent_type": "task-verifier", "description": "verify", "prompt": "Verify fixture task."}},
}


def verifier_child_probe(executable, harness, mode, capture_dir):
    """Script one verifier spawn; measure the child's own first model request.

    The role definition is the shipped builtin. Every other parent-side control
    comes from the same launch recipe `native_probe` measures.
    """
    with tempfile.TemporaryDirectory(prefix="cas-skill-child-") as directory:
        root = Path(directory)
        home, repo, profile = (root / name for name in ("home", "repo", "profile"))
        for path in (home, repo, profile):
            path.mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True, capture_output=True)
        skills = home / ".agents/skills" if harness == "codex" else profile / "skills"
        for relative, content in public_builtin_inventory(harness):
            file = skills / relative
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(content)
        source = REPO / "cas-cli/src/builtins"
        agents = profile / "agents"
        agents.mkdir()
        if harness == "codex":
            shutil.copy(source / "codex/agents/task-verifier.toml", agents)
        else:
            prefix = "mcp__cas__" if harness == "claude" else "cas__"
            tools = ", ".join(prefix + tool for tool in ("task", "verification", "rule", "search", "artifact", "coordination"))
            (agents / "task-verifier.md").write_text(
                "---\nname: task-verifier\ndescription: Internal agent for verifying task completion.\n"
                f"model: inherit\ntools: Read, Grep, Glob, Bash, {tools}\n---\n"
                + (source / "agents/task-verifier.body.md").read_text())
        stub = Stub()
        stub.spawn = SPAWN[harness]
        thread = threading.Thread(target=stub.serve_forever, daemon=True)
        thread.start()
        try:
            env = {"HOME": str(home), "CODEX_HOME": str(profile), "GROK_HOME": str(profile), "CLAUDE_CONFIG_DIR": str(profile),
                   "PATH": "/usr/bin:/bin", "SHELL": "/bin/bash", "TERM": "dumb"}
            argv = [str(executable)]
            with executable.open("rb") as binary:
                if binary.read(50).startswith(b"#!/usr/bin/env node"):
                    argv.insert(0, shutil.which("node"))
            if harness == "claude":
                env.update({"ANTHROPIC_API_KEY": "fixture-only", "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{stub.server_port}",
                            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_AUTOUPDATER": "1"})
                argv += ["-p", "Spawn the verifier.", "--output-format", "json", "--no-session-persistence",
                         "--strict-mcp-config", "--permission-mode", "bypassPermissions"]
            elif harness == "codex":
                (profile / "config.toml").write_text(f"""model = "gpt-5.2-codex"
model_provider = "fixture"
[model_providers.fixture]
name = "fixture"
base_url = "http://127.0.0.1:{stub.server_port}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
""")
                argv += ["exec", "--sandbox", "read-only", "--skip-git-repo-check", "--json", "-C", str(repo)]
                if mode == "no-instructions":
                    argv += ["-c", "skills.include_instructions=false"]
                argv.append("Spawn the verifier.")
            else:
                env["XAI_API_KEY"] = "fixture-only"
                (profile / "config.toml").write_text(f"""[models]
default = "fixture"
[model.fixture]
model = "fixture"
base_url = "http://127.0.0.1:{stub.server_port}/v1"
name = "fixture"
env_key = "XAI_API_KEY"
api_backend = "responses"
supports_backend_search = false
context_window = 200000
""")
                argv += ["-p", "Spawn the verifier.", "-m", "fixture", "--cwd", str(repo), "--output-format", "json", "--always-approve"]
            result = subprocess.run(argv, env=env, cwd=repo, text=True, capture_output=True, timeout=90, stdin=subprocess.DEVNULL)
            requests = [r for r in stub.requests if "input" in r or "messages" in r]
            children = [r for r in requests if any(VERIFIER_MARKER in t for t in strings({k: v for k, v in r.items() if k != "tools"}))]
            names = [t.get("name") or t.get("function", {}).get("name") for t in requests[0].get("tools", [])]
            (capture_dir / f"{harness}-parent-{mode}-wire.json").write_text(json.dumps(requests[0], indent=2) + "\n")
            report = {"harness": harness, "mode": mode, "exit": result.returncode, "requests": len(requests),
                      "native_child": bool(children), "parent_tools": names, "stderr_tail": result.stderr[-400:]}
            if children:
                (capture_dir / f"{harness}-child-{mode}-wire.json").write_text(json.dumps(children[0], indent=2) + "\n")
                blocks = native_catalog_blocks(list(strings(children[0])), harness, allow_absent=True)
                report["child_catalog_characters"] = len("\n".join(blocks))
            return report
        finally:
            stub.shutdown()
            stub.server_close()
            thread.join(timeout=5)


def opencode_probe(executable, mode, audience, named, capture_dir):
    """Measure OpenCode's skill section; catalog and role come from inline config."""
    with tempfile.TemporaryDirectory(prefix="cas-skill-opencode-") as directory:
        root = Path(directory)
        home, repo = root / "home", root / "repo"
        home.mkdir()
        repo.mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True, capture_output=True)
        skills = repo / ".opencode/skills"
        for relative, content in public_builtin_inventory("claude"):
            file = skills / relative
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(content)
        for file in skills.glob("*/SKILL.md"):
            file.write_text(file.read_text() + f"\n{BODY}_ORIGINAL {file.parent.name}\n")
        selected = role_skills(audience)
        stub = Stub()
        thread = threading.Thread(target=stub.serve_forever, daemon=True)
        thread.start()
        try:
            agent = {"mode": "primary", "description": "fixture", "prompt": "fixture role"}
            if mode == "permission":
                agent["permission"] = {"skill": {"*": "deny", **{name: "allow" for name in sorted(selected)}}}
            config = {"agent": {"cassy-worker": agent},
                      "provider": {"fixture": {"npm": "@ai-sdk/openai-compatible", "name": "fixture",
                          "options": {"baseURL": f"http://127.0.0.1:{stub.server_port}/v1", "apiKey": "fixture-only"},
                          "models": {"model": {"name": "model"}}}}}
            env = {"HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config"), "XDG_DATA_HOME": str(home / ".data"),
                   "XDG_CACHE_HOME": str(home / ".cache"), "XDG_STATE_HOME": str(home / ".state"),
                   "PATH": "/usr/bin:/bin", "SHELL": "/bin/bash", "TERM": "dumb",
                   "OPENCODE_CONFIG_CONTENT": json.dumps(config), "OPENCODE_DISABLE_AUTOUPDATE": "1",
                   "OPENCODE_DISABLE_MODELS_FETCH": "1", "OPENCODE_DISABLE_LSP_DOWNLOAD": "1"}
            argv = [str(executable), "run", "--agent", "cassy-worker", "-m", "fixture/model", "--dir", str(repo)]
            argv += ["--command", "cas-technical-drawing", "go"] if named else ["Reply with fixture done."]
            result = subprocess.run(argv, env=env, cwd=repo, text=True, capture_output=True, timeout=120, stdin=subprocess.DEVNULL)
            requests = [r for r in stub.requests if "messages" in r]
            if not requests:
                raise RuntimeError(f"opencode fixture made no request: exit={result.returncode}\n{result.stdout[-800:]}\n{result.stderr[-800:]}")
            main = max(requests, key=lambda r: len(json.dumps(r)))
            (capture_dir / f"opencode-{mode}-{audience}-named-{str(named).lower()}-wire.json").write_text(json.dumps(main, indent=2) + "\n")
            sections = []
            for text in strings(main):
                sections += re.findall(r"<available_skills>.*?</available_skills>", text, re.S)
            complete = "\n".join(dict.fromkeys(sections))
            names = sorted(set(re.findall(r"<name>([\w-]+)</name>", complete)))
            return {"harness": "opencode", "mode": mode, "audience": audience, "named": named, "exit": result.returncode,
                    "full_native_catalog_characters": len(complete), "catalog_names": names,
                    "native_body_loaded": any(BODY in text for text in strings(main)), "stderr_tail": result.stderr[-300:]}
        finally:
            stub.shutdown()
            stub.server_close()
            thread.join(timeout=5)


def native_probe(executable, harness, mode, named, capture_dir, inventory="synthetic", audience="worker", invoke=None):
    with tempfile.TemporaryDirectory(prefix="cas-skill-catalog-") as directory:
        root = Path(directory)
        home, repo, profile = (root / name for name in ("home", "repo", "profile"))
        for path in (home, repo, profile):
            path.mkdir()
        if inventory == "builtin":
            subprocess.run(["git", "init", "-q", str(repo)], check=True, capture_output=True)
        skills = home / ".agents/skills" if harness == "codex" else profile / "skills"
        disabled = []
        for index in range(COUNT if inventory == "synthetic" else 0):
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
        if inventory == "builtin":
            for relative, content in public_builtin_inventory(harness):
                file = skills / relative
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_bytes(content)
            for file in skills.glob("*/SKILL.md"):
                file.write_text(file.read_text() + f"\n{BODY}_ORIGINAL {file.parent.name}\n")
            if mode in ("local-root", "extra-root", "env-overlay", "trusted-root"):
                project = repo / {"codex": ".agents/skills", "grok": ".grok/skills"}[harness]
                shutil.copytree(skills, project)
                if mode == "trusted-root":
                    (project / ".gitignore").write_text("*\n")
                selected = role_skills(audience)
                for file in project.glob("*/SKILL.md"):
                    file.write_text(file.read_text().replace(f"{BODY}_ORIGINAL", f"{BODY}_PROJECT"))
                    if file.parent.name not in selected:
                        explicit_metadata(file.parent, harness)
                    if harness == "codex":
                        disabled.append(skills / file.parent.name / "SKILL.md")
        view = None
        if mode == "view":
            view = root / "catalog-view"
            shutil.copytree(skills, view)
            selected = role_skills(audience)
            for file in view.glob("*/SKILL.md"):
                if file.parent.name in selected:
                    text = file.read_text()
                    text = re.sub(r"^description:.*$", f"description: Factory {audience} workflow.", text, count=1, flags=re.M)
                    file.write_text(text)
                else:
                    explicit_metadata(file.parent, harness)
            (view / "CATALOG.txt").write_text("\n".join(sorted(file.parent.name for file in view.glob("*/SKILL.md"))) + "\n")
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
                if mode == "settings":
                    selected = role_skills(audience)
                    config += "\n[skills]\ndisabled = " + json.dumps(sorted(file.parent.name for file in skills.glob("*/SKILL.md") if file.parent.name not in selected)) + "\n"
                elif mode == "extra-root":
                    config += "\n[skills]\npaths = " + json.dumps([str(project)]) + "\n"
            (profile / "config.toml").write_text(config)
            env = {"HOME": str(home), "CODEX_HOME": str(profile), "GROK_HOME": str(profile),
                   "CLAUDE_CONFIG_DIR": str(profile), "PATH": "/usr/bin:/bin",
                   "SHELL": "/bin/bash", "TERM": "dumb"}
            if harness == "claude":
                env.update({"ANTHROPIC_API_KEY": "fixture-only", "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{stub.server_port}",
                            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_AUTOUPDATER": "1"})
            if harness == "grok":
                env["XAI_API_KEY"] = "fixture-only"
                if mode == "trusted-root":
                    env["GROK_FOLDER_TRUST"] = "false"
                if mode == "env-overlay":
                    overlay = root / "discovery-overlay.toml"
                    overlay.write_text("[skills]\npaths = " + json.dumps([str(project)]) + "\n")
                    env["GROK_CONFIG_PATH"] = str(overlay)
            argv = [str(executable)]
            with executable.open("rb") as binary:
                node_script = binary.read(50).startswith(b"#!/usr/bin/env node")
            if node_script:
                argv.insert(0, shutil.which("node"))
            invoked = invoke or ("fixture-skill-49" if inventory == "synthetic" else "cas-technical-drawing")
            prompt = (f"Use ${invoked}." if harness == "codex" else f"/{invoked}") if named else "Reply with fixture done."
            if harness == "codex":
                argv += ["exec", "--sandbox", "read-only", "--skip-git-repo-check", "--json", "-C", str(repo)]
                if mode == "no-instructions":
                    argv += ["-c", "skills.include_instructions=false"]
                argv.append(prompt)
            elif harness == "claude":
                argv += ["-p", prompt, "--output-format", "json", "--no-session-persistence", "--strict-mcp-config"]
                if mode == "settings":
                    selected = role_skills(audience)
                    overrides = {file.parent.name: "user-invocable-only" for file in skills.glob("*/SKILL.md") if file.parent.name not in selected}
                    overrides.update({name: "user-invocable-only" for name in CLAUDE_BUNDLED})
                    argv += ["--settings", json.dumps({"skillOverrides": overrides})]
            else:
                argv += ["-p", prompt, "-m", "fixture", "--cwd", str(repo), "--output-format", "json", "--disable-web-search", "--no-subagents"]
            if view is not None:
                if not shutil.which("bwrap"):
                    raise RuntimeError("process-local view unsupported: bubblewrap is unavailable")
                argv = ["bwrap", "--dev-bind", "/", "/", "--unshare-user", "--die-with-parent",
                        "--ro-bind", str(view), str(skills), "--"] + argv
            result = subprocess.run(argv, env=env, cwd=repo, text=True, capture_output=True,
                                    timeout=45, stdin=subprocess.DEVNULL)
            model_requests = [request for request in stub.requests if "input" in request or "messages" in request]
            if result.returncode or not model_requests:
                if view is not None and "No permissions to create a new namespace" in result.stderr:
                    return {"mode": mode, "inventory": inventory, "audience": audience,
                            "status": "unsupported", "exit": result.returncode, "requests": len(model_requests),
                            "reason": result.stderr.strip(), "full_native_catalog_characters": None,
                            "native_body_loaded": False,
                            "original_skill_files_unchanged": all(file.read_bytes() == value for file, value in original_files.items()),
                            "refresh_link_control": refresh_link_control(root)}
                raise RuntimeError(f"native fixture failed: exit={result.returncode}, requests={len(stub.requests)}\n{result.stdout}\n{result.stderr}")
            texts = list(strings(model_requests[0]))
            case = f"{harness}-{inventory}-{mode}-{audience}-named-{str(named).lower()}-{invoked}"
            wire_capture = capture_dir / f"{case}-wire.json"
            wire_capture.write_text(json.dumps(model_requests[0], indent=2) + "\n")
            complete = "\n".join(native_catalog_blocks(texts, harness, allow_absent=mode == "no-instructions"))
            complete_names = sorted(set(re.findall(r"^- ([\w-]+):", complete, re.M)))
            full_capture = capture_dir / f"{case}-full-native.txt"
            full_capture.write_text(complete + "\n")
            catalogs = [text for text in texts if "fixture-skill-00" in text and "fixture-skill-01" in text]
            catalog = "\n".join(catalogs)
            # Normalize only the disposable prefix, preserving the same renderer
            # and skill paths across baseline and treatments.
            raw_catalog_bytes = len(catalog.encode())
            catalog = catalog.replace(str(root), "<fixture>")
            rows = [line for line in catalog.splitlines() if re.search(r"fixture-skill-\d\d", line)]
            names = set(re.findall(r"fixture-skill-\d\d", catalog))
            body_loaded = any(BODY in text for text in texts)
            if invoked == "init":
                user_texts = list(strings([message for message in model_requests[0]["messages"] if message["role"] == "user"]))
                body_loaded = any("CLAUDE.md" in text and "codebase" in text and "/init" != text.strip() for text in user_texts)
            unchanged = all(file.read_bytes() == value for file, value in original_files.items())
            expected_body = named and mode != "disabled" and not (harness == "grok" and mode == "settings")
            if not unchanged or body_loaded != expected_body:
                raise AssertionError(f"invocation/source control failed: {harness}/{mode}/{named}: body={body_loaded}, unchanged={unchanged}")
            if inventory == "synthetic" and mode in ("baseline", "explicit-only") and len(names) != (COUNT if mode == "baseline" else KEEP):
                raise AssertionError(f"catalog extraction/control failed: {harness}/{mode}: {len(names)} names")
            capture = capture_dir / f"{case}-containing-text.txt"
            capture.write_text(catalog + "\n")
            return {
                "mode": mode, "named": named, "exit": result.returncode,
                "status": "observed",
                "inventory": inventory, "audience": audience,
                "invoked": invoked if named else None,
                "native_child": False,
                "full_native_capture": full_capture.name, "wire_capture": wire_capture.name,
                "owned_skill_count": len(list(skills.glob("*/SKILL.md"))),
                "full_native_catalog_characters": len(complete),
                "full_native_catalog_bytes": len(complete.encode()),
                "full_native_catalog_names": complete_names,
                "full_native_catalog_below_2500": len(complete) < 2500,
                "account_selector_override_requested": False,
                "process_view": view is not None,
                "requests": len(model_requests), "fixture_rows": len(names),
                "row_count": len(rows), "containing_text_bytes_raw": raw_catalog_bytes,
                "fixture_row_bytes_normalized": len("\n".join(rows).encode()),
                "containing_text_bytes_normalized": len(catalog.encode()),
                "native_body_loaded": body_loaded,
                "native_body_source": "bundled" if invoked == "init" and body_loaded else
                                      "project" if any(f"{BODY}_PROJECT" in text for text in texts) else
                                      "original" if any(f"{BODY}_ORIGINAL" in text for text in texts) else None,
                "original_skill_files_unchanged": unchanged,
                "generated_profile_config_unchanged": (profile / "config.toml").read_bytes() == config.encode(),
                "generated_profile_settings_preserved": contains_settings(tomllib.loads(config), tomllib.loads((profile / "config.toml").read_text())),
                "request_fixture_count": len(set(re.findall(r"fixture-skill-\d\d", "\n".join(texts)))),
                "named_skill_catalogued": invoked in complete_names,
            }
        finally:
            stub.shutdown()
            stub.server_close()
            thread.join(timeout=5)


MATRIX = {
    "claude": ("claude", "settings"),
    "codex": ("codex", "no-instructions"),
    "grok": ("grok", "trusted-root"),
    "opencode": ("opencode", "permission"),
}


def replay_matrix(output):
    """Replay the shipped recipe per harness: before, after and verifier child."""
    output.mkdir(parents=True, exist_ok=True)
    rows = {}
    for harness, (command, mode) in MATRIX.items():
        executable = Path(shutil.which(command)).resolve()
        for audience in ("worker", "reviewer"):
            if harness == "opencode":
                before = opencode_probe(executable, "baseline", audience, False, output)
                after = opencode_probe(executable, mode, audience, True, output)
            else:
                before = native_probe(executable, harness, "baseline", False, output, "builtin", audience)
                after = native_probe(executable, harness, mode, True, output, "builtin", audience)
            rows[f"{harness}/{audience}"] = {
                "before_characters": before["full_native_catalog_characters"],
                "after_characters": after["full_native_catalog_characters"],
                "named_body_expands": after["native_body_loaded"],
                "under_2500": after["full_native_catalog_characters"] < 2500,
            }
        if harness != "opencode":
            child = verifier_child_probe(executable, harness, "baseline", output)
            rows[f"{harness}/verifier-child"] = {"native_child": child["native_child"],
                                                 "after_characters": child.get("child_catalog_characters")}
    (output / "matrix.json").write_text(json.dumps(rows, indent=2) + "\n")
    print(json.dumps(rows, indent=2))
    if not all(row.get("under_2500", True) and row.get("named_body_expands", True)
               and row.get("native_child", True) for row in rows.values()):
        raise SystemExit(1)


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--matrix":
        replay_matrix(Path(sys.argv[2]))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", required=True, type=Path)
    parser.add_argument("--harness", required=True, choices=["codex", "claude", "grok", "opencode"])
    parser.add_argument("--mode", choices=["baseline", "explicit-only", "disabled", "local-root", "view", "settings", "extra-root", "env-overlay", "trusted-root", "no-instructions", "permission"])
    parser.add_argument("--named", choices=["yes", "no"])
    parser.add_argument("--inventory", choices=["synthetic", "builtin"], default="synthetic")
    parser.add_argument("--audience", choices=["worker", "reviewer"], default="worker")
    parser.add_argument("--invoke", choices=["cas-technical-drawing", "init"])
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--verifier-child", action="store_true", help="script a native verifier spawn and measure the child")
    args = parser.parse_args()
    if args.harness == "opencode":
        args.output.parent.mkdir(parents=True, exist_ok=True)
        report = opencode_probe(args.executable.resolve(), args.mode or "baseline", args.audience, args.named != "no", args.output.parent)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        return
    if args.verifier_child:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        report = verifier_child_probe(args.executable.resolve(), args.harness, args.mode or "baseline", args.output.parent)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        return
    if args.invoke == "init" and (args.harness != "claude" or args.inventory != "builtin" or args.mode != "settings"):
        parser.error("bundled init control requires Claude builtin settings mode")
    if args.mode == "disabled" and args.harness != "codex":
        parser.error("the per-path disabled control is Codex-only")
    if args.mode == "view" and args.inventory != "builtin":
        parser.error("the view experiment uses only the public builtin inventory")
    if args.mode == "settings" and args.harness == "codex":
        parser.error("native visibility settings comparator is Claude/Grok-only")
    if args.inventory == "builtin" and args.mode not in ("baseline", "view", "settings", "local-root", "extra-root", "env-overlay", "trusted-root", "no-instructions"):
        parser.error("builtin inventory requires baseline, view, local-root, extra-root, trusted-root, no-instructions or settings mode")
    if args.mode == "trusted-root" and args.harness != "grok":
        parser.error("trusted-root is the Grok project-root comparator")
    if args.mode == "no-instructions" and args.harness != "codex":
        parser.error("no-instructions is the Codex catalog-instruction control")
    if args.inventory == "builtin" and args.mode == "local-root" and args.harness not in ("codex", "grok"):
        parser.error("the local-root comparator is Codex/Grok-only")
    if args.mode in ("extra-root", "env-overlay") and (args.inventory != "builtin" or args.harness != "grok"):
        parser.error("extra-root/env-overlay are Grok public inventory comparators")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    cases = [(args.mode, args.named != "no")] if args.mode else [("baseline", False), ("explicit-only", False),
                                                  ("explicit-only", True), ("local-root", True)]
    results = [native_probe(args.executable.resolve(), args.harness, mode, named, args.output.parent,
                            args.inventory, args.audience, args.invoke) for mode, named in cases]
    report = {"harness": subprocess.check_output([str(args.executable), "--version"], text=True).strip(),
              "fixture_skills": COUNT if args.inventory == "synthetic" else None,
              "role_skills": KEEP if args.inventory == "synthetic" else 1, "results": results}
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if any(result.get("status") == "unsupported" for result in results):
        raise SystemExit(3)


if __name__ == "__main__":
    main()
