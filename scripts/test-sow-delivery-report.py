#!/usr/bin/env python3
"""Gate checks of cas-sow-delivery-report: python3 scripts/test-sow-delivery-report.py."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

CHECK = Path(__file__).resolve().parent.parent / 'cas-cli/src/builtins/skills/cas-sow-delivery-report/scripts/check-sow.py'
CAPTURE = CHECK.with_name('capture.mjs')
HEADER = '| R# | The SOW says | Status | Evidence | Owner or open decision |\n| --- | --- | --- | --- | --- |\n'


def run(tmp, name, text, *args):
    path = Path(tmp) / name
    path.write_text(text)
    return subprocess.run([sys.executable, str(CHECK), *args[:1], str(path), *args[1:]],
                          capture_output=True, text=True)


class SowGateTests(unittest.TestCase):
    def test_gap_row_stops_the_completion_report_and_names_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            done = HEADER + ('| R1 | "Dashboard." | Met (live) | shot R1 | |\n'
                             '| R2 | "Accuracy." | Met (measured) | delivered report | |\n'
                             '| R3 | "Export." | Partial | works | Client: one file or many? |\n')
            self.assertEqual(run(tmp, 'done.md', done, 'gap').returncode, 0)
            gap = done + '| R4 | "Regions." | Gap → PM | SOW silent | PM |\n'
            result = run(tmp, 'gap.md', gap, 'gap')
            self.assertEqual(result.returncode, 1)
            self.assertIn('STOP: 1 Gap row(s): R4 (PM)', result.stderr)
            self.assertIn('status variant', result.stderr)

    def test_hygiene_fails_on_a_planted_ticket_label(self):
        with tempfile.TemporaryDirectory() as tmp:
            clean = '# Northwind round 1\n\nEvery outcome is live on staging since mid-2026.\n'
            self.assertEqual(run(tmp, 'clean.md', clean, 'hygiene').returncode, 0)
            for label in ('cas-ddb0', 'cas-9060', 'cas-1a2b3', 'PROJ-123', 'ENG-1234', 'proj-123', 'eng-1234', 'abc-12f4', 'ISO-123', 'ISO-123456'):
                with self.subTest(label=label):
                    result = run(tmp, 'planted.md', clean + f'The SHA-256 export shipped in {label}.\n', 'hygiene')
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                    self.assertIn(f"planted.md:4: ticket label '{label}'", result.stdout)

    def test_hygiene_accepts_client_prose_and_dates(self):
        with tempfile.TemporaryDirectory() as tmp:
            for prose in ('Self-feed and self-feed work since mid-2026.',
                          'COVID-19', 'SHA-256', 'UTF-8', 'ISO-9001', 'ISO-27001', 'GPT-4',
                          'Q3-2026', 'FY-2026', 'FY-26', 'SOC-2', 'ASC-606'):
                with self.subTest(prose=prose):
                    result = run(tmp, 'clean.md', f'# Northwind round 1\n\n{prose}\n', 'hygiene')
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_login_requires_a_success_url_before_reading_credentials_or_launching_a_browser(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / 'shots.json').write_text('[]')
            result = subprocess.run(['node', str(CAPTURE), '--shots', str(Path(tmp) / 'shots.json'),
                                     '--base', 'https://example.invalid', '--out', tmp,
                                     '--login', '--credentials', str(Path(tmp) / 'not-a-credential-file')],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn('login failed:', result.stderr)
            self.assertIn('--success-url', result.stderr)
            self.assertFalse((Path(tmp) / 'manifest.json').exists())


if __name__ == '__main__':
    unittest.main()
