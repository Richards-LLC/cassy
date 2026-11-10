#!/usr/bin/env python3
"""Cassy's git merge drivers (cas-0c988, cas-7aa5).

    cas-merge-drivers.py install [<repo>]
        Register the drivers for every merge in the repository: git config
        `merge.cas-generated.*`, `merge.cas-changelog.*`, and `merge.cas-journeys.*`, plus matching
        lines in `$GIT_COMMON_DIR/info/attributes`. `info/attributes` applies
        to every merge in the clone, including `git merge-tree --write-tree`,
        whatever `.gitattributes` the checked-out tree has. That matters while
        the attribute is still arriving on a branch. Idempotent.

    cas-merge-drivers.py changelog <base> <ours> <theirs> [<marker-size>]
        Merge driver for CHANGELOG.md (`%O %A %B %L`). It runs an ordinary
        three-way merge. Conflicts inside `## [Unreleased]` are parallel
        additions (two epics each adding a `###` section), so both sides are
        kept, ours first. A conflict anywhere else (an edit to a released
        section, or a hunk that carries a `## ` heading, such as a release
        cut) stays a conflict.

    cas-merge-drivers.py journeys <base> <ours> <theirs> [<marker-size>]
        Union independent bug_families entries and take the newest ISO updated
        date. Competing edits to one family stay conflicts. Other fields use
        Git's ordinary merge; unsupported YAML layouts fall back to it too.

`merge=cas-generated` (hub-web/dist and the builtin reference-history ledger)
keeps ours with no conflict; the merge then rebuilds both with
scripts/regenerate-generated-artifacts.sh.
"""

import datetime
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ATTRIBUTES = (
    "CHANGELOG.md merge=cas-changelog",
    "docs/qa/journeys.yaml merge=cas-journeys",
    "hub-web/dist/** merge=cas-generated",
    "cas-cli/src/builtins/reference-history.json merge=cas-generated",
)


