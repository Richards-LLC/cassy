#!/usr/bin/env python3
"""No-build consistency of the schema migration registry.

Every cas-cli/src/migration/migrations/mNNN_<name>.rs must be declared
(`mod mNNN_<name>;`) and registered (`mNNN_<name>::MIGRATION,`) exactly once in
mod.rs, declare `id: NNN`, and the registry must list ids in strictly
increasing order. A miss otherwise surfaces only when the full suite or a
fresh store runs migrations at the release cut.

Exit 0 when consistent, 1 with findings, 2 when the registry cannot be read.
"""

from __future__ import annotations

import argparse
from collections import Counter
from pathlib import Path
import re
import sys

DIRECTORY = "cas-cli/src/migration/migrations"


def check(repo: Path) -> list[str]:
    root = repo / DIRECTORY
    registry = (root / "mod.rs").read_text()
    files = sorted(path.stem for path in root.glob("m[0-9]*_*.rs"))
    declared = re.findall(r"^\s*(?:pub\s+)?mod\s+(m[0-9]+_\w+)\s*;", registry, re.M)
    listed = re.findall(r"^\s*(m[0-9]+_\w+)::MIGRATION\s*,", registry, re.M)
    findings = []
    for kind, names in (("declared", declared), ("registered", listed)):
        for name, count in Counter(names).items():
            if count > 1:
                findings.append(f"{name} is {kind} {count} times in mod.rs")
    for name in files:
        if name not in declared:
            findings.append(f"{name}.rs has no `mod {name};` in mod.rs")
        if name not in listed:
            findings.append(f"{name}.rs is not registered as `{name}::MIGRATION` in MIGRATIONS")
    for name in set(declared) | set(listed):
        if name not in files:
            findings.append(f"mod.rs names {name}, but {DIRECTORY}/{name}.rs does not exist")
    previous = 0
    for name in listed:
        number = int(re.match(r"m([0-9]+)_", name).group(1))
        path = root / f"{name}.rs"
        if path.is_file():
            declared_id = re.search(r"^\s*id:\s*([0-9]+)\s*,", path.read_text(), re.M)
            if not declared_id:
                findings.append(f"{name}.rs declares no `id:`")
            elif int(declared_id.group(1)) != number:
                findings.append(f"{name}.rs declares id {declared_id.group(1)}, not {number}")
        if number <= previous:
            findings.append(f"MIGRATIONS lists {name} after id {previous}; ids must strictly increase")
        previous = max(previous, number)
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("repo", nargs="?", default=".")
    args = parser.parse_args()
    try:
        findings = check(Path(args.repo))
    except OSError as error:
        print(f"migration-registry: {error}", file=sys.stderr)
        return 2
    for finding in findings:
        print(f"migration-registry: {finding}")
    print(f"migration-registry: {len(findings)} finding(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
