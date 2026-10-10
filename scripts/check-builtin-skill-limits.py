#!/usr/bin/env python3
"""No-build size and description limits for the shipped builtin skills.

The embedded catalogs in cas-cli/src/builtins.rs are `include_str!` copies of
source files, so these limits are checked on the exact bytes Cassy ships
without compiling. Each limit mirrors a Rust test that otherwise fails only
in the full suite at the release cut (3.50.0: the violet SKILL.md grew past
12 KiB after merge admission). The Rust tests stay authoritative; keep this
table in step with them:

- skill descriptions: present, at most 1,024 chars (harness limit) and at
  most 250 chars (house budget); cas-cli/tests/builtin_skill_description_test.rs
- skills/violet/SKILL.md: at most 12 KiB and 80 lines in every catalog;
  cas-cli/src/builtins.rs test_builtin_violet_skill_body_under_12kb
- skills/cas-cli-craft/SKILL.md: at most 80 lines (claude);
  cas-cli/src/builtins.rs cas-cli-craft catalog test
- root AGENTS.md and CLAUDE.md: under 10,000 chars (Grok's cap);
  cas-cli/tests/builtin_flavor_drift_test.rs

Exit 0 with no findings, 1 with findings (each names the file and limit), 2
when the catalog cannot be read.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys

CATALOGS = {
    "claude": "BUILTIN_SKILLS",
    "codex": "CODEX_BUILTIN_SKILLS",
    "grok": "GROK_BUILTIN_SKILLS",
}
DESCRIPTION_MAX_CHARS = 1024
HOUSE_DESCRIPTION_MAX_CHARS = 250
BODY_LIMITS = {
    # path: (catalogs, max bytes or None, max lines)
    "skills/violet/SKILL.md": (("claude", "codex", "grok"), 12 * 1024, 80),
    "skills/cas-cli-craft/SKILL.md": (("claude",), None, 80),
}
ROOT_DOC_MAX_CHARS = 10_000
ENTRY = re.compile(
    r'BuiltinFile\s*\{\s*path:\s*"([^"]+)"\s*,\s*content:\s*include_str!\(\s*"([^"]+)"\s*\)\s*,?\s*\}',
    re.S,
)


def catalog(source: str, name: str) -> list[tuple[str, str]]:
    """(catalog path, include_str! path) pairs of one `&[BuiltinFile]` const."""
    start = re.search(rf"pub const {name}: &\[BuiltinFile\] = &\[", source)
    if not start:
        raise ValueError(f"cas-cli/src/builtins.rs has no {name} catalog")
    depth, index = 1, start.end()
    while depth and index < len(source):
        depth += {"[": 1, "]": -1}.get(source[index], 0)
        index += 1
    if depth:
        raise ValueError(f"{name} catalog is not closed")
    body = source[start.end():index - 1]
    entries = ENTRY.findall(body)
    if len(entries) != body.count("BuiltinFile {"):
        # A non-`include_str!` entry would otherwise go unchecked.
        raise ValueError(f"{name}: {body.count('BuiltinFile {')} entries, "
                         f"{len(entries)} readable as include_str!")
    return entries


def description(content: str) -> str | None:
    """The single-line `description:` value, as the Rust test reads it."""
    if not content.startswith("---\n"):
        return None
    end = content.find("\n---", 4)
    if end < 0:
        return None
    for line in content[4:end].splitlines():
        if line.startswith("description:"):
            return line[len("description:"):].strip()
    return None


def is_skill_body(path: str) -> bool:
    relative = path.removeprefix("skills/")
    return path.startswith("skills/") and (relative.endswith("/SKILL.md") or "/" not in relative)


def check(repo: Path) -> list[str]:
    source = (repo / "cas-cli/src/builtins.rs").read_text()
    findings = []
    for flavor, name in CATALOGS.items():
        for path, include in catalog(source, name):
            content = (repo / "cas-cli/src" / include).read_text()
            where = f"{flavor}/{path} ({include})"
            if is_skill_body(path):
                value = description(content)
                if not value:
                    findings.append(f"{where}: no description")
                elif len(value) > DESCRIPTION_MAX_CHARS:
                    findings.append(f"{where}: description is {len(value)} chars, over {DESCRIPTION_MAX_CHARS}")
                elif len(value) > HOUSE_DESCRIPTION_MAX_CHARS:
                    findings.append(f"{where}: description is {len(value)} chars, over the "
                                    f"{HOUSE_DESCRIPTION_MAX_CHARS}-char house budget")
            limits = BODY_LIMITS.get(path)
            if limits and flavor in limits[0]:
                _, max_bytes, max_lines = limits
                size, lines = len(content.encode()), len(content.splitlines())
                if max_bytes is not None and size > max_bytes:
                    findings.append(f"{where}: {size} bytes, over the {max_bytes}-byte limit")
                if lines > max_lines:
                    findings.append(f"{where}: {lines} lines, over the {max_lines}-line limit")
    for doc in ("AGENTS.md", "CLAUDE.md"):
        path = repo / doc
        if path.is_file() and len(path.read_text()) >= ROOT_DOC_MAX_CHARS:
            findings.append(f"{doc}: {len(path.read_text())} chars, not under {ROOT_DOC_MAX_CHARS}")
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("repo", nargs="?", default=".")
    args = parser.parse_args()
    try:
        findings = check(Path(args.repo))
    except (OSError, ValueError) as error:
        print(f"builtin-skill-limits: {error}", file=sys.stderr)
        return 2
    for finding in findings:
        print(f"builtin-skill-limits: {finding}")
    print(f"builtin-skill-limits: {len(findings)} finding(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
