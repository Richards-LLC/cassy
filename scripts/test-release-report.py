#!/usr/bin/env python3
"""Behavioral checks for the portable release renderer: python3 scripts/test-release-report.py."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location('release_renderer', ROOT / 'cas-cli/src/builtins/skills/cas-release-report/scripts/render.py')
renderer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(renderer)
EXEMPLAR = (ROOT / 'docs/release-reports/2026-09-08-v3.19.0.md').read_text()


class ReleaseRendererTests(unittest.TestCase):
    def test_exemplar_accounts_for_every_closure_and_change(self):
        output = renderer.render(EXEMPLAR, project_root=ROOT)
        self.assertEqual(output.count('data-issue="'), 21)
        self.assertEqual(output.count('<article class="change">'), 36)
        self.assertIn('data-total="21"', output)
        self.assertIn('break-before:auto', output)
        self.assertNotIn('break-before:page', output)
        self.assertNotIn('{{', output)

    def test_different_project_and_partial_tokens(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'design-tokens.json').write_text('{"color":{"light":{"verdict":{"$value":"#123456"}}}}')
            output = renderer.render(EXEMPLAR.replace('Cassy', 'Orchard').replace('v3.19.0', 'v9.99.1'), project_root=root)
            self.assertIn('Orchard v9.99.1', output)
            self.assertNotIn('Cassy', output)
            self.assertIn('--verdict:#123456', output)
            self.assertIn('--bg:#F7F4EE', output)

    def test_bad_totals_and_duplicate_issues_fail(self):
        for source in (EXEMPLAR.replace('| Total | 21 |', '| Total | 22 |'),
                       EXEMPLAR.replace('#705, #746', '#705, #705'),
                       EXEMPLAR.replace('| Factory | 4 |', '| Factory | 3 |')):
            with self.subTest(source=source[:30]), self.assertRaises(ValueError):
                renderer.render(source)

    def test_ledger_disagreement_fails(self):
        source = EXEMPLAR.replace('[#705]', '[#99999]')
        self.assertNotEqual(source, EXEMPLAR)
        with self.assertRaisesRegex(ValueError, 'fixes ledger'):
            renderer.render(source)

    def test_unsafe_links_and_html_are_not_executable(self):
        with self.assertRaises(ValueError):
            renderer.inline('[open](javascript:alert)')
        self.assertEqual(renderer.inline('<script>'), '&lt;script&gt;')
        self.assertEqual(renderer.inline('[safe](https://example.org/?a=1&b=2)'),
                         '<a href="https://example.org/?a=1&amp;b=2">safe</a>')

    def test_section_loss_fails(self):
        with self.assertRaisesRegex(ValueError, 'expected sections'):
            renderer.render(EXEMPLAR.replace('## Evidence and scope', '## Missing'))

    def test_empty_release_has_no_invented_closures(self):
        source = '''# Orchard v9.99.1

Published today

Installation is simpler.

## Change map

Install improved without issue closures.

Dots count closed issues, not impact.

| Surface | Issues closed | Issue numbers |
| --- | ---: | --- |
| Install | 0 | No listed issue |
| Total | 0 | No closures |

Source: release assets.

## Release at a glance

| Measure | Value | Definition |
| --- | ---: | --- |
| Issues | 0 | Verified closures |

No latency evidence available.

## What you can do now

No user changes.

## Under the hood

No developer changes.

## Fixes ledger

| Issue | Title |
| --- | --- |

## Install

Use the release archive.

## Evidence and scope

Source: release assets.
'''
        output = renderer.render(source)
        self.assertIn('data-total="0"', output)
        self.assertNotIn('data-issue="', output)
        self.assertIn('No user changes.', output)


if __name__ == '__main__':
    unittest.main()
