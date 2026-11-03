#!/usr/bin/env python3
"""Diff-only signals for the independent verifier's model-owned plan boundary."""
import argparse
import fnmatch
import json
from pathlib import Path, PurePosixPath
import re
import subprocess

KINDS = {"literal_collection", "regex", "exact_string_assertion"}
STRING = r'''"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*' '''.strip()
STRINGS = re.compile(STRING)


def git(repo, *args):
    result = subprocess.run(["git", "-C", repo, *args], capture_output=True)
    if result.returncode:
        raise ValueError(result.stderr.decode("utf-8", errors="replace").strip())
    return result.stdout.decode("utf-8")


def strings(value, name, paths=False):
    if not isinstance(value, list) or any(not isinstance(v, str) or not v.strip() for v in value):
        raise ValueError(f"{name} must be an array of nonempty strings")
    if paths and any(PurePosixPath(v).is_absolute() or ".." in PurePosixPath(v).parts
                     or "\\" in v or ":" in v for v in value):
        raise ValueError(f"{name} must contain repository-relative forward-slash globs")
    return value


def boundary(text):
    if not text.lstrip().startswith("{"):
        blocks = re.findall(r"^```model-boundary[^\S\n]*\n(.*?)\n```[^\S\n]*$", text, re.M | re.S)
        if not blocks:
            if "```model-boundary" in text:
                raise ValueError("malformed model-boundary block")
            return {}
        if len(blocks) != 1:
            raise ValueError("use one model-boundary block per plan")
        text = blocks[0]
    value = json.loads(text)
    keys = {"model_owned_paths", "model_owned_concerns", "model_boundary_exceptions"}
    if not isinstance(value, dict) or set(value) - keys:
        raise ValueError("model boundary must be an object with documented fields only")
    strings(value.get("model_owned_paths", []), "model_owned_paths", paths=True)
    strings(value.get("model_owned_concerns", []), "model_owned_concerns")
    exceptions = value.get("model_boundary_exceptions", [])
    if not isinstance(exceptions, list):
        raise ValueError("model_boundary_exceptions must be an array")
    for exception in exceptions:
        if not isinstance(exception, dict) or set(exception) != {"paths", "kinds", "reason"}:
            raise ValueError("each exception needs paths, kinds and a reason")
        if not strings(exception["paths"], "exception paths", paths=True):
            raise ValueError("exception paths must not be empty")
        if not strings(exception["kinds"], "exception kinds") or set(exception["kinds"]) - KINDS:
            raise ValueError("exception kinds must name supported detectors")
        if not isinstance(exception["reason"], str) or not exception["reason"].strip():
            raise ValueError("exception reason must not be empty")
    return value


def added_lines(patch):
    lines, cursor = set(), None
    for line in patch.splitlines():
        if line.startswith("@@ "):
            cursor = int(re.search(r"\+(\d+)", line).group(1))
        elif cursor is not None:
            if line.startswith("+"):
                lines.add(cursor)
                cursor += 1
            elif line.startswith(" "):
                cursor += 1
    return lines


def constructs(text):
    # Syntax signals only: no intent vocabulary or reply interpretation.
    # Full-file spans let a new member of an existing multiline list count.
    for match in re.finditer(r"\[[^\[\]]*\]|\{[^{}]*\}|(?:=|\bin)\s*\([^()]*\)", text):
        body = match.group().lstrip("= \t\r\n")
        if body.startswith("in"):
            body = body[2:].lstrip()
        body = body[1:-1]
        if STRINGS.search(body) and not re.sub(STRING + r"|[\s,]", "", body):
            yield "literal_collection", match
    regex = (r"(?:Regex::new|re\.(?:compile|search|match|fullmatch|findall)|RegExp|regex!)\s*\(\s*[rubf#]*(?:"
             + STRING + r")|(?:=|\breturn|\()\s*/(?:\\.|[^/\n])+/[a-z]*")
    for match in re.finditer(regex, text):
        yield "regex", match
    assertions = (r"(?:assert_eq!|assert_ne!|assertEqual|assertNotEqual)\s*\([\s\S]*?\)\s*(?:;|$)"
                  r"|\.to(?:Be|Equal|StrictEqual)\s*\(\s*(?:" + STRING + r")"
                  r"|\bassert!?[^\n;]*(?:==|!=)[^\n;]*")
    for match in re.finditer(assertions, text, re.M):
        if STRINGS.search(match.group()):
            yield "exact_string_assertion", match


def check(args):
    base = git(args.repo, "rev-parse", "--verify", "--end-of-options", args.base + "^{commit}").strip()
    head = git(args.repo, "rev-parse", "--verify", "--end-of-options", args.head + "^{commit}").strip()
    if args.plan_file:
        text = Path(args.plan_file).read_text()
        source = str(Path(args.plan_file).resolve())
    else:
        strings([args.plan], "plan path", paths=True)
        text = git(args.repo, "show", f"{base}:{args.plan}")
        source = f"{base}:{args.plan}"
    policy = boundary(text)
    paths = policy.get("model_owned_paths", [])
    concerns = policy.get("model_owned_concerns", [])
    report = {"base": base, "head": head, "plan": source, "boundary": policy,
              "findings": [], "allowed": []}
    if not paths and not concerns:
        report["status"] = "not_declared"
        return report
    # Treat moves as additions so moving code into an owned path cannot hide it.
    files = git(args.repo, "diff", "--no-ext-diff", "--no-textconv", "--no-renames",
                "--diff-filter=AM", "--name-only", "-z", base, head, "--").split("\0")
    for file in sorted(filter(None, files)):
        if paths and not any(fnmatch.fnmatchcase(file, pattern) for pattern in paths):
            continue
        patch = git(args.repo, "diff", "--no-ext-diff", "--no-textconv", "--no-renames", "--no-color",
                    "--unified=0", base, head, "--", file)
        added = added_lines(patch)
        if not added:
            continue
        text = git(args.repo, "show", f"{head}:{file}")
        sites = set()
        for kind, match in constructs(text):
            first = text.count("\n", 0, match.start()) + 1
            last = text.count("\n", 0, match.end()) + 1
            changed = added.intersection(range(first, last + 1))
            if changed:
                sites.add((min(changed), kind))
        for line, kind in sorted(sites):
            site = {"file": file, "line": line, "kind": kind}
            exception = next((e for e in policy.get("model_boundary_exceptions", [])
                              if kind in e["kinds"] and any(fnmatch.fnmatchcase(file, p) for p in e["paths"])), None)
            if exception:
                report["allowed"].append({**site, "reason": exception["reason"]})
            else:
                report["findings"].append(site)
    report["status"] = "rejected" if report["findings"] else "clear"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=".")
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", default="HEAD")
    plan = parser.add_mutually_exclusive_group(required=True)
    plan.add_argument("--plan")
    plan.add_argument("--plan-file")
    args = parser.parse_args()
    try:
        report = check(args)
    except (OSError, ValueError) as error:
        print(json.dumps({"error": str(error)}))
        return 2
    print(json.dumps(report, indent=2))
    return 1 if report["findings"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
