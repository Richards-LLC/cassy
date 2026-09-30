#!/usr/bin/env python3
"""Check reasoned builtin text contracts without compiling Rust.

The embedded-catalog Rust test checks the same JSON against installed catalog
contents; this entry point checks canonical source documents for cheap feedback.
"""
import argparse
import json
from pathlib import Path
import sys


def contracts(policy):
    if set(policy) != {"version", "documents"} or type(policy["version"]) is not int or policy["version"] != 1:
        raise ValueError("expected version 1 and documents")
    documents = policy["documents"]
    if not isinstance(documents, dict) or not documents:
        raise ValueError("documents must be a nonempty object")
    for name, document in documents.items():
        if not name or set(document) != {"source", "contains", "absent", "any_of"}:
            raise ValueError(f"{name}: expected source, contains, absent and any_of")
        source = Path(document["source"])
        if source.is_absolute() or ".." in source.parts or not document["source"]:
            raise ValueError(f"{name}: source must be a checkout-relative path")
        rules = []
        for kind in ("contains", "absent", "any_of"):
            entries = document[kind]
            if not isinstance(entries, list):
                raise ValueError(f"{name}: {kind} must be an array")
            for entry in entries:
                if not isinstance(entry, dict) or not isinstance(entry.get("reason"), str) or not entry["reason"].strip():
                    raise ValueError(f"{name}: every rule needs a reason")
                field = "texts" if kind == "any_of" else "text"
                if set(entry) != {field, "reason"}:
                    raise ValueError(f"{name}: invalid {kind} fields")
                values = entry[field] if kind == "any_of" else [entry[field]]
                if not isinstance(values, list) or not values or any(not isinstance(v, str) or not v for v in values):
                    raise ValueError(f"{name}: phrases must be nonempty strings")
                rules.append((kind, values, entry["reason"]))
        if not rules:
            raise ValueError(f"{name}: empty contract")
        yield name, document["source"], rules


def violations(text, rules):
    failures = []
    for kind, phrases, reason in rules:
        present = [phrase in text for phrase in phrases]
        passes = all(present) if kind == "contains" else not any(present) if kind == "absent" else any(present)
        if not passes:
            failures.append(f"{kind} {phrases!r}: {reason}")
    return failures


def check(root, policy):
    failures = []
    count = 0
    for name, source, rules in contracts(policy):
        text = (root / source).read_text()
        count += len(rules)
        failures.extend(f"{name}: {failure}" for failure in violations(text, rules))
    return count, failures


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--policy", type=Path, default=Path(__file__).with_name("builtin-contract-phrases.json"))
    args = parser.parse_args()
    try:
        count, failures = check(args.root, json.loads(args.policy.read_text()))
        for failure in failures:
            print(failure, file=sys.stderr)
        print(f"builtin-contract-phrases: {count} contract(s), {len(failures)} violation(s)")
        sys.exit(int(bool(failures)))
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f"builtin-contract-phrases: {error}", file=sys.stderr)
        sys.exit(1)
