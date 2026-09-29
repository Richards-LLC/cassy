#!/usr/bin/env python3
"""Resolve cas integration source suites to Cargo's explicit test harnesses.

Read only the name/path fields of [[test]] and the harness's direct #[path]
modules. Source files keep their original locations so fixture paths survive.
"""

import argparse
from collections import Counter
from pathlib import Path
import re
import sys


def targets(package):
    manifest = (package / "Cargo.toml").read_text()
    roots = []
    for block in re.findall(r"(?ms)^\[\[test\]\]\s*\n(.*?)(?=^\[|\Z)", manifest):
        name = re.search(r'^name\s*=\s*"([^"]+)"', block, re.M)
        path = re.search(r'^path\s*=\s*"([^"]+)"', block, re.M)
        if not name or not path:
            raise ValueError("explicit test targets must declare name and path")
        roots.append((name[1], (package / path[1]).resolve()))
    return roots


def inventory(package):
    test_dir = (package / "tests").resolve()
    mapping = {}
    sources = []
    for target, root in targets(package):
        if target in mapping:
            raise ValueError(f"duplicate target or suite alias: {target}")
        if not root.is_file():
            raise ValueError(f"missing harness: {root}")
        mapping[target] = target
        if root.parent == test_dir:
            sources.append(root)
            mapping[root.stem] = target
        for relative, module in re.findall(
            r'#\[path\s*=\s*"([^"]+)"\]\s*mod\s+(\w+)\s*;', root.read_text()
        ):
            source = (root.parent / relative).resolve()
            if source.parent != test_dir:
                continue  # a standalone root's private fixture/helper modules
            if not source.is_file():
                raise ValueError(f"missing suite: {source}")
            if module in mapping:
                raise ValueError(f"duplicate suite module: {module}")
            mapping[module] = target
            sources.append(source)
    return mapping, sources


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--count", action="store_true", help="count configured test targets, including Cargo auto-discovery")
    args = parser.parse_args()
    package = args.package.resolve()
    if args.count:
        names = {name for name, _ in targets(package)}
        manifest = (package / "Cargo.toml").read_text()
        if not re.search(r"^autotests\s*=\s*false\s*$", manifest, re.M):
            names.update(path.stem for path in (package / "tests").glob("*.rs"))
            names.update(path.parent.name for path in (package / "tests").glob("*/main.rs"))
        print(len(names))
        return
    mapping, sources = inventory(package)
    if args.check:
        manifest = (package / "Cargo.toml").read_text()
        if not re.search(r"^autotests\s*=\s*false\s*$", manifest, re.M):
            raise ValueError("autotests must be false to prevent per-suite binaries")
        expected = set((package / "tests").glob("*.rs"))
        missing = expected - set(sources)
        duplicates = [path for path, count in Counter(sources).items() if count != 1]
        if missing or duplicates:
            raise ValueError(f"unwired suites: {sorted(missing)}; duplicates: {duplicates}")
        count = len(targets(package))
        if not 1 <= count <= 10:
            raise ValueError(f"expected at most 10 harnesses, found {count}")
        print(f"PASS: {len(sources)} integration source suites in {count} Cargo test harnesses")
    else:
        for suite, target in sorted(mapping.items()):
            print(f"{suite}|{target}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        sys.exit(str(error))
