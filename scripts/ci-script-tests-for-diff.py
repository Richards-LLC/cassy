#!/usr/bin/env python3
"""Run the script-tier tests a lane's changes touch (no build).

`make -C cas-cli test-ci-tiers` runs every script test and takes several
minutes, so merge admission runs only the entries whose subject changed:

- `scripts/test-<stem>.*` itself changed, or
- a file named `scripts/<stem>.*` changed (e.g. `scripts/check-foo.py`, its
  fixture `scripts/check-foo.json`), for the test `scripts/test-<stem>.*`, or
- a non-script input in NAMED changed, for its one named assertion.
- either terminal-qa runner copy changed, for `terminal-qa.test.mjs`.
- a registered input in PROVISIONED_INPUTS changed, for its provisioned suite.

Entries in HEAVY take longer than lane admission's budget allows (about 90 s
for every fast row together). They are reported as deferred to the full
`ci-script-tests` row, which the integration sweep and the cut still run.
Provisioned suites run in the existing scoped validation job on changed
inputs. The source-built terminal suite also runs after the full-lane build.

Exit 0 when every selected test passes, 1 when one fails, 2 when the tier
list or the diff cannot be read.
"""

from __future__ import annotations

import argparse
from fnmatch import fnmatchcase
from pathlib import Path
import re
import shlex
import subprocess
import sys
import time

MAKEFILE = "cas-cli/Makefile"
TARGET = "test-ci-tiers:"
# Measured on the factory host (2026-10-10, idle): each takes over 20 s alone,
# more than lane admission can spend on one row. PROVISIONED_INPUTS below
# records the separate path-scoped cohort added after those measurements.
HEAVY = {
    "scripts/test-codemap-latency-receipt.sh": 126,
    "scripts/test-fast-release-rows.py": 70,
    "scripts/test-worker-memory.py": 21,
}
# cas-eac2: hub-web/package.json scripts set worker-memory admission weights.
# cas-2077 changed one and first failed in the merge queue (4 GiB != 2 GiB);
# this one assertion pins that contract in 0.001 s. The full suite stays HEAVY.
NAMED = {
    "hub-web/package.json": "python3 scripts/test-worker-memory.py "
                            "AdmissionTests.test_cas_4cb9_weights_follow_shell_and_actual_package_scripts",
}
RELEASE_TRAIN_TEST = "scripts/test-release-portable.py"
TERMINAL_QA_TEST = "scripts/terminal-qa.test.mjs"
TERMINAL_QA_BUILTIN = "cas-cli/src/builtins/skills/cas-cli-craft/scripts/terminal-qa.mjs"

# These suites need provisioned tools or exceed fast-row admission's budget.
# The existing scoped validation job runs just their changed inputs on PRs/pushes;
# test-ci-tiers retains every command for assembly. Directory globs include
# fixture additions/deletions, not just the runner's similarly named script.
VISUAL_INPUTS = (
    "scripts/visual-qa.mjs",
    "cas-cli/src/builtins/skills/cas-ui-craft/scripts/visual-qa.mjs",
    "hub-web/package.json", "hub-web/package-lock.json",
)
COHORT_INPUTS = ("scripts/ci-script-tests-for-diff.py", "cas-cli/Makefile",
                 ".github/actions/run-script-suite-tests/*")
