#!/usr/bin/env python3
"""Per-task doc fragments: lint them in the lane, compile them at release prep.

Deliveries never edit the shared generated docs (cas-4b33). They add:

- `docs/qa/bug-families/<slug>.yaml`: one journey bug family; the file stem is
  its key in the registry's `bug_families` map.
- `docs/product-overview.d/<task>.md`: `## <Section>` headings that already
  exist in `docs/PRODUCT_OVERVIEW.md`, each followed by the text to append to
  that section.

`needs-upkeep --base <ref>` prints `yes` when the diff from <ref> touches a
registry input or a registry `surfaces` path (or the registry declares no
surfaces), so the fast row knows it must run Cassy's registry and ownership
validator, and `no` otherwise.

`check` is the lane lint (the `journey-catalog` fast row runs it): the
registry parses, journey ids are unique, bug families are well formed with
unique keys across the inline map and the fragment files, their journey and
invariant references resolve, and every overview fragment names existing
sections. `compile` runs `check`, folds every fragment into its shared file,
deletes the fragments and verifies the result; `scripts/release.sh` runs it
once while preparing a release.

Exit 0 when clean or compiled, 1 on problems, 2 on usage errors.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys

import yaml

REGISTRY = "docs/qa/journeys.yaml"
FAMILIES = "docs/qa/bug-families"
OVERVIEW = "docs/PRODUCT_OVERVIEW.md"
OVERVIEW_FRAGMENTS = "docs/product-overview.d"
LIST_FIELDS = ("tasks", "fixed_by", "journeys", "invariants")


class Problems(list):
    def add(self, subject: str, code: str, message: str, fix: str) -> None:
        self.append((subject, code, message, fix))


def family_files(root: Path) -> list[Path]:
    folder = root / FAMILIES
    if not folder.is_dir():
        return []
    return sorted(p for p in folder.iterdir() if p.is_file() and p.suffix in (".yaml", ".yml"))


def overview_files(root: Path) -> list[Path]:
    folder = root / OVERVIEW_FRAGMENTS
    if not folder.is_dir():
        return []
    return sorted(p for p in folder.glob("*.md") if p.name != "README.md")


def load_registry(root: Path, problems: Problems) -> dict | None:
    path = root / REGISTRY
    if not path.is_file():
        return None
    try:
        registry = yaml.safe_load(path.read_text())
    except yaml.YAMLError as error:
        problems.add(REGISTRY, "registry_yaml", f"cannot parse: {error}", f"repair the YAML in {REGISTRY}")
        return None
    if not isinstance(registry, dict):
        problems.add(REGISTRY, "registry_yaml", "top level is not a mapping", f"repair {REGISTRY}")
        return None
    return registry


def load_families(root: Path, registry: dict, problems: Problems) -> dict[str, dict]:
    inline = registry.get("bug_families") or {}
    if not isinstance(inline, dict):
        problems.add(f"{REGISTRY}:bug_families", "bug_family", "bug_families is not a mapping", f"repair {REGISTRY}")
        inline = {}
    families = dict(inline)
    for path in family_files(root):
        rel = path.relative_to(root).as_posix()
        try:
            family = yaml.safe_load(path.read_text())
        except yaml.YAMLError as error:
            problems.add(rel, "bug_family_yaml", f"cannot parse: {error}", f"repair the YAML in {rel}")
            continue
        if path.stem in families:
            problems.add(rel, "duplicate_bug_family", f"key {path.stem} is already defined",
                         f"rename {rel} to an unused slug")
            continue
        families[path.stem] = family
    return families


def check_registry(root: Path, problems: Problems) -> None:
    registry = load_registry(root, problems)
    if registry is None:
        return
    journeys = registry.get("journeys") or []
    ids: set[str] = set()
    for journey in journeys if isinstance(journeys, list) else []:
        jid = journey.get("id") if isinstance(journey, dict) else None
        if not isinstance(jid, str) or not jid:
            problems.add(f"{REGISTRY}:journeys", "journey_id", "a journey has no id", f"give every journey an id in {REGISTRY}")
        elif jid in ids:
            problems.add(f"{REGISTRY}:journeys.{jid}", "duplicate_id", "journey id repeats", f"keep one {jid} entry")
        else:
            ids.add(jid)
    criteria = set(registry.get("invariants") or {}) | set(registry.get("manual_checks") or {})
    for key, family in load_families(root, registry, problems).items():
        subject = f"bug_families.{key}"
        if not isinstance(family, dict) or not isinstance(family.get("title"), str) or not family["title"].strip():
            problems.add(subject, "bug_family", "needs a non-empty title", f"add title: to bug family {key}")
            continue
        for field in LIST_FIELDS:
            if field in family and not isinstance(family[field], list):
                problems.add(subject, "bug_family", f"{field} must be a list", f"write {field}: [..] in bug family {key}")
        for jid in family.get("journeys") or []:
            if jid not in ids:
                problems.add(subject, "unknown_journey", f"journey {jid} is not in {REGISTRY}",
                             f"name an existing journey id in bug family {key}")
        for inv in family.get("invariants") or []:
            if inv not in criteria:
                problems.add(subject, "unknown_invariant", f"invariant {inv} is not in {REGISTRY}",
                             f"name an existing invariant in bug family {key} or add it to {REGISTRY}")


def overview_sections(text: str) -> list[str]:
    return [line[3:].strip() for line in text.splitlines() if line.startswith("## ")]


def parse_overview_fragment(text: str) -> list[tuple[str, list[str]]] | None:
    """[(heading, body lines)], or None when text precedes the first heading."""
    sections: list[tuple[str, list[str]]] = []
    for line in text.splitlines():
        if line.startswith("## "):
            sections.append((line[3:].strip(), []))
        elif sections:
            sections[-1][1].append(line)
        elif line.strip():
            return None
    return [(heading, trim(body)) for heading, body in sections]


def trim(lines: list[str]) -> list[str]:
    while lines and not lines[0].strip():
        lines = lines[1:]
    while lines and not lines[-1].strip():
        lines = lines[:-1]
    return lines


def check_overview(root: Path, problems: Problems) -> None:
    fragments = overview_files(root)
    if not fragments:
        return
    overview = root / OVERVIEW
    if not overview.is_file():
        problems.add(OVERVIEW_FRAGMENTS, "overview_missing", f"{OVERVIEW} does not exist", f"remove the fragments or add {OVERVIEW}")
        return
    known = set(overview_sections(overview.read_text()))
    for path in fragments:
        rel = path.relative_to(root).as_posix()
        sections = parse_overview_fragment(path.read_text())
        if not sections:
            problems.add(rel, "overview_fragment", "needs one or more ## <Section> headings before any text",
                         f"start {rel} with a ## heading from {OVERVIEW}")
            continue
        for heading, body in sections:
            if heading not in known:
                problems.add(rel, "unknown_section", f"section {heading!r} is not a ## heading in {OVERVIEW}",
                             f"use one of: {', '.join(sorted(known))}")
            elif not body:
                problems.add(rel, "overview_fragment", f"section {heading!r} has no text", f"add text under ## {heading} in {rel}")


def check(root: Path) -> Problems:
    problems = Problems()
    check_registry(root, problems)
    check_overview(root, problems)
    return problems


def compile_families(root: Path) -> int:
    files = family_files(root)
    if not files:
        return 0
    path = root / REGISTRY
    text = path.read_text()
    before = yaml.safe_load(text)
    lines = text.split("\n")
    header = next((i for i, line in enumerate(lines) if line.split("#")[0].rstrip() in ("bug_families:", "bug_families: {}")), None)
    if header is None:
        header = next(i for i, line in enumerate(lines) if line.startswith("journeys:"))
        lines.insert(header, "bug_families:")
    lines[header] = "bug_families:"
    end = next((i for i in range(header + 1, len(lines))
                if lines[i] and not lines[i][0].isspace() and not lines[i].startswith("#")), len(lines))
    # Comments directly above the next key belong to that key.
    while end > header + 1 and (not lines[end - 1].strip() or lines[end - 1].startswith("#")):
        end -= 1
    added: list[str] = []
    expected = dict(before.get("bug_families") or {})
    for family in files:
        expected[family.stem] = yaml.safe_load(family.read_text())
        added.append(f"  {family.stem}:")
        added.extend(f"    {line}" if line.strip() else "" for line in trim(family.read_text().split("\n")))
    lines[end:end] = added
    compiled = "\n".join(lines)
    after = yaml.safe_load(compiled)
    if after.get("bug_families") != expected or {k: v for k, v in after.items() if k != "bug_families"} != {
        k: v for k, v in before.items() if k != "bug_families"
    }:
        raise SystemExit(f"error: compiling {FAMILIES} into {REGISTRY} changed other registry content; nothing written")
    path.write_text(compiled)
    for family in files:
        family.unlink()
    return len(files)


def compile_overview(root: Path) -> int:
    files = overview_files(root)
    if not files:
        return 0
    path = root / OVERVIEW
    lines = path.read_text().split("\n")
    for fragment in files:
        for heading, body in parse_overview_fragment(fragment.read_text()) or []:
            start = next(i for i, line in enumerate(lines) if line.startswith("## ") and line[3:].strip() == heading)
            end = next((i for i in range(start + 1, len(lines))
                        if lines[i].startswith(("# ", "## ", "<!-- cas:"))), len(lines))
            while end > start + 1 and not lines[end - 1].strip():
                end -= 1
            lines[end:end] = ["", *body] + ([""] if end < len(lines) and lines[end].strip() else [])
    path.write_text("\n".join(lines))
    for fragment in files:
        fragment.unlink()
    return len(files)


def glob_regex(pattern: str) -> re.Pattern:
    out, i = "", 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out, i = out + "(?:.*/)?", i + 3
        elif pattern.startswith("**", i):
            out, i = out + ".*", i + 2
        elif pattern[i] == "*":
            out, i = out + "[^/]*", i + 1
        elif pattern[i] == "?":
            out, i = out + "[^/]", i + 1
        else:
            out, i = out + re.escape(pattern[i]), i + 1
    return re.compile(out + r"\Z")


def needs_upkeep(root: Path, base: str) -> bool:
    """Whether the diff from `base` touches the registry or a user-facing surface."""
    if not (root / REGISTRY).is_file():
        return False
    changed = subprocess.run(["git", "diff", "--name-only", "--no-renames", base, "--"], cwd=root,
                             check=True, capture_output=True, text=True).stdout.split()
    if any(path == REGISTRY or path.startswith(FAMILIES + "/") for path in changed):
        return True
    try:
        surfaces = (yaml.safe_load((root / REGISTRY).read_text()) or {}).get("surfaces")
    except yaml.YAMLError:
        return True
    if surfaces is None:  # inherits qa.user_facing_paths: assume any path may be a surface
        return bool(changed)
    patterns = [glob_regex(glob) for glob in surfaces if isinstance(glob, str)]
    return any(pattern.match(path) for path in changed for pattern in patterns)


def report(problems: Problems) -> None:
    for subject, code, message, fix in problems:
        print(f"ERROR {subject} [{code}]: {message}")
        print(f"FIX: {fix}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("check", "compile", "needs-upkeep"))
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--base", help="needs-upkeep: diff base")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    if args.command == "needs-upkeep":
        if not args.base:
            parser.error("needs-upkeep requires --base")
        print("yes" if needs_upkeep(root, args.base) else "no")
        return 0
    problems = check(root)
    if problems:
        report(problems)
        return 1
    if args.command == "check":
        print(f"doc-fragments: PASS ({len(family_files(root))} bug family, {len(overview_files(root))} overview fragment(s))")
        return 0
    families = compile_families(root)
    overview = compile_overview(root)
    print(f"doc-fragments: compiled {families} bug family and {overview} overview fragment(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
