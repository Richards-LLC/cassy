#!/usr/bin/env python3
"""Apply the shipped-catalog operator-data policy directly to builtin sources."""

import json
from pathlib import Path
import re
import sys


def check(root, policy_path):
    policy = json.loads(policy_path.read_text())
    rules = [(rule["name"], re.compile(rule["pattern"])) for rule in policy["rules"]]
    synthetic = {name: re.compile(pattern) for name, pattern in policy["synthetic"].items()}
    allow = {(entry["path"], entry["rule"]) for entry in policy["allowlist"]}
    if not root.is_dir():
        raise ValueError(f"missing builtin source directory: {root}")
    failures = 0
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        # Rust catalogs ship agents, skills (including their assets) and workflows;
        # implementation source and generated reference history are not documents.
        if not path.is_file() or not any(part in relative.split("/") for part in ("agents", "skills", "workflows")):
            continue
        canonical = re.sub(r"^(codex|grok)/", "", relative)
        try:
            text = path.read_text()
        except UnicodeDecodeError:
            continue
        for name, pattern in rules:
            if (canonical, name) in allow:
                continue
            for hit in pattern.finditer(text):
                if name in synthetic and synthetic[name].search(hit.group()):
                    continue
                line = text.count("\n", 0, hit.start()) + 1
                print(f"{path}:{line}: {name}: {hit.group()}", file=sys.stderr)
                failures += 1
    print(f"builtin-doc-hygiene: {failures} operator-data violation(s)")
    return int(bool(failures))


if __name__ == "__main__":
    try:
        repo = Path.cwd()
        sys.exit(check(repo / "cas-cli/src/builtins", Path(__file__).with_name("builtin-doc-hygiene.json")))
    except (ValueError, OSError, KeyError) as error:
        print(f"builtin-doc-hygiene: {error}", file=sys.stderr)
        sys.exit(1)
