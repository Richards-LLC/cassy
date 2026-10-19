#!/usr/bin/env python3
"""No-build lint: shipped text suggests only MCP calls the server accepts.

The single implementation of the call-shape lint (cas-043b). It scans
- every builtin file `include_str!`-ed by cas-cli/src/builtins.rs and
  cas-cli/src/maintenance_jobs.rs (skills, agents, jobs, every flavor), and
- runtime Rust templates in cas-cli/src and crates/*/src, test code excluded,
for `tool action=name key=value ...` calls. Each call must name an accepted
action, only published fields, and the fields its handler requires.

The accepted surface is scripts/mcp-call-surface.json. The Rust test
mcp_action_surface_test::call_shape_surface_snapshot_matches_the_published_surface
keeps it equal to cas_mcp::actions and the tools/list input schemas.

Exit 0 when clean, 1 with offenders (each with a FIX: line), 2 on bad input.
"""

from __future__ import annotations

import json
from pathlib import Path
import re
import sys

SURFACE = "scripts/mcp-call-surface.json"
# Fields a handler rejects the call without, for every caller.
REQUIRED_FIELDS = {
    ("coordination", "message"): ("target", "summary", "message"),
    ("coordination", "interrupt"): ("target", "summary", "message"),
    ("coordination", "remind"): ("remind_message",),
    ("verification", "add"): ("dispatch_id",),
    ("task", "create"): ("title",),
}
# The canonical verifier body explains when dispatch_id must be supplied; its
# generated projections share that conditional template. Runtime authority is
# still required, including for native Codex roles that cannot mint a handoff.
VERIFIER_DISPATCH_TEMPLATES = (
    "agents/task-verifier.md",
    "agents/task-verifier.body.md",
    "builtins/codex/agents/task-verifier.toml",
)
# `task action=create` needs `risk` unless the type is exempt.
RISK_EXEMPT_TASK_TYPES = {"epic", "spike", "chore", "gate"}
# Format-string variables that stand for a prefixed tool in runtime text.
TEMPLATE_TOOL_VARIABLES = {"coord": "coordination", "sup_ver": "verification", "caller_task": "task"}
KEY = re.compile(r"\[?([a-z_]+)=")
TASK_TYPE = re.compile(r"task_type=([a-z]+)")
TEST_MODULE = re.compile(r"^\s*(pub(\(crate\))? )?mod \w+ \{")


def normalize(text: str) -> tuple[str, list[int]]:
    """Join `\\`-newline continuations and unescape `\\"`, keeping each char's line."""
    out: list[str] = []
    lines: list[int] = []
    line, i = 1, 0
    while i < len(text):
        if text.startswith("\\\n", i):
            i, line = i + 2, line + 1
            while i < len(text) and text[i] in " \t":
                i += 1
            out.append(" ")
            lines.append(line)
            continue
        if text.startswith('\\"', i):
            out.append('"')
            lines.append(line)
            i += 2
            continue
        out.append(text[i])
        lines.append(line)
        if text[i] == "\n":
            line += 1
        i += 1
    return "".join(out), lines


def parameters(text: str, i: int) -> tuple[list[str], int]:
    """Keys of the `key=value` tokens after a call; quoted values may hold spaces."""
    keys = []
    while True:
        while i < len(text) and text[i] in " \t":
            i += 1
        match = KEY.match(text, i)
        if not match:
            return keys, i
        keys.append(match[1])
        i = match.end()
        if i < len(text) and text[i] == '"':
            end = text.find('"', i + 1)
            i = end + 1 if end >= 0 else len(text)
        else:
            while i < len(text) and text[i] not in " \t`\n":
                i += 1


