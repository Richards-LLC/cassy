#!/usr/bin/env python3
"""Cassy's git merge drivers (cas-0c988, cas-7aa5).

    cas-merge-drivers.py install [<repo>]
        Register the drivers for every merge in the repository: git config
        `merge.cas-generated.*` and `merge.cas-changelog.*`, plus matching
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

`merge=cas-generated` (hub-web/dist) keeps ours with no conflict; the merge
then rebuilds it with scripts/regenerate-generated-artifacts.sh.
"""

import os
import shutil
import subprocess
import sys
from pathlib import Path

ATTRIBUTES = (
    "CHANGELOG.md merge=cas-changelog",
    "hub-web/dist/** merge=cas-generated",
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


def main(argv):
    if len(argv) >= 2 and argv[1] == "install":
        return install(argv[2] if len(argv) > 2 else os.getcwd())
    if len(argv) >= 5 and argv[1] == "changelog":
        marker = int(argv[5]) if len(argv) > 5 and argv[5].isdigit() else 7
        return changelog(argv[2], argv[3], argv[4], marker)
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
