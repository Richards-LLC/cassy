#!/usr/bin/env python3
# gate[C-38 ci]: Fast-row catalog
"""No-build row catalog shared by CI and the factory pre-push gate."""
import argparse
import sys

FAST_ROWS = (
    "failure-log", "version-literals", "changelog-and-versions", "release-script",
    "release-notes-shell-injection", "procedure-guardrails", "test-targets",
    "markdown-lint", "test-shape", "test-env", "builtin-doc-hygiene",
    "journey-catalog", "builtin-skill-limits", "migration-registry",
    "communication-slos", "ci-script-tests-changed", "fixture-paths-src", "builtin-projections-src",
    "call-shapes",
    "release-notes-draft",
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fast-rows", action="store_true", required=True)
    parser.parse_args()
    print(",".join(FAST_ROWS))


if __name__ == '__main__':
    try:
        sys.exit(main())
    except BaseException as error:
        if not isinstance(error, SystemExit) or error.code:
            print("[C-38]", file=sys.stderr)
        raise
