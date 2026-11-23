#!/usr/bin/env python3
"""Release shell tests replace only the CLI boundary; Git remains real.

Authority, provider verification and atomic publication are exercised by the
Rust two-clone tests. This adapter verifies shell routing and refusal handling.
"""
import argparse
import os
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument("action")
for name in ("tag", "sha", "branch", "pr", "repo", "notes", "title", "run", "attempt"):
    parser.add_argument(f"--{name}")
parser.add_argument("--remote", default="origin")
parser.add_argument("--prerelease", action="store_true")
parser.add_argument("--isolated-dry-run", action="store_true")
parser.add_argument("assets", nargs="*")
args = parser.parse_args()
if os.environ.get("FAKE_CAS_REFUSE_PUBLICATION") == "1":
    sys.exit(77)


def git(*argv):
    return subprocess.check_output(["git", *argv], text=True).strip()


def gh(*argv):
    subprocess.run([os.environ.get("GH_BIN", "gh"), *argv], check=True)


if args.action == "is-enrolled":
    print(str(args.branch.removeprefix("refs/heads/") == os.environ.get("FAKE_ENROLLED_BRANCH")).lower())
elif args.action == "is-bound-tag":
    print(str(args.tag == os.environ.get("FAKE_BOUND_TAG")).lower())
elif args.action == "publish-branch":
    git("push", "--quiet", args.remote, f"{args.sha}:refs/heads/{args.branch}")
elif args.action == "arm-pr":
    gh("pr", "merge", args.pr, "--auto", "--merge", "--match-head-commit", args.sha)
elif args.action == "publish-tag":
    if args.isolated_dry_run and (not args.tag.startswith("dryrun-") or args.tag == os.environ.get("FAKE_BOUND_TAG")):
        sys.exit(78)
    ref = f"refs/tags/{args.tag}"
    remote = git("ls-remote", "--tags", "origin", ref)
    if remote:
        git("fetch", "--no-tags", "origin", ref)
        obj = remote.split()[0]
        if git("rev-parse", f"{obj}^{{commit}}") != args.sha:
            sys.exit(79)
    else:
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "tag", "-a", args.tag, "-m", args.tag, args.sha)
        git("push", "origin", ref)
elif args.action == "create-release":
    gh("release", "create", args.tag, "--repo", args.repo, "--verify-tag", "--title", args.title,
       "--notes-file", args.notes, *(["--prerelease"] if args.prerelease else []), *args.assets)
else:
    parser.error("unexpected publication action")
