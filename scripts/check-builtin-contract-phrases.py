#!/usr/bin/env python3
"""Check the reasoned text registry against canonical sources without a build.

The embedded-catalog test reads the same policy and schema fixtures. Rules bind
only contract tokens; absent rules forbid unsafe/retired instructions. Alternatives
retain OR semantics, including contracts that may live in either of two documents.
"""
import argparse
import json
from pathlib import Path
import sys


def nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def relative(value):
    return nonempty(value) and not value.startswith("/") and ".." not in value.split("/") and "\\" not in value


def fields(value, required, optional=()):
    return isinstance(value, dict) and set(required) <= set(value) <= set(required) | set(optional)


def validate(policy):
    if not fields(policy, ("version", "documents", "alternatives")) or type(policy["version"]) is not int or policy["version"] != 2:
        raise ValueError("expected version 2, documents and alternatives")
    documents = policy["documents"]
    if not isinstance(documents, dict) or not documents:
        raise ValueError("documents must be a nonempty object")
    alternatives = policy["alternatives"]
    if not isinstance(alternatives, list):
        raise ValueError("alternatives must be an array")
    referenced = set()
    for entry in alternatives:
        if not fields(entry, ("reason", "choices")) or not nonempty(entry["reason"]):
            raise ValueError("every alternative needs choices and a reason")
        if not isinstance(entry["choices"], list) or not entry["choices"]:
            raise ValueError("alternative choices must be nonempty")
        for choice in entry["choices"]:
            if not fields(choice, ("document", "text", "absent", "case_sensitive")):
                raise ValueError("invalid alternative choice fields")
            if not nonempty(choice["text"]) or not isinstance(choice["document"], str) or choice["document"] not in documents:
                raise ValueError("alternative needs a known document and nonempty phrase")
            if type(choice["absent"]) is not bool or type(choice["case_sensitive"]) is not bool:
                raise ValueError("alternative flags must be boolean")
            referenced.add(choice["document"])
    for name, document in documents.items():
        if not relative(name) or not fields(document, ("source", "catalogs", "contains", "absent", "any_of")):
            raise ValueError(f"{name}: invalid document fields/path")
        if not relative(document["source"]):
            raise ValueError(f"{name}: source must be a checkout-relative path")
        catalogs = document["catalogs"]
        if not isinstance(catalogs, list) or not catalogs or any(c not in ("claude", "codex", "grok", "opencode") for c in catalogs) or len(set(catalogs)) != len(catalogs):
            raise ValueError(f"{name}: invalid catalogs")
        count = 0
        for kind in ("contains", "absent", "any_of"):
            entries = document[kind]
            if not isinstance(entries, list):
                raise ValueError(f"{name}: {kind} must be an array")
            for entry in entries:
                field = "texts" if kind == "any_of" else "text"
                if not fields(entry, (field, "reason"), ("case_sensitive", "unicode_case")) or not nonempty(entry["reason"]):
                    raise ValueError(f"{name}: every rule needs a reason and valid fields")
                if "unicode_case" in entry and type(entry["unicode_case"]) is not bool:
                    raise ValueError(f"{name}: unicode_case must be boolean")
                if entry.get("unicode_case", False) and entry.get("case_sensitive", True):
                    raise ValueError(f"{name}: unicode_case requires case_sensitive=false")
                if "case_sensitive" in entry and type(entry["case_sensitive"]) is not bool:
                    raise ValueError(f"{name}: case_sensitive must be boolean")
                values = entry[field] if kind == "any_of" else [entry[field]]
                if not isinstance(values, list) or not values or not all(nonempty(v) for v in values):
                    raise ValueError(f"{name}: phrases must be nonempty strings")
                count += 1
        if not count and name not in referenced:
            raise ValueError(f"{name}: empty contract")
    return policy


def contracts(policy):
    validate(policy)
    for name, document in policy["documents"].items():
        rules = []
        for kind in ("contains", "absent", "any_of"):
            for entry in document[kind]:
                values = entry["texts"] if kind == "any_of" else [entry["text"]]
                rules.append((kind, values, entry["reason"], entry.get("case_sensitive", True), entry.get("unicode_case", False)))
        yield name, document["source"], rules


def ascii_lower(text):
    return text.translate(str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz"))


def present(text, phrase, case_sensitive, unicode_case=False):
    if not case_sensitive:
        text, phrase = (text.lower(), phrase.lower()) if unicode_case else (ascii_lower(text), ascii_lower(phrase))
    return phrase in text


def violations(text, rules):
    failures = []
    for kind, phrases, reason, sensitive, unicode_case in rules:
        matches = [present(text, phrase, sensitive, unicode_case) for phrase in phrases]
        passes = all(matches) if kind == "contains" else not any(matches) if kind == "absent" else any(matches)
        if not passes:
            failures.append(f"{kind} {phrases!r}: {reason}")
    return failures


def check_texts(policy, texts):
    failures, count = [], 0
    for name, _, rules in contracts(policy):
        count += len(rules)
        failures.extend(f"{name}: {failure}" for failure in violations(texts[name], rules))
    for entry in policy["alternatives"]:
        count += 1
        if not any(present(texts[c["document"]], c["text"], c["case_sensitive"]) != c["absent"] for c in entry["choices"]):
            failures.append(f"alternative {entry['choices']!r}: {entry['reason']}")
    return count, failures


def check(root, policy):
    validate(policy)
    texts = {name: (root / document["source"]).read_text() for name, document in policy["documents"].items()}
    return check_texts(policy, texts)


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
