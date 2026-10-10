#!/usr/bin/env python3
"""No-build consistency between `cas doctor` source and its accepted snapshot.

The doctor snapshot (cas-cli/tests/snapshots/component_output_test__doctor_snapshot.snap)
is only compared when the component-output test runs in the full suite, so a
new doctor row reached the 3.50.0 cut before failing there. Two checks run
without a build:

1. Grouping: every row in the snapshot sits under the group that
   `CheckGroup::for_name` in cas-cli/src/cli/doctor.rs assigns to its name
   (an unlisted name falls back to Store).
2. With --base: a lane that adds a `recorder.mark("<phase>")` to doctor.rs
   adds a doctor check phase, which changes the default run's rows. It must
   also update the snapshot. A mark that adds no default-run row says so on
   its line with the comment `doctor-snapshot: no default row`.

Exit 0 when consistent, 1 with findings, 2 when the inputs cannot be read.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys

DOCTOR = "cas-cli/src/cli/doctor.rs"
SNAPSHOT = "cas-cli/tests/snapshots/component_output_test__doctor_snapshot.snap"
GROUPS = ("Host", "Store", "Indexes", "Cloud", "Config", "Integrations")
MARK = re.compile(r'recorder\.mark\("([^"]+)"')
WAIVER = "doctor-snapshot: no default row"


def group_rules(source: str) -> tuple[dict[str, str], list[tuple[str, str]]]:
    """Exact names and `name.starts_with(..)` prefixes from `for_name`."""
    start = source.find("fn for_name(name: &str) -> Self {")
    if start < 0:
        raise ValueError(f"{DOCTOR}: CheckGroup::for_name not found")
    end = source.find("_ => Self::", start)
    if end < 0:
        raise ValueError(f"{DOCTOR}: CheckGroup::for_name has no fallback arm")
    body = source[start:end]
    exact: dict[str, str] = {}
    prefixes: list[tuple[str, str]] = []
    for arm, group in re.findall(r"((?:\s*\|?\s*\"[^\"]+\")+)\s*=>\s*Self::(\w+)", body):
        for name in re.findall(r'"([^"]+)"', arm):
            exact[name] = group
    for prefix, group in re.findall(r'name if name\.starts_with\("([^"]+)"\)\s*=>\s*Self::(\w+)', body):
        prefixes.append((prefix, group))
    if not exact:
        raise ValueError(f"{DOCTOR}: CheckGroup::for_name has no name arms")
    return exact, prefixes


def group_for(name: str, exact: dict[str, str], prefixes: list[tuple[str, str]]) -> str:
    lowered = name.lower()
    if lowered in exact:
        return exact[lowered]
    for prefix, group in prefixes:
        if lowered.startswith(prefix):
            return group
    return "Store"


def snapshot_rows(text: str) -> list[tuple[str, str]]:
    """(group, row name) for every row in the rendered snapshot."""
    rows = []
    group = None
    for line in text.splitlines():
        head = re.match(r"^(" + "|".join(GROUPS) + r")\s{2,}(.*)$", line)
        if head:
            group = head.group(1)
            for name in re.findall(r"\[(?:OK|WARN|ERROR|FAIL|INFO)\] (.+?)(?=\s{2,}\[|$)", head.group(2)):
                rows.append((group, name.strip()))
            continue
        detail = re.match(r"^\s{2,}\[(?:OK|WARN|ERROR|FAIL|INFO)\] (.+?)(?:\s{2,}|$)", line)
        if detail and group:
            rows.append((group, detail.group(1).strip()))
        elif line and not line.startswith(" "):
            group = None
    return rows


def added_marks(repo: Path, base: str) -> list[str]:
    diff = subprocess.run(["git", "-C", str(repo), "diff", "--unified=0", base, "--", DOCTOR],
                          capture_output=True, text=True)
    if diff.returncode:
        raise ValueError(f"cannot diff {DOCTOR} against {base}: {diff.stderr.strip()}")
    removed = set()
    added = []
    for line in diff.stdout.splitlines():
        if line.startswith("-") and not line.startswith("---"):
            removed.update(MARK.findall(line))
        elif line.startswith("+") and not line.startswith("+++") and WAIVER not in line:
            added.extend(MARK.findall(line))
    return [name for name in added if name not in removed]


def snapshot_changed(repo: Path, base: str) -> bool:
    return subprocess.run(["git", "-C", str(repo), "diff", "--quiet", base, "--", SNAPSHOT]).returncode != 0


def check(repo: Path, base: str | None) -> list[str]:
    exact, prefixes = group_rules((repo / DOCTOR).read_text())
    findings = []
    for group, name in snapshot_rows((repo / SNAPSHOT).read_text()):
        expected = group_for(name, exact, prefixes)
        if expected != group:
            findings.append(f"snapshot row '{name}' is under {group}, but CheckGroup::for_name "
                            f"puts it under {expected}")
    if base:
        new = added_marks(repo, base)
        if new and not snapshot_changed(repo, base):
            findings.append(
                f"{DOCTOR} adds doctor phase(s) {', '.join(repr(n) for n in new)} but {SNAPSHOT} "
                f"is unchanged; run `cargo nextest run -p cas --test component_output_test` and accept "
                f"the snapshot, or mark a phase with no default-run row `// {WAIVER}`")
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("repo", nargs="?", default=".")
    parser.add_argument("--base", help="lane base: also require a snapshot update for new doctor phases")
    args = parser.parse_args()
    try:
        findings = check(Path(args.repo), args.base)
    except (OSError, ValueError) as error:
        print(f"doctor-snapshot: {error}", file=sys.stderr)
        return 2
    for finding in findings:
        print(f"doctor-snapshot: {finding}")
    print(f"doctor-snapshot: {len(findings)} finding(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