def lint(text: str, origin: str, surface: dict) -> list[tuple[int, str, str]]:
    """(line, defect, call) for every call the surface rejects."""
    tools = "|".join(sorted(surface))
    call = re.compile(r"(mcp__cas__|mcp__cs__|cas__|cas_|\{[a-z_]*\})?(" + tools
                      + r"|\{[a-z_]+\}) action=([A-Za-z_<>{}\-]+)")
    text, lines = normalize(text)
    found = []
    for match in call.finditer(text):
        prefix, tool, action = match.groups()
        if tool.startswith("{"):
            tool = TEMPLATE_TOOL_VARIABLES.get(tool[1:-1])
            if tool is None:
                continue
        # An unprefixed tool name must start a word (`subtask action=` is not a task call).
        before = text[match.start() - 1] if match.start() else ""
        if prefix is None and (before.isascii() and (before.isalnum() or before == "_")):
            continue
        if not re.fullmatch(r"[a-z_]+", action):
            continue  # a placeholder such as action=<a>
        keys, end = parameters(text, match.end())
        shown = text[match.start():max(end, match.end())].split("\n")[0][:160]
        defects = []
        if action not in surface[tool]["actions"]:
            defects.append("unknown action")
        else:
            defects += [f"unknown field {key}" for key in keys if key not in surface[tool]["fields"]]
            if keys:
                for field in REQUIRED_FIELDS.get((tool, action), ()):
                    if field == "dispatch_id" and origin.endswith(VERIFIER_DISPATCH_TEMPLATES):
                        continue
                    if field not in keys:
                        defects.append(f"missing {field}")
                if (tool, action) == ("task", "create") and "risk" not in keys:
                    declared = TASK_TYPE.search(shown)
                    if (declared[1] if declared else "") not in RISK_EXEMPT_TASK_TYPES:
                        defects.append("missing risk")
        found += [(lines[match.start()], f"{tool} action={action}: {defect}", shown) for defect in defects]
    return found


def runtime_text(source: str) -> str:
    """Blank `#[cfg(test)] mod … { … }` blocks and `//` lines, keeping line numbers."""
    lines = source.split("\n")
    kept, i = [], 0
    while i < len(lines):
        if lines[i].strip() == "#[cfg(test)]" and i + 1 < len(lines) and TEST_MODULE.match(lines[i + 1]):
            depth, j = 0, i + 1
            while j < len(lines):
                depth += lines[j].count("{") - lines[j].count("}")
                if depth <= 0 and j > i + 1:
                    break
                j += 1
            kept += [""] * (min(j + 1, len(lines)) - i)
            i = j + 1
            continue
        kept.append("" if lines[i].lstrip().startswith("//") else lines[i])
        i += 1
    return "\n".join(kept)


def sources(root: Path) -> list[tuple[str, str]]:
    """(relative path, linted text) for shipped builtin files and runtime Rust."""
    found = []
    shipped = set()
    for catalog in ("cas-cli/src/builtins.rs", "cas-cli/src/maintenance_jobs.rs"):
        if (root / catalog).is_file():
            shipped |= set(re.findall(r'include_str!\(\s*"(builtins/[^"]+)"', (root / catalog).read_text()))
    for path in sorted(shipped):
        found.append(("cas-cli/src/" + path, (root / "cas-cli/src" / path).read_text()))
    for base in [root / "cas-cli/src", *sorted((root / "crates").glob("*/src"))]:
        for path in sorted(base.rglob("*.rs")):
            relative = path.relative_to(root).as_posix()
            parts = relative.split("/")
            if relative.startswith("cas-cli/src/builtins/") or any(
                    part == "tests" or part.startswith("test") or "_test" in part for part in parts):
                continue
            found.append((relative, runtime_text(path.read_text())))
    return found


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    try:
        surface = json.loads((root / SURFACE).read_text())["tools"]
        offenders = [(origin, *hit) for origin, text in sources(root) for hit in lint(text, origin, surface)]
    except (OSError, ValueError, KeyError) as error:
        print(f"call-shapes: cannot lint: {error}")
        return 2
    for origin, line, defect, call in offenders:
        print(f"{origin}:{line}: {defect} — `{call}`")
        print(f"FIX: {origin}:{line}: {defect}; write the call with an action and fields "
              f"from {SURFACE} (quote values that contain spaces)")
    if not offenders:
        print("call-shapes: every builtin and runtime call matches the MCP surface")
    return 1 if offenders else 0


if __name__ == "__main__":
    sys.exit(main())
