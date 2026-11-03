#!/usr/bin/env python3
"""Portable timestamp/path contracts."""
import json
import hashlib
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / 'scripts/release-portable.sh'


class Fixture(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.executable(self.bin / 'date', '''#!/bin/sh
for arg in "$@"; do
  case "$arg" in -d|--date*|-I*) exit 64;; esac
done
exec /bin/date "$@"
''')
        for name in ('realpath', 'readlink'):
            self.executable(self.bin / name, '#!/bin/sh\nexit 64\n')
        self.env = dict(os.environ, PATH=str(self.bin) + ':' + os.environ['PATH'])

    def executable(self, path, source):
        path.write_text(source)
        path.chmod(0o755)

    def helper(self, function, argument):
        return subprocess.run(['bash', '-euo', 'pipefail', '-c',
                               f'source {shlex.quote(str(HELPER))}; {function} "$1"', 'fixture', argument],
                              env=self.env, text=True, capture_output=True)


class Portable(Fixture):
    def test_bsd_only_timestamp_parser_handles_utc_offset_and_fraction(self):
        for timestamp, epoch in [('1970-01-01T00:00:00Z', '0'),
                                 ('1970-01-01T01:00:00+01:00', '0'),
                                 ('1969-12-31T23:59:59.999Z', '-1'),
                                 ('2026-10-02T19:28:19Z', '1790969299')]:
            with self.subTest(timestamp=timestamp):
                result = self.helper('release_portable_timestamp_epoch', timestamp)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), epoch)

    def test_invalid_and_timezone_free_timestamps_fail(self):
        for timestamp in ('invalid', '', '2026-02-30T00:00:00Z', '1970-01-01T00:00:00'):
            with self.subTest(timestamp=timestamp):
                result = self.helper('release_portable_timestamp_epoch', timestamp)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, '')

    def test_realpath_resolves_symlinks_and_missing_tail_without_coreutils(self):
        outside = self.root / 'outside space'
        outside.mkdir()
        link = self.root / 'link'
        link.symlink_to(outside, target_is_directory=True)
        result = self.helper('release_portable_realpath', str(link / 'missing' / '..' / 'report.pdf'))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), str(outside.resolve() / 'report.pdf'))

    def test_linux_date_result_is_preserved(self):
        self.executable(self.bin / 'date', '#!/bin/sh\necho 4242\n')
        result = self.helper('release_portable_timestamp_epoch', 'GNU-supported date input')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), '4242')

    def test_sha_wrapper_uses_shasum_when_no_binary_exists(self):
        # A function called sha256sum must not be mistaken for its absent
        # executable. Limit PATH to the stock Perl hashing tool and Python.
        isolated = self.root / 'stock-bin'
        isolated.mkdir()
        shasum = subprocess.check_output(['bash', '-c', 'command -v shasum'], text=True).strip()
        (isolated / 'shasum').symlink_to(shasum)
        env = dict(self.env, PATH=str(isolated))
        result = subprocess.run(['/bin/bash', '-euo', 'pipefail', '-c',
                                 f'source {shlex.quote(str(HELPER))}; release_portable_define_sha256sum; '
                                 "printf fixture | sha256sum"],
                                env=env, text=True, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.split()[0], hashlib.sha256(b'fixture').hexdigest())


class MacReceipts(Fixture):
    def test_release_latency_discovers_run_dir_with_stock_bash_and_bsd_date(self):
        artifacts = self.root / 'artifacts'
        run = artifacts / 'v9.99.7-repo'
        run.mkdir(parents=True)
        (run / 'gate.green.epoch').write_text('100\n')
        (run / 'pipeline.start.epoch').write_text('105\n')
        gh = self.bin / 'gh'
        self.executable(gh, '''#!/bin/sh
case "$1" in
  release) echo 2026-08-20T13:04:10.500+01:00;;
  api) echo '{"workflow_runs":[{"id":1,"created_at":"2026-08-20T12:00:00.500Z"}]}' ;;
  *) exit 64;;
esac
''')
        env = dict(self.env, GH_BIN=str(gh), CAS_RELEASE_ARTIFACTS_ROOT=str(artifacts),
                   CAS_RELEASE_TRAIN_RUN_DIR='', CAS_RELEASE_RUN_DIR='')
        result = subprocess.run(['/bin/bash', str(ROOT / 'scripts/release-latency-receipt.sh'), 'v9.99.7'],
                                env=env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('PUBLISH_LATENCY_SECONDS=250\n', result.stdout)
        self.assertIn('GREEN_TO_PIPELINE_SECS=5\n', result.stdout)

    def test_codemap_timing_preserves_fractional_numeric_offset(self):
        repo = self.root / 'repo'
        (repo / '.claude').mkdir(parents=True)
        (repo / '.claude/CODEMAP.md').write_text('fixture\n')
        (repo / 'scripts').mkdir()
        self.executable(repo / 'scripts/classify-ci-diff.sh', '#!/bin/sh\necho docs-only\n')
        # This fixture isolates timestamp handling from the separate CI policy suite.
        self.executable(repo / 'scripts/test-ci-test-tiers.sh', '#!/bin/sh\nexit 0\n')
        for args in [('init', '-q'), ('config', 'user.name', 'Fixture'), ('config', 'user.email', 'fixture@example.invalid'),
                     ('remote', 'add', 'origin', 'https://example.invalid/fixture'), ('add', '.'), ('commit', '-qm', 'fixture')]:
            subprocess.run(['git', '-C', str(repo), *args], check=True, capture_output=True)
        cas = self.bin / 'cas'
        self.executable(cas, '#!/bin/sh\necho "Status: up to date"\n')
        gh = self.bin / 'gh'
        payload = dict(createdAt='2026-08-30T13:00:00.500+01:00', url='https://example.invalid/run',
                       jobs=[dict(name='Fast Validation', startedAt='2026-08-30T12:00:03.500Z',
                                  completedAt='2026-08-30T12:00:08.500Z', conclusion='success'),
                             dict(name='macOS Check', startedAt='2026-08-30T12:00:04.500Z',
                                  completedAt='2026-08-30T12:00:08.500Z', conclusion='success')])
        self.executable(gh, '#!/bin/sh\nprintf "%s\\n" ' + shlex.quote(json.dumps(payload)) + '\n')
        result = subprocess.run(['/bin/bash', str(ROOT / 'scripts/codemap-latency-receipt.sh'),
                                 '--repo-root', str(repo), '--cas-bin', str(cas), '--github-run-id', '1'],
                                env=dict(self.env, GH_BIN=str(gh)), text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('GITHUB_QUEUE_SECONDS=3\n', result.stdout)
        self.assertIn('DOCS_ONLY_REQUIRED_COMPUTE_SECONDS=5\n', result.stdout)


if __name__ == '__main__':
    unittest.main()
