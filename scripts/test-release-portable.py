#!/usr/bin/env python3
"""Portable timestamp/path contracts and the real train report-stage boundary."""
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
TRAIN = ROOT / 'scripts/release-train.sh'


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

    def test_train_stat_fixture_uses_native_host_fallback(self):
        source = (ROOT / 'scripts/test-release-train.sh').read_text()
        start = source.index('# cas-fed5: the train and gate run on stock macOS.')
        end = source.index('# setsid absent (macOS):', start)
        fixture = ('set -euo pipefail\n' + f'tmp={shlex.quote(str(self.root))}\n'
                   + f'repo_root={shlex.quote(str(ROOT))}\n'
                   + 'ok() { echo PASS; }; bad() { echo "$1" >&2; exit 1; }\n'
                   + source[start:end])
        result = subprocess.run(['/bin/bash', '-c', fixture], env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('PASS\n', result.stdout)


class Report(Fixture):
    def setUp(self):
        super().setUp()
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        for args in [('init', '-q'), ('config', 'user.name', 'Fixture'), ('config', 'user.email', 'fixture@example.invalid'),
                     ('commit', '--allow-empty', '-qm', 'fixture'), ('tag', '-a', 'v9.99.7', '-m', 'fixture')]:
            subprocess.run(['git', '-C', str(self.repo), *args], check=True, capture_output=True)
        sha = subprocess.check_output(['git', '-C', str(self.repo), 'rev-parse', 'HEAD'], text=True).strip()
        self.run = self.root / 'artifacts/v9.99.7-repo'
        self.run.mkdir(parents=True)
        (self.run / 'landed-main.sha').write_text(sha + '\n')
        (self.run / 'release-workflow.json').write_text(json.dumps(
            dict(headBranch='v9.99.7', headSha=sha, status='completed', conclusion='success')))
        self.receipts('2026-10-02T19:28:19Z')
        self.renderer = self.root / 'render'
        self.executable(self.renderer, '#!/bin/sh\necho reached >"$RENDER_MARKER"\nexit 7\n')
        self.env.update(CAS_RELEASE_ARTIFACTS_ROOT=str(self.root / 'artifacts'),
                        CAS_RELEASE_TRAIN_REPORT_CMD=str(self.renderer), RENDER_MARKER=str(self.root / 'render.called'),
                        CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO='')

    def receipts(self, timestamp):
        (self.run / 'release-published.receipt').write_text(
            f'TAG=v9.99.7\nPUBLISHED_AT={timestamp}\nLINUX_SHA256={"a" * 64}\nMACOS_SHA256={"b" * 64}\n')
        (self.run / 'release-latency.receipt').write_text(
            f'TAG=v9.99.7\nPUBLISHED_AT={timestamp}\nPUBLISH_LATENCY_SECONDS=25\n')

    def train(self, action):
        return subprocess.run(['bash', str(TRAIN), '9.99.7', str(self.repo), action],
                              env=self.env, text=True, capture_output=True)

    def test_report_verifies_publication_and_reaches_renderer_with_bsd_date(self):
        result = self.train('--report')
        self.assertEqual(result.returncode, 7, result.stdout + result.stderr)
        self.assertIn('publication: verified at 2026-10-02T19:28:19Z', result.stdout)
        self.assertTrue((self.root / 'render.called').is_file())

    def test_invalid_timestamp_still_blocks_renderer(self):
        self.receipts('not-a-date')
        result = self.train('--report')
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('PUBLISHED_AT is invalid', result.stdout)
        self.assertFalse((self.root / 'render.called').exists())

    def report_files(self):
        directory = self.repo / 'docs/release-reports'
        directory.mkdir(parents=True)
        pdf = b'%PDF-1.7\n/Type /Page\n'
        html = b'<html>fixture</html>\n'
        prefix = directory / 'v9.99.7'
        Path(str(prefix) + '.md').write_text('fixture\n')
        Path(str(prefix) + '.pdf').write_bytes(pdf)
        Path(str(prefix) + '.html').write_bytes(html)
        pdf_sha = hashlib.sha256(pdf).hexdigest()
        fields = dict(TAG='v9.99.7', PDF_PATH='docs/release-reports/v9.99.7.pdf',
                      HTML_PATH='docs/release-reports/v9.99.7.html', PDF_SHA256=pdf_sha,
                      PDF_SIZE_BYTES=len(pdf), PDF_REMOTE_SHA256=pdf_sha, PDF_REMOTE_SIZE_BYTES=len(pdf),
                      PDF_REMOTE_PAGE_COUNT=1, HTML_SHA256=hashlib.sha256(html).hexdigest(),
                      PAGE_COUNT=1, PDF_FILE_PERMALINK='https://example.invalid/fixture',
                      PDF_FILE_ID='pdf-fixture', HTML_FILE_ID='html-fixture', USER_THREAD_TS='1.2', DEV_THREAD_TS='3.4')
        (self.run / 'release-report.receipt').write_text(''.join(f'{k}={v}\n' for k, v in fields.items()))
        # Exercise receipt routing without rendering or posting a PDF.
        self.executable(self.bin / 'pdfinfo', '#!/bin/sh\necho "Pages: 1"\n')
        return directory

    def test_report_receipt_accepts_symlinked_checkout_without_realpath(self):
        self.report_files()
        alias = self.root / 'alias/repo'
        alias.parent.mkdir()
        alias.symlink_to(self.repo, target_is_directory=True)
        self.repo = alias
        result = self.train('--report')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('release report: verified PDF=', result.stdout)
        self.assertFalse((self.root / 'render.called').exists())

    def test_report_receipt_rejects_symlink_escape(self):
        directory = self.report_files()
        outside = self.root / 'outside.pdf'
        pdf = directory / 'v9.99.7.pdf'
        pdf.rename(outside)
        pdf.symlink_to(outside)
        result = self.train('--report')
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('PDF_PATH or HTML_PATH escapes the release worktree', result.stdout)


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
