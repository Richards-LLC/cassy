#!/usr/bin/env python3
"""Print whether a diff touches the Commander web check's inputs (true/false).

The Commander step (typecheck, unit tests, build, visual QA, dist drift and the
journeys) costs ~19 min per merge-queue entry. It runs when the diff changes
anything that step reads: hub-web itself, the journey catalog, the repo-level
files hub-web's scripts and tests load, or the CI steps that run it. A change
elsewhere in scripts/ or .github/ no longer triggers it (cas-7822: PR #1191
touched one unrelated script and spent 1132 s in the step).

Fail-closed: any Git or parse failure exits nonzero, and the caller then runs
the web check. Without PyYAML a changed workflow counts as a web change.
"""
import json
import subprocess
import sys

# Paths whose change runs the web check. Directory entries end in '/'.
# scripts/test-hub-web-gate.py pins this list against hub-web's own repo-level
# references, so a new `../scripts/...` read in hub-web fails CI until listed.
WEB_INPUTS = (
    'hub-web/',
    # The user-journey catalog the Commander journeys implement.
    'docs/qa/journeys.md',
    # Read by hub-web/scripts/generate-tokens.mjs and src/tokens.test.ts.
    'docs/design/design-tokens.json',
    # Imported by hub-web/scripts/visual-qa.mjs and the real-hub journeys.
    'scripts/visual-qa.mjs',
    # Wraps npm build/typecheck/visual-qa and run-verified-tests.mjs, with its
    # import closure.
    'scripts/worker-memory.py',
    'scripts/host_memory.py',
    'scripts/assembly-proof.py',
    'scripts/proof_target.py',
    'scripts/release_scratch.py',
    # Run inside the CI Commander step.
    'scripts/test-ci-journey-evidence-upload.sh',
    # The decision itself.
    'scripts/classify-web-inputs.py',
    '.github/actions/classify-required-diff/',
)
WORKFLOW = '.github/workflows/ci.yml'


def git(*args):
    return subprocess.run(['git', *args], check=True, text=True, capture_output=True).stdout


def covered(path):
    return any(path == entry or (entry.endswith('/') and path.startswith(entry)) for entry in WEB_INPUTS)


def web_steps(ref):
    """Every workflow step that names hub-web, keyed by job and position."""
    import yaml  # noqa: PLC0415 - optional; ImportError is handled by the caller

    try:
        body = git('show', f'{ref}:{WORKFLOW}')
    except subprocess.CalledProcessError:
        return None
    workflow = yaml.safe_load(body) or {}
    steps = {}
    for job_name, job in (workflow.get('jobs') or {}).items():
        for index, step in enumerate((job or {}).get('steps') or []):
            text = json.dumps(step, sort_keys=True)
            if 'hub-web' in text:
                steps[f'{job_name}#{step.get("name") or index}'] = text
    return steps


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
    if not hits and WORKFLOW in changed:
        try:
            if web_steps(base) != web_steps(head):
                hits = [f'{WORKFLOW} (a step that names hub-web)']
        except ImportError:
            hits = [f'{WORKFLOW} (PyYAML unavailable; any workflow change counts)']
    if hits:
        print(f'classify-web-inputs: {len(hits)} web input(s) changed, first {hits[0]}', file=sys.stderr)
    print('true' if hits else 'false')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
