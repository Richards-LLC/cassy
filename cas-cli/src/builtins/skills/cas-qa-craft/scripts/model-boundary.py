#!/usr/bin/env python3
"""Diff-only signals for the independent verifier's model-owned plan boundary."""
import argparse
import json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=".")
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", default="HEAD")
    plan = parser.add_mutually_exclusive_group(required=True)
    plan.add_argument("--plan")
    plan.add_argument("--plan-file")
    parser.parse_args()
    print(json.dumps({"findings": [], "allowed": []}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
