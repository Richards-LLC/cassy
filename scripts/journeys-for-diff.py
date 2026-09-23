#!/usr/bin/env python3
"""Map changed paths to the user journeys they touch (docs/qa/journeys.md).

Usage:
  journeys-for-diff.py <base-ref> [<head-ref>]   paths changed base..head (default HEAD)
  journeys-for-diff.py --paths <path>...         explicit repo-relative paths
  journeys-for-diff.py --all                     every catalog journey
  journeys-for-diff.py --check                   validate the catalog; exit 1 on errors

Prints JSON: {"catalog": ..., "journeys": [{"id", "title", "surface", "suite", "reason"}]}.
A path matching a surface's **Surface-wide** globs selects every journey of
that surface. No match is an empty list and exit 0.

The catalog contract is described in docs/qa/journey-evaluation.md.
"""

from __future__ import annotations

import fnmatch
import json
import os
import re
import subprocess
import sys
from pathlib import Path

CATALOG = "docs/qa/journeys.md"
REQUIRED = ("Entry", "Goal", "Touches", "Suite", "Gaps")
BLOCKS = ("Steps", "Expected experience", "Edge paths")
ID_RE = re.compile(r"^[A-Z]+-J[0-9]+$")


def repo_root() -> Path:
    override = os.environ.get("CAS_JOURNEYS_ROOT")
    if override:
        return Path(override)
    out = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True)
    return Path(out.stdout.strip())


def globs(value: str) -> list[str]:
    return re.findall(r"`([^`]+)`", value)


def parse(text: str) -> tuple[dict[str, list[str]], list[dict]]:
    """Return ({surface: surface-wide globs}, [journey])."""
    surfaces: dict[str, list[str]] = {}
    journeys: list[dict] = []
    surface = None
    current = None
    block = None
    for line in text.splitlines():
        if line.startswith("## "):
            surface = line[3:].strip()
            surfaces.setdefault(surface, [])
            current, block = None, None
            continue
        if line.startswith("### "):
            head = line[4:].strip()
            ident, _, title = head.partition(" · ")
            current = {"id": ident.strip(), "title": title.strip(), "surface": surface, "fields": {}, "blocks": {}}
            journeys.append(current)
            block = None
            continue
        field = re.match(r"^- \*\*([A-Za-z -]+):\*\*\s*(.*)$", line)
        if field and current is None and surface is not None:
            if field.group(1) == "Surface-wide":
                surfaces[surface] = globs(field.group(2))
            continue
        if current is None:
            continue
        if field and block is None:
            current["fields"][field.group(1)] = field.group(2).strip()
            continue
        heading = re.match(r"^\*\*([A-Za-z ]+)\*\*\s*$", line)
        if heading:
            block = heading.group(1)
            current["blocks"][block] = []
            continue
        item = re.match(r"^\s*(?:[0-9]+\.|-)\s+(.*)$", line)
        if item and block is not None:
            current["blocks"][block].append(item.group(1).strip())
    return surfaces, journeys


def suite_path(journey: dict) -> str | None:
    found = globs(journey["fields"].get("Suite", ""))
    return found[0] if found else None


def check(root: Path, surfaces: dict[str, list[str]], journeys: list[dict]) -> list[str]:
    errors: list[str] = []
    seen: set[str] = set()
    for surface, wide in surfaces.items():
        if not wide:
            errors.append(f"surface {surface}: missing **Surface-wide:** globs")
    for journey in journeys:
        ident = journey["id"]
        where = f"{ident or '<no id>'}"
        if not ID_RE.match(ident):
            errors.append(f"{where}: id must look like HUB-J1")
        if ident in seen:
            errors.append(f"{where}: duplicate id")
        seen.add(ident)
        if not journey["title"]:
            errors.append(f"{where}: heading must be '### <ID> · <title>'")
        for name in REQUIRED:
            if not journey["fields"].get(name):
                errors.append(f"{where}: missing **{name}:**")
        if not globs(journey["fields"].get("Touches", "")):
            errors.append(f"{where}: **Touches:** needs at least one `glob`")
        for name in BLOCKS:
            if not journey["blocks"].get(name):
                errors.append(f"{where}: missing non-empty **{name}** list")
        suite = journey["fields"].get("Suite", "")
        spec = suite_path(journey)
        if spec is None:
            if not suite.startswith("not automated"):
                errors.append(f"{where}: **Suite:** must be a `spec path` or 'not automated — <reason>'")
            continue
        spec_file = root / spec
        if not spec_file.is_file():
            errors.append(f"{where}: suite {spec} does not exist")
            continue
        body = spec_file.read_text()
        if not re.search(rf"""['"`]{re.escape(ident)}\b""", body):
            errors.append(f"{where}: {spec} has no test titled with {ident}")
        for step in journey["blocks"].get("Steps", []):
            title = step.split(" — ")[0].strip()
            if title and title not in body:
                errors.append(f"{where}: step '{title}' has no matching test.step in {spec}")
    return errors


def changed_paths(root: Path, base: str, head: str) -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(root), "diff", "--name-only", f"{base}...{head}"],
        capture_output=True, text=True,
    )
    if out.returncode != 0:
        sys.stderr.write(out.stderr)
        raise SystemExit(2)
    return [line for line in out.stdout.splitlines() if line]


def select(paths: list[str], surfaces: dict[str, list[str]], journeys: list[dict]) -> list[dict]:
    chosen: dict[str, dict] = {}
    for path in paths:
        for surface, wide in surfaces.items():
            if any(fnmatch.fnmatchcase(path, pattern) for pattern in wide):
                for journey in journeys:
                    if journey["surface"] == surface and journey["id"] not in chosen:
                        chosen[journey["id"]] = row(journey, f"surface-wide:{path}")
        for journey in journeys:
            if journey["id"] in chosen:
                continue
            if path == suite_path(journey):
                chosen[journey["id"]] = row(journey, "suite")
                continue
            for pattern in globs(journey["fields"].get("Touches", "")):
                if fnmatch.fnmatchcase(path, pattern):
                    chosen[journey["id"]] = row(journey, pattern)
                    break
    order = [j["id"] for j in journeys]
    return sorted(chosen.values(), key=lambda r: order.index(r["id"]))


def row(journey: dict, reason: str) -> dict:
    return {
        "id": journey["id"],
        "title": journey["title"],
        "surface": journey["surface"],
        "suite": suite_path(journey),
        "reason": reason,
    }


def main(argv: list[str]) -> int:
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__.strip())
        return 0 if argv else 2
    root = repo_root()
    catalog = root / CATALOG
    if not catalog.is_file():
        sys.stderr.write(f"journeys: no catalog at {catalog}\n")
        return 2
    surfaces, journeys = parse(catalog.read_text())
    if argv[0] == "--check":
        errors = check(root, surfaces, journeys)
        for error in errors:
            print(f"journeys: {error}", file=sys.stderr)
        if errors:
            return 1
        print(f"journeys: catalog OK ({len(journeys)} journeys)")
        return 0
    if argv[0] == "--all":
        selected = [row(j, "all") for j in journeys]
    elif argv[0] == "--paths":
        selected = select(argv[1:], surfaces, journeys)
    else:
        selected = select(changed_paths(root, argv[0], argv[1] if len(argv) > 1 else "HEAD"), surfaces, journeys)
    print(json.dumps({"catalog": CATALOG, "journeys": selected}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
