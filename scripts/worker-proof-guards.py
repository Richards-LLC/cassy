#!/usr/bin/env python3
"""Cheap guards before requested tests; independent counts prevent false proofs."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

GUARD_FILTER = ('test(=test_support::lib_test_process_env_mutation_is_isolated) | '
                'test(=config::mod_tests::serialized_factory_fields_have_registry_entries_cas_f7b9)')
REGISTRY_FILTER = 'test(gates::registry_tests)'
# cas-19792: the registry scan costs ~40 s, so it runs only when this probe,
# using the scan's own inventory (cas-cli/gate_audit.rs), finds an audited
# change. No probe answer means run it.
SELECTOR_FILTER = 'test(=gates::registry_tests::delivery_touches_audited_site)'
# run-verified-tests.sh's status for a selection that ran zero tests.
ZERO_SELECTION = 4


def git(repo, *args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True)


def verified(script, cargo, args, count, *, selected=False, extra=None):
    env = os.environ.copy()
    env.update(extra or {}, CARGO=cargo, VERIFIED_TEST_COUNT_FILE=str(count))
    if not selected:
        # Guard output remains in the producer log, outside the selected capture.
        env.pop('VERIFIED_TEST_LOG', None)
    # The build target lease must survive Python and Bash into Cargo/rustc.
    result = subprocess.run(['bash', '-c', script, 'worker-verified-tests', *args],
                            env=env, close_fds=False)
    if result.returncode:
        # Exit 4 marks a requested selection that ran no test, which the capped
        # runner answers with a crate run (cas-fcb1). A fixed guard that runs
        # no test is a failed guard, never that fallback.
        raise SystemExit(1 if result.returncode == ZERO_SELECTION and not selected else result.returncode)


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
        answer = Path(temporary) / 'select'
        verified(args.verified_script, args.cargo,
                 ['nextest', 'run', '-p', 'cas', '--lib', '-E', f'{GUARD_FILTER} | {SELECTOR_FILTER}'],
                 guard_count, extra={'CAS_GATE_AUDIT_SELECT_BASE': base,
                                     'CAS_GATE_AUDIT_SELECT_ROOT': str(repo),
                                     'CAS_GATE_AUDIT_SELECT_OUT': str(answer)})
        tests = int(guard_count.read_text().strip())
        if not answer.is_file() or answer.read_text().strip() != '0':
            print('guards: delivery touches an audited gate site; adding gates::registry_tests', flush=True)
            verified(args.verified_script, args.cargo,
                     ['nextest', 'run', '-p', 'cas', '--lib', '-E', REGISTRY_FILTER], guard_count)
            tests += int(guard_count.read_text().strip())
        print(f'guards: PASS tests={tests} '
              f'elapsed={time.monotonic() - started:.3f}s', flush=True)
    requested = args.args[1:] if args.args[:1] == ['--'] else args.args
    verified(args.verified_script, args.cargo, requested, args.count_file, selected=True)


if __name__ == '__main__':
    main()