def git(repo, *args, check=True):
    result = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True)
    if check and result.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def install(repo):
    common = Path(git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    # The driver runs from a stable copy in the common dir, so it works in any
    # worktree, in merge-tree, and on a tree that does not carry this script.
    home = common / "cas"
    home.mkdir(parents=True, exist_ok=True)
    driver = home / "cas-merge-drivers.py"
    source = Path(__file__).resolve()
    if source != driver.resolve():
        shutil.copyfile(source, driver)
    for key, value in (
        ("merge.cas-journeys.name", "journey registry: merge independent families and newest updated date"),
        ("merge.cas-journeys.driver", f"python3 '{driver}' journeys %O %A %B %L"),
        ("merge.cas-generated.name", "generated build output: keep ours, regenerate after the merge"),
        ("merge.cas-generated.driver", "true"),
        ("merge.cas-changelog.name", "CHANGELOG: union [Unreleased] additions, conflict elsewhere"),
        ("merge.cas-changelog.driver", f"python3 '{driver}' changelog %O %A %B %L"),
    ):
        git(repo, "config", key, value)
    info = common / "info"
    info.mkdir(parents=True, exist_ok=True)
    attributes = info / "attributes"
    lines = attributes.read_text().splitlines() if attributes.exists() else []
    missing = [line for line in ATTRIBUTES if line not in lines]
    if missing:
        with attributes.open("a") as handle:
            if lines and lines[-1].strip():
                handle.write("\n")
            handle.write("# Cassy merge drivers (scripts/cas-merge-drivers.py install)\n")
            for line in missing:
                handle.write(line + "\n")
    return 0


def is_unreleased(heading):
    return heading is not None and heading.strip().lower().startswith("## [unreleased]")


def resolve_unreleased(text, marker):
    """Union conflict hunks inside `## [Unreleased]`; return (text, unresolved)."""
    start, middle, end = "<" * marker, "=" * marker, ">" * marker
    lines = text.splitlines(keepends=True)
    out, heading, unresolved, index = [], None, 0, 0
    while index < len(lines):
        line = lines[index]
        if line.startswith(start):
            cursor = index + 1
            ours, theirs = [], []
            while cursor < len(lines) and not lines[cursor].startswith(middle):
                ours.append(lines[cursor])
                cursor += 1
            cursor += 1
            while cursor < len(lines) and not lines[cursor].startswith(end):
                theirs.append(lines[cursor])
                cursor += 1
            hunk_end = cursor
            carries_heading = any(part.startswith("## ") for part in ours + theirs)
            if is_unreleased(heading) and not carries_heading:
                out.extend(ours)
                if ours and theirs and ours[-1].strip() and theirs[0].strip():
                    out.append("\n")
                out.extend(theirs)
            else:
                out.extend(lines[index:hunk_end + 1])
                unresolved += 1
            index = hunk_end + 1
            continue
        if line.startswith("## "):
            heading = line
        out.append(line)
        index += 1
    return "".join(out), unresolved


def changelog(base, ours, theirs, marker=7):
    result = subprocess.run(
        ["git", "merge-file", "-p", f"--marker-size={marker}",
         "-L", "ours", "-L", "base", "-L", "theirs", ours, base, theirs],
        capture_output=True,
    )
    if result.returncode < 0 or result.returncode > 127:
        return 2
    text = result.stdout.decode("utf-8", "surrogateescape")
    unresolved = 0
    if result.returncode > 0:
        text, unresolved = resolve_unreleased(text, marker)
    with open(ours, "w", encoding="utf-8", errors="surrogateescape") as handle:
        handle.write(text)
    return 1 if unresolved else 0


def registry_parts(text):
    """Split the registry's block map without reserializing YAML or its comments.

    Only the repository's simple, two-space family keys are handled. Unusual
    YAML (flow maps, anchors, aliases, duplicate keys) uses Git's normal merge.
    """
    lines = text.splitlines(keepends=True)
    starts = [i for i, line in enumerate(lines) if line == "bug_families:\n"]
    if len(starts) != 1 or re.search(r"(?:^|[\s\[,{])(?:[&*][\w-]+|<<:)", text):
        raise ValueError("unsupported registry map")
    start = starts[0] + 1
    end = start
    families = {}
    key = None
    while end < len(lines):
        line = lines[end]
        if line.strip() and not line.startswith(" "):
            break
        match = re.fullmatch(r"  ([A-Za-z0-9_-]+):[ \t]*\n", line)
        if match:
            key = match[1]
            if key in families:
                raise ValueError("duplicate family")
            families[key] = ""
        elif line.strip() and not line.startswith(("    ", "  #")):
            raise ValueError("unsupported family key")
        if key is None:
            if line.strip():
                raise ValueError("missing family key")
        else:
            families[key] += line
        end += 1
    # Preserve the rest of the document for Git's ordinary three-way merge.
    return "".join(lines[:start] + lines[end:]), families


def journeys(base, ours, theirs, marker=7):
    paths = [base, ours, theirs]
    originals = [Path(path).read_text() for path in paths]
    try:
        parts = [registry_parts(text) for text in originals]
        dates = []
        for text, _ in parts:
            matches = re.findall(r"(?m)^updated: ['\"]?(\d{4}-\d{2}-\d{2})['\"]?[ \t]*$", text)
            if len(matches) != 1:
                raise ValueError("unsupported updated date")
            if len(re.findall(r"(?m)^updated:", text)) != 1:
                raise ValueError("duplicate updated field")
            dates.append(datetime.date.fromisoformat(matches[0]))
    except ValueError:
        return ordinary_merge(base, ours, theirs, marker)

    merged = []
    conflict = False
    maps = [families for _, families in parts]
    # Ours-first order retains existing layout and appends incoming-only keys.
    for key in dict.fromkeys([*maps[1], *maps[2], *maps[0]]):
        ancestor, left, right = (families.get(key, "") for families in maps)
        if left == right or right == ancestor:
            merged.append(left)
        elif left == ancestor:
            merged.append(right)
        else:
            # A family is the conflict unit, including edit/delete conflicts.
            conflict = True
            merged.append(f"{'<' * marker} ours\n{left}{'=' * marker}\n{right}{'>' * marker} theirs\n")

    newest = max(dates).isoformat()
    with tempfile.TemporaryDirectory() as temporary:
        inputs = []
        for i, (text, _) in enumerate(parts):
            path = Path(temporary) / str(i)
            path.write_text(re.sub(r"(?m)^updated:.*$", f"updated: '{newest}'", text))
            inputs.append(str(path))
        result = ordinary_merge(*inputs, marker)
        text = Path(inputs[1]).read_text()
    # Keep conflicts in policy, journeys, and all other fields unresolved.
    text = text.replace("bug_families:\n", "bug_families:\n" + "".join(merged), 1)
    Path(ours).write_text(text)
    return result or int(conflict)


def ordinary_merge(base, ours, theirs, marker=7):
    result = subprocess.run(
        ["git", "merge-file", f"--marker-size={marker}",
         "-L", "ours", "-L", "base", "-L", "theirs", ours, base, theirs],
        capture_output=True,
    )
    return 0 if result.returncode == 0 else 1

def main(argv):
    if len(argv) >= 2 and argv[1] == "install":
        return install(argv[2] if len(argv) > 2 else os.getcwd())
    if len(argv) >= 5 and argv[1] in ("changelog", "journeys"):
        marker = int(argv[5]) if len(argv) > 5 and argv[5].isdigit() else 7
        driver = changelog if argv[1] == "changelog" else journeys
        return driver(argv[2], argv[3], argv[4], marker)
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
