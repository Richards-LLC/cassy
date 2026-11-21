#!/usr/bin/env python3
"""Cheap guards before requested tests; independent counts prevent false proofs."""
import argparse
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
import time

GUARD_FILTER = ('test(=test_support::lib_test_process_env_mutation_is_isolated) | '
                'test(=config::mod_tests::serialized_factory_fields_have_registry_entries_cas_f7b9)')
REGISTRY_FILTER = 'test(gates::registry_tests)'
# cas-19792: changed lines the gate registry scan audits (cas-cli/gate_audit.rs):
# refusal and McpError results, gate tags and declarations, and Boolean
# parameters it classifies as bypass candidates. Unsure means run more.
REFUSAL_SITE = re.compile(
    r'\bErr\s*\(|McpError|tool_error|\bbail!|\bensure!|\btag\s*\(|override_tag'
    r'|gate(?:-audit)?\[|:\s*bool\b')


def test_only(path):
    name = Path(path).name
    return (name == 'tests.rs' or name.endswith('_tests.rs') or '/tests/' in f'/{path}'
            or not path.endswith('.rs'))


def touches_refusal_site(repo, base):
    """True when the delivery changes a line the gate registry scan audits."""
    diff = git(repo, 'diff', '--unified=0', '--no-color', base, 'HEAD', '--', 'cas-cli')
    path = None
    for line in diff.splitlines():
        if line.startswith('diff --git '):
            path = line.rsplit(' b/', 1)[-1]
        elif path and path.endswith('gate_audit.rs'):
            return True
        elif (path and not test_only(path) and line[:1] in '+-'
              and not line.startswith(('+++', '---')) and REFUSAL_SITE.search(line[1:])):
            return True
    return False


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True)


def verified(script, cargo, args, count, *, selected=False):
    env = os.environ.copy()
    env.update(CARGO=cargo, VERIFIED_TEST_COUNT_FILE=str(count))
    if not selected:
        # Guard output remains in the producer log, outside the selected capture.
        env.pop('VERIFIED_TEST_LOG', None)
    # The build target lease must survive Python and Bash into Cargo/rustc.
    result = subprocess.run(['bash', '-c', script, 'worker-verified-tests', *args],
                            env=env, close_fds=False)
    if result.returncode:
        raise SystemExit(result.returncode)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True)
    parser.add_argument('--verified-script', required=True)
    parser.add_argument('--cargo', required=True)
    parser.add_argument('--count-file', required=True)
    parser.add_argument('args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    repo = Path.cwd()
    base = git(repo, 'merge-base', args.base, 'HEAD').strip()
    started = time.monotonic()
    subprocess.run([sys.executable, str(repo / 'scripts/check-test-shape.py'),
                    '--changed-since', base], check=True)
    with tempfile.TemporaryDirectory(prefix='worker-proof-guards-') as temporary:
        guard_count = Path(temporary) / 'count'
        guards = GUARD_FILTER
        if touches_refusal_site(repo, base):
            guards = f'{GUARD_FILTER} | {REGISTRY_FILTER}'
            print('guards: refusal or gate site changed; adding gates::registry_tests', flush=True)
        verified(args.verified_script, args.cargo,
                 ['nextest', 'run', '-p', 'cas', '--lib', '-E', guards], guard_count)
        print(f'guards: PASS tests={guard_count.read_text().strip()} '
              f'elapsed={time.monotonic() - started:.3f}s', flush=True)
    requested = args.args[1:] if args.args[:1] == ['--'] else args.args
    verified(args.verified_script, args.cargo, requested, args.count_file, selected=True)


if __name__ == '__main__':
    main()
