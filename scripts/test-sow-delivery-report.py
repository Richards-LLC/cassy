#!/usr/bin/env python3
"""Gate checks of cas-sow-delivery-report: python3 scripts/test-sow-delivery-report.py."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

CHECK = Path(__file__).resolve().parent.parent / 'cas-cli/src/builtins/skills/cas-sow-delivery-report/scripts/check-sow.py'
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
            result = run(tmp, 'planted.md', clean + 'The export shipped in cas-ddb0.\n', 'hygiene')
            self.assertEqual(result.returncode, 1)
            self.assertIn("planted.md:4: ticket label 'cas-ddb0'", result.stdout)


if __name__ == '__main__':
    unittest.main()
