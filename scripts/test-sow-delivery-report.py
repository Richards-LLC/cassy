#!/usr/bin/env python3
"""Gate checks of cas-sow-delivery-report: python3 scripts/test-sow-delivery-report.py."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
CHECK = ROOT / 'cas-cli/src/builtins/skills/cas-sow-delivery-report/scripts/check-sow.py'
# Client hygiene moved into the deliverables core (`cas deliverable finalize`).
SPEC = importlib.util.spec_from_file_location('deliverable_hygiene', ROOT / 'cas-cli/src/deliverable/assets/hygiene.py')
hygiene = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(hygiene)
CAPTURE = ROOT / 'cas-cli/src/deliverable/assets/capture.mjs'
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
        clean = '# Northwind round 1\n\nEvery outcome is live on staging since mid-2026.\n'
        self.assertEqual(hygiene.scan(clean, 'customer'), [])
        for label in ('cas-ddb0', 'cas-9060', 'cas-1a2b3', 'PROJ-123', 'ENG-1234', 'proj-123', 'eng-1234', 'abc-12f4', 'ISO-123', 'ISO-123456'):
            with self.subTest(label=label):
                hits = hygiene.scan(clean + f'The SHA-256 export shipped in {label}.\n', 'customer')
                self.assertEqual([(line, kind, match) for line, kind, match, _ in hits], [(4, 'ticket label', label)])

    def test_hygiene_accepts_client_prose_and_dates(self):
        for prose in ('Self-feed and self-feed work since mid-2026.',
                      'COVID-19', 'SHA-256', 'UTF-8', 'ISO-9001', 'ISO-27001', 'GPT-4',
                      'Q3-2026', 'FY-2026', 'FY-26', 'SOC-2', 'ASC-606'):
            with self.subTest(prose=prose):
                self.assertEqual(hygiene.scan(f'# Northwind round 1\n\n{prose}\n', 'customer'), [])

    def test_catalog_budget_fails_before_credentials_or_browser(self):
        with tempfile.TemporaryDirectory() as tmp:
            plan = Path(tmp) / 'plan.json'
            plan.write_text(json.dumps({'root': tmp, 'config': {
                'max_shots': 6, 'viewports': ['1280x800'], 'mask': []},
                'shots': [{'id': 'R1', 'path': '/', 'viewport': [9999, 9999]}],
                'credentials_file': str(Path(tmp) / 'absent')}))
            alias = Path(tmp) / 'assets'
            alias.symlink_to(CAPTURE.parent, target_is_directory=True)
            for script, flags in [(CAPTURE, []), (alias / CAPTURE.name, []),
                                  (alias / CAPTURE.name, ['--preserve-symlinks-main'])]:
                with self.subTest(script=script, flags=flags):
                    result = subprocess.run(['node', *flags, str(script), str(plan), '--validate'],
                                            capture_output=True, text=True)
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertEqual(json.loads(result.stdout)['code'], 'capture-catalog')
            self.assertFalse((Path(tmp) / 'pairs.json').exists())

    def test_missing_mask_names_selector_and_sides_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            # Playwright is the boundary: no browser is needed to prove the
            # real capture entry point refuses before taking an unmasked shot.
            module = Path(tmp) / 'playwright.cjs'
            module.write_text("""
const page = {
  setDefaultTimeout() {}, setDefaultNavigationTimeout() {},
  async setViewportSize() {}, async emulateMedia() {}, async goto() {},
  url: () => 'http://localhost/', async content() { return '<p>fixture</p>'; },
  locator: () => ({ async count() { return 0; } }),
  async screenshot() { throw new Error('unmasked screenshot attempted'); }
};
module.exports = { chromium: { async launch() { return {
  async newContext() { return { async newPage() { return page; }, async close() {} }; },
  async close() {}
}; } } };
""")
            plan = Path(tmp) / 'plan.json'
            plan.write_text(json.dumps({'root': tmp, 'out': tmp, 'remaining_ms': 10000,
                'config': {'max_shots': 6, 'viewports': ['1280x800'], 'mask': ['.private'],
                           'max_html_mb': 8, 'login_path': ''},
                'shots': [{'id': 'goal', 'path': '/'}],
                'phases': [{'side': side, 'base': 'http://localhost/', 'ref': side}
                           for side in ['before', 'after']]}))
            result = subprocess.run(['node', str(CAPTURE), str(plan)], capture_output=True,
                                    text=True, cwd=tmp, env={**os.environ, 'PLAYWRIGHT_MODULE': str(module)})
            self.assertEqual(result.returncode, 0, result.stderr)
            receipt = json.loads(result.stdout)
            self.assertEqual(receipt['captures'], [])
            self.assertEqual(len(receipt['failures']), 1, receipt)
            finding = receipt['failures'][0]
            self.assertEqual(finding['code'], 'capture-mask')
            for token in ['.private', 'before', 'after']:
                self.assertEqual(finding['message'].count(token), 1, finding)


if __name__ == '__main__':
    unittest.main()
