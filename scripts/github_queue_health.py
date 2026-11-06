#!/usr/bin/env python3
"""Advisory merge-queue preflight. One bounded read; never writes or fails a release."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

QUERY = Path(__file__).with_name("github-queue-health.graphql").read_text()
SLUG = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\Z")


def slug_ok(value):
    return isinstance(value, str) and SLUG.fullmatch(value) and all(p not in (".", "..") for p in value.split("/"))


def report(value):
    errors = value.get("errors")
    if errors is not None and (not isinstance(errors, list) or errors):
        raise ValueError("GraphQL error (possibly rate limited)")
    repo = value["data"]["repository"]
    branch = repo["defaultBranchRef"]["name"]
    rules = repo["defaultBranchRef"]["rules"]
    if rules["pageInfo"]["hasNextPage"] is not False:
        raise ValueError("active rules incomplete; no additional request made")
    queue = [r for r in rules["nodes"] if r["type"] == "MERGE_QUEUE"]
    if not queue:
        return f"merge queue not configured on {branch}; expected build>=5, merge>=5, ALLGREEN (report only)"
    warnings = []
    for rule in queue:
        params = rule["parameters"]
        build, merge, grouping = (params[k] for k in ("maxEntriesToBuild", "maxEntriesToMerge", "groupingStrategy"))
        if type(build) is not int or type(merge) is not int or not isinstance(grouping, str):
            raise ValueError("queue parameters invalid")
        if build >= 5 and merge >= 5 and grouping == "ALLGREEN":
            continue
        ruleset = rule["repositoryRuleset"]
        source, identifier = ruleset["source"], ruleset["databaseId"]
        if type(identifier) is not int or identifier <= 0:
            raise ValueError("ruleset id invalid")
        if source["__typename"] == "Repository" and slug_ok(source["nameWithOwner"]):
            endpoint = f"repos/{source['nameWithOwner']}/rulesets/{identifier}"
        elif source["__typename"] == "Organization" and slug_ok(source["login"] + "/repo"):
            endpoint = f"orgs/{source['login']}/rulesets/{identifier}"
        else:
            raise ValueError("ruleset source unavailable")
        update = ('{name,target,enforcement,conditions,bypass_actors,rules} | '
                  '(.rules[] | select(.type == "merge_queue").parameters) |= '
                  '(.max_entries_to_build = ([.max_entries_to_build,5]|max) | '
                  '.max_entries_to_merge = ([.max_entries_to_merge,5]|max) | .grouping_strategy = "ALLGREEN")')
        fix = f"gh api {endpoint} | jq '{update}' | gh api -X PUT {endpoint} --input -"
        warnings.append(f"ruleset {identifier}: build={build}, merge={merge}, grouping={grouping}; expected build>=5, merge>=5, ALLGREEN. Run: {fix}")
    return f"merge queue drift on {branch}: " + "; ".join(warnings) if warnings else None


def preflight(worktree, gh):
    try:
        origin = subprocess.run(["git", "config", "--get", "remote.origin.url"], cwd=worktree,
                                capture_output=True, text=True, timeout=2, check=True).stdout.strip()
        match = re.fullmatch(r"(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)(.+?)(?:\.git)?/?", origin)
        if not match or not slug_ok(match[1]):
            raise ValueError("no GitHub origin")
        owner, name = match[1].split("/")
        result = subprocess.run([gh, "api", "graphql", "-f", f"owner={owner}", "-f", f"name={name}",
                                 "-f", "query=" + QUERY], cwd=worktree,
                                env={**os.environ, "GH_HOST": "github.com"},
                                capture_output=True, text=True, timeout=2)
        if result.returncode:
            # Do not expose provider stderr, which can carry credentials.
            raise ValueError("authentication, rate limit or provider failure")
        return report(json.loads(result.stdout))
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError, AttributeError, IndexError):
        return "merge queue unavailable (authentication, rate limit, provider response or origin); report only"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worktree", type=Path, default=Path.cwd())
    parser.add_argument("--gh", default="gh")
    args = parser.parse_args()
    warning = preflight(args.worktree, args.gh)
    if warning:
        # One warning line, even if a provider branch name contains whitespace.
        print("release: warning: " + " ".join(warning.split()), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