PROVISIONED_INPUTS = {
    "scripts/test-cas-image-generate.sh": (
        "cas-cli/src/builtins/skills/cas-image-generate/scripts/generate-image.sh",),
    "scripts/test-check-changelog-lint.sh": (
        "scripts/check-changelog-lint.sh", "scripts/markdownlint.py", "CHANGELOG.md",
        ".markdownlint-cli2.jsonc", "package.json", "package-lock.json"),
    "scripts/test-codex-cargo-admission.py": (".codex/hooks.json",),
    "scripts/test-factory-client-terminal.py": (
        "cas-cli/src/ui/factory/client.rs", "cas-cli/src/ui/factory/session.rs",
        "cas-cli/src/ui/factory/protocol.rs", "cas-cli/src/cli/factory/mod.rs",
        "Cargo.lock", "Cargo.toml", "cas-cli/Cargo.toml"),
    "scripts/test-journey-bundles.py": ("scripts/journey-bundles.py",),
    "scripts/test-opencode-slack-policy.mjs": ("crates/cas-mux/src/opencode.rs",),
    "scripts/test-release-budget-breach.py": (
        "scripts/release-metrics.py", "scripts/release-budget-breach.py",
        "scripts/release-stage-budgets.json", "scripts/release-train.d/post-publication.sh",
        "scripts/release-train.d/announce.sh", "scripts/release-portable.sh",
        "scripts/release-latency-receipt.sh", "scripts/release-train-announce.py"),
    "scripts/test-release-report-post.py": (
        "scripts/release-report-post.py", "docs/release-reports/v3.19.0.pdf",
        "docs/release-reports/v3.20.0.pdf"),
    "scripts/test-release-report.py": (
        "cas-cli/src/builtins/skills/cas-release-report/scripts/render.py",
        "cas-cli/src/deliverable/assets/*", "docs/release-reports/2026-09-08-v3.19.0.md",
        "cas-cli/src/builtins/skills/design-spec/references/design-tokens.json",
        "docs/design/*", "DESIGN.md"),
    "scripts/test-verified-web-runners.mjs": (
        "hub-web/scripts/run-verified-tests.mjs", "hub-web/package.json", "hub-web/package-lock.json"),
    "scripts/test_factory_model_history.py": ("scripts/factory-model-history.py",),
    **{f"scripts/visual-qa{suffix}.test.mjs": VISUAL_INPUTS + fixtures for suffix, fixtures in {
        "": ("scripts/visual-qa-fixtures/*",
             "cas-cli/src/builtins/skills/cas-ui-craft/references/exemplars/*"),
        "-enter-transition": ("scripts/visual-qa-fixtures/enter-transition.html",),
        "-entrypoint": ("scripts/visual-qa-fixtures/clean.html", "hub-web/scripts/visual-qa.mjs"),
        "-readiness": ("scripts/visual-qa-fixtures/loading-only.html", "scripts/visual-qa-fixtures/ranking.after.png"),
        "-secrets": (),
    }.items()},
}
WEB_TESTS = {"scripts/test-verified-web-runners.mjs"} | {
    path for path in PROVISIONED_INPUTS if path.startswith("scripts/visual-qa")
}


def requirements(selected: list[tuple[str, str]]) -> dict[str, bool]:
    tests = {test for test, _ in selected}
    return {
        "needed": bool(tests),
        "web": bool(tests & WEB_TESTS),
        "browser": any(test.startswith("scripts/visual-qa") for test in tests),
        "pdf": "scripts/test-release-report.py" in tests,
        "lint": "scripts/test-check-changelog-lint.sh" in tests,
    }


def tier_commands(repo: Path) -> list[str]:
    """The test-ci-tiers recipe; empty for a checkout without the tier."""
    if not (repo / MAKEFILE).is_file():
        return []
    commands, inside = [], False
    for line in (repo / MAKEFILE).read_text().splitlines():
        if line.startswith(TARGET):
            inside = True
            continue
        if inside:
            if not line.startswith("\t"):
                break
            commands.append(line.strip().removeprefix("cd .. && "))
    return commands


def test_script(command: str) -> str | None:
    for token in shlex.split(command):
        token = token.removeprefix("./")
        if re.fullmatch(r"scripts/(?:test[-_][\w.-]+|(?:terminal|visual)-qa[\w.-]*\.test\.mjs)", token):
            return token
    return None


def changed_paths(repo: Path, base: str) -> list[str]:
    out = subprocess.run(["git", "-C", str(repo), "diff", "--name-only", "--no-renames", base],
                         capture_output=True, text=True)
    if out.returncode:
        raise ValueError(f"cannot diff scripts against {base}: {out.stderr.strip()}")
    return [line for line in out.stdout.splitlines() if line]


