#!/usr/bin/env python3
"""Run pytest with a machine-readable pass count and touched-test stability receipt."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET


def run_verified(args, *, cwd=None, env=None, spawn=subprocess.run):
    cwd = Path.cwd() if cwd is None else Path(cwd)
    env = os.environ.copy() if env is None else env
    count_file = env.get('VERIFIED_TEST_COUNT_FILE')
    report_file = env.get('VERIFIED_TEST_REPORT_FILE')
    if count_file:
        Path(count_file).unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix='cas-pytest-') as scratch:
        report = Path(scratch) / 'report.xml'
        native = [*args]
        if not any(arg.startswith('--override-ini=addopts=') for arg in native):
            native.append('--override-ini=addopts=')
        native.append(f'--junitxml={report}')
        result = spawn(['pytest', *native], cwd=cwd, env=env, stdout=None, stderr=subprocess.STDOUT,
                       close_fds=False)
        if result.returncode:
            return result.returncode
        if not report.is_file():
            raise ValueError('pytest did not produce a JUnit report')
        root = ET.parse(report).getroot()
        cases = root.findall('.//testcase')
        passed = sum(not any(case.find(tag) is not None for tag in ('failure', 'error', 'skipped'))
                     for case in cases)
        if passed < 1:
            raise ValueError('pytest: zero tests passed or missing test summary')
        raw = report.read_bytes()
        if count_file:
            Path(count_file).write_text(f'{passed}\n')
        if report_file:
            Path(report_file).write_bytes(raw)
        if env.get('CAS_WORKER_STABILITY_TRIAL') != '1':
            root_dir = cwd.resolve()
            while root_dir != root_dir.parent and not (root_dir / 'scripts/worker-test-stability.py').is_file():
                root_dir = root_dir.parent
            stability = root_dir / 'scripts/worker-test-stability.py'
            if stability.is_file():
                run = spawn([sys.executable, str(stability), '--run-adapter', 'pytest',
                             '--report', str(report), '--', *args], cwd=root_dir, env=env,
                            stdout=None, stderr=subprocess.STDOUT, close_fds=False)
                if run.returncode:
                    return run.returncode
            elif env.get('CAS_FACTORY_MODE') == '1':
                raise ValueError('cannot find worker stability adapter; no bound receipt')
        print(f'verified-pytest: PASS ({passed} tests passed)')
        return 0


if __name__ == '__main__':
    try:
        sys.exit(run_verified(sys.argv[1:]))
    except (OSError, ValueError, ET.ParseError) as error:
        print(f'verified-pytest: FAIL: {error}', file=sys.stderr)
        sys.exit(1)
