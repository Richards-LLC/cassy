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

Version changes also select the existing web step: release assets embed the
cas-cli version even when no web path changed. --dist-policy additionally prints
whether committed output must match: release-version changes, changed dist,
and unreadable versions retain equality; source-only lanes defer it to release
preparation. A Git failure exits nonzero, and the caller runs every web check.
"""
import re
import subprocess
import sys
import tomllib

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
    # Runs each CI Commander journey shard (queue retry and quarantine).
    'scripts/ci-queue-flakes.py',
    # Builds and verifies the journey dist hub-web/e2e/journeys/serve-dist.mjs
    # serves.
    'scripts/journey_dist.py',
    # The decision itself. The workflow that runs the step is not an input
    # (cas-c846).
    'scripts/classify-web-inputs.py',
    '.github/actions/classify-required-diff/',
)


def git(*args):
    return subprocess.run(['git', *args], check=True, text=True, capture_output=True).stdout


def covered(path):
    return any(path == entry or (entry.endswith('/') and path.startswith(entry)) for entry in WEB_INPUTS)


def version(ref):
    """An unreadable/inherited version cannot authorize a source-only lane."""
    try:
        value = tomllib.loads(git('show', f'{ref}:cas-cli/Cargo.toml'))['package']['version']
        return value if isinstance(value, str) and re.fullmatch(r'\d+\.\d+\.\d+', value) else None
    except (subprocess.CalledProcessError, tomllib.TOMLDecodeError, KeyError, TypeError):
        return None


def main(argv):
    policy = len(argv) == 4 and argv[3] == '--dist-policy'
    if len(argv) != 3 and not policy:
        print(f'usage: {argv[0]} <merge-base> <head> [--dist-policy]', file=sys.stderr)
        return 2
    base, head = argv[1:3]
    try:
        changed = [path for path in git('diff', '--name-only', '--no-renames', base, head).splitlines() if path]
    except subprocess.CalledProcessError as error:
        print(f'classify-web-inputs: git diff failed: {error.stderr.strip()}', file=sys.stderr)
        return 1
    hits = [path for path in changed if covered(path)]
    if hits:
        print(f'classify-web-inputs: {len(hits)} web input(s) changed, first {hits[0]}', file=sys.stderr)
    base_version, head_version = version(base), version(head)
    # The integration owner commits dist during release preparation. Only a
    # known unchanged-version, unchanged-dist lane may defer that equality.
    dist_needed = (base_version is None or head_version is None or base_version != head_version
                   or any(path.startswith('hub-web/dist/') for path in changed))
    web_needed = bool(hits) or dist_needed
    answer = 'true' if web_needed else 'false'
    if policy:
        answer += ' ' + ('true' if dist_needed else 'false')
    print(answer)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