def stem(path: str) -> str:
    name = Path(path).name
    return name.split(".", 1)[0]


def select(commands: list[str], changed: list[str]) -> list[tuple[str, str]]:
    selected = []
    for command in commands:
        test = test_script(command)
        if not test:
            continue
        subject = stem(test).removeprefix("test-")
        # cas-850b: stem matching misses both release-train.sh and
        # release-train.d/post-publication.sh, although the portable suite
        # exercises their release/report lifecycle.
        release_train_related = test == RELEASE_TRAIN_TEST and any(
            path.startswith("scripts/release-train") for path in changed
        )
        terminal_qa_related = test == TERMINAL_QA_TEST and any(
            path.startswith("scripts/terminal-qa") or path == TERMINAL_QA_BUILTIN for path in changed
        )
        provisioned_related = test in PROVISIONED_INPUTS and any(
            fnmatchcase(path, pattern) for path in changed
            for pattern in (*PROVISIONED_INPUTS[test], *COHORT_INPUTS))
        # Registered suites have an explicit input closure: basename matches
        # outside scripts/ must not select an unrelated suite.
        stem_related = test not in PROVISIONED_INPUTS and any(
            path.startswith("scripts/") and stem(path) in {subject, stem(test)} for path in changed)
        if test in changed or stem_related or provisioned_related or release_train_related or terminal_qa_related:
            selected.append((test, command))
    return selected + [(path, NAMED[path]) for path in sorted(NAMED) if path in changed]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    comparison = parser.add_mutually_exclusive_group(required=True)
    comparison.add_argument("--base")
    comparison.add_argument("--all", action="store_true", help="select the exhaustive assembly tier")
    parser.add_argument("--repo", default=".")
    parser.add_argument("--only", help="restrict changed-input selection to one tier test path")
    parser.add_argument("--dry-run", action="store_true", help="print the selection only")
    parser.add_argument("--provisioned", action="store_true", help="run only the provisioned CI cohort, including slow suites")
    parser.add_argument("--github-output", type=Path, help="write selected tool requirements and exit without running tests")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    try:
        commands = tier_commands(repo)
        if args.only and args.only not in {test_script(command) for command in commands} | NAMED.keys():
            raise ValueError(f"unknown tier test: {args.only}")
        selected = ([(test_script(command), command) for command in commands if test_script(command)]
                    if args.all else select(commands, changed_paths(repo, args.base)))
        if args.provisioned:
            selected = [(test, command) for test, command in selected if test in PROVISIONED_INPUTS]
        if args.only:
            selected = [(test, command) for test, command in selected if test == args.only]
    except (OSError, ValueError) as error:
        print(f"ci-script-tests-changed: {error}", file=sys.stderr)
        return 2
    if args.github_output:
        with args.github_output.open("a") as output:
            for name, needed in requirements(selected).items():
                print(f"{name}={str(needed).lower()}", file=output)
        return 0
    if not selected:
        print("ci-script-tests-changed: no script test covers this lane's changes")
        return 0
    failed = []
    for test, command in selected:
        if test in PROVISIONED_INPUTS and not (args.provisioned or args.all or args.only):
            print(f"ci-script-tests-changed: DEFERRED {test} to the path-scoped CI validation step")
            continue
        if test in HEAVY and not args.all:
            print(f"ci-script-tests-changed: DEFERRED {test} (~{HEAVY[test]}s) to the full "
                  f"ci-script-tests row")
            continue
        if args.dry_run:
            print(f"ci-script-tests-changed: would run {command}")
            continue
        started = time.monotonic()
        result = subprocess.run(command, shell=True, cwd=repo)
        took = time.monotonic() - started
        status = "PASS" if result.returncode == 0 else f"FAIL (exit {result.returncode})"
        print(f"ci-script-tests-changed: {status} {command} ({took:.1f}s)", flush=True)
        if result.returncode:
            failed.append(test)
    if failed:
        print(f"ci-script-tests-changed: failed: {', '.join(failed)}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
