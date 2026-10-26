#!/usr/bin/env python3
"""Print whether a diff touches the Commander web check's inputs (true/false).

The Commander step (typecheck, unit tests, build, visual QA, dist drift and the
journeys) costs ~19 min per merge-queue entry. It runs when the diff changes
anything that step reads: hub-web itself, the journey catalog, the repo-level
files hub-web's scripts and tests load, or this classifier. A change elsewhere
in scripts/ or .github/ no longer triggers it (cas-7822: PR #1191 touched one
unrelated script and spent 1132 s in the step). That includes
.github/workflows/ci.yml (cas-c846: PR #1198 changed only ci.yml outside
hub-web, ran ~13 min of journeys and was ejected twice by them). Release
plumbing is not an input either (cas-a1b0): hub-web no longer runs its scripts
through scripts/worker-memory.py, so that wrapper's import closure (host
memory, assembly proof, release scratch and env) left this list, and a
release-script change no longer pays ~22 min of Commander checks.

Fail-closed: a Git failure exits nonzero, and the caller then runs the web
check.
"""
import subprocess
import sys

# Paths whose change runs the web check. Directory entries end in '/'.
# scripts/test-hub-web-gate.py pins this list against hub-web's own repo-level
# references, so a new `../scripts/...` read in hub-web fails CI until listed.
WEB_INPUTS = (
    'hub-web/',
    # The user-journey catalog the Commander journeys implement.
    'docs/qa/journeys.md',
    # The journey registry hub-web/src/usage.test.ts reads for the usage map.
    'docs/qa/journeys.yaml',
    # Read by hub-web/scripts/generate-tokens.mjs and src/tokens.test.ts.
    'docs/design/design-tokens.json',
    # Imported by hub-web/scripts/visual-qa.mjs and the real-hub journeys.
    'scripts/visual-qa.mjs',
    # Run inside the CI Commander step.
    'scripts/test-ci-journey-evidence-upload.sh',
    # The decision itself. The workflow that runs the step is not an input
    # (cas-c846).
    'scripts/classify-web-inputs.py',
    '.github/actions/classify-required-diff/',
)


def git(*args):
    return subprocess.run(['git', *args], check=True, text=True, capture_output=True).stdout


def covered(path):
    return any(path == entry or (entry.endswith('/') and path.startswith(entry)) for entry in WEB_INPUTS)


def main(argv):
    if len(argv) != 3:
        print(f'usage: {argv[0]} <merge-base> <head>', file=sys.stderr)
        return 2
    base, head = argv[1], argv[2]
    try:
        changed = [path for path in git('diff', '--name-only', '--no-renames', base, head).splitlines() if path]
    except subprocess.CalledProcessError as error:
        print(f'classify-web-inputs: git diff failed: {error.stderr.strip()}', file=sys.stderr)
        return 1
    hits = [path for path in changed if covered(path)]
    if hits:
        print(f'classify-web-inputs: {len(hits)} web input(s) changed, first {hits[0]}', file=sys.stderr)
    print('true' if hits else 'false')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
