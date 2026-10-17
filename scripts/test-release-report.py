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
BY_AREA = '''---
version: 9.99.1
date: 2026-10-16
areas: ["Sessions", "Tasks and merges"]
---

# Orchard v9.99.1

Published 16 October 2026 · 13:04 UTC

Typing no longer competes with cleanup.

## Change map

3 changes shipped: 2 in Sessions and 1 in Tasks and merges.

One dot is one change listed in the v9.99.1 CHANGELOG. Dots count changes, not their size or importance.

| Area | Changes | What changed |
| --- | ---: | --- |
| Sessions | 2 | Typing during cleanup; Peer ownership board |
| Tasks and merges | 1 | Loaded-proof receipts |
| Total | 3 | 2 areas |

Source: CHANGELOG.

## Release at a glance

| Measure | Value | Definition |
| --- | ---: | --- |
| Changes shipped | 3 | 1 added, 2 fixed |
| GitHub issues closed | 2 | Closed from PR #1 opening to 15 min after merge |

No manual interventions.

## What you can do now

### Sessions

#### Typing during cleanup

Was: typing could stall.

Now: it does not.

## Under the hood

No developer changes.

## Fixes ledger

| Issue | Title | Closed | Area |
| --- | --- | --- | --- |
| [#1215](https://example.org/issues/1215) | Peer report | 2026-10-16T13:03:40Z | Sessions |
| [#1222](https://example.org/issues/1222) | Receipts | 2026-10-16T13:03:41Z | Tasks and merges |

## Install

Use the release archive.

## Evidence and scope

Source: release assets.
'''


class ReleaseRendererTests(unittest.TestCase):
    def test_exemplar_accounts_for_every_closure_and_change(self):
        output = renderer.render(EXEMPLAR, project_root=ROOT)
        self.assertEqual(output.count('data-issue="'), 21)
        self.assertEqual(output.count('<article class="change">'), 36)
        self.assertIn('data-total="21"', output)
        self.assertIn('break-before:auto', output)
        self.assertNotIn('break-before:page', output)
        self.assertNotIn('{{', output)

    def test_publication_over_budget_row_survives_html_rendering(self):
        source = EXEMPLAR.replace("| Green to published |",
            "| Tag to published | 15m 8s — over budget (10m 0s budget) | First tag workflow to publication |\n| Green to published |")
        output = renderer.render(source, project_root=ROOT)
        self.assertIn("15m 8s — over budget (10m 0s budget)", output)
        self.assertIn("Tag to published", output)

    def test_intervention_count_and_unknown_survive_html_rendering(self):
        for value in ("8", "0", "unavailable"):
            source = EXEMPLAR.replace("| Green to published |",
                f"| Manual interventions | {value} | Distinct rescued stages plus recorded hand fixes |\n| Green to published |")
            output = renderer.render(source, project_root=ROOT)
            self.assertIn(f'<span class="stat-value">{value}</span><span class="stat-label">Manual interventions</span>', output)
            self.assertIn("Manual interventions", output)

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
        # cas-1a83: `cas release report` writes Evidence URLs as autolinks.
        self.assertEqual(renderer.inline('PR: <https://example.org/pull/9?a=1&b=2>.'),
                         'PR: <a href="https://example.org/pull/9?a=1&amp;b=2">'
                         'https://example.org/pull/9?a=1&amp;b=2</a>.')
        self.assertEqual(renderer.inline('<javascript:alert(1)>'), '&lt;javascript:alert(1)&gt;')

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
        self.assertNotIn('data-surface="Install"', output)
        self.assertNotIn('no listed issues', output)
        self.assertIn('No user changes.', output)

    def test_changes_by_area_lead_page_one_with_issues_secondary(self):
        output = renderer.render(BY_AREA)
        hero = output[output.index('data-section="hero"'):output.index('data-section="glance"')]
        self.assertEqual(hero.count('data-change="'), 3)
        self.assertIn('data-area="Sessions" data-count="2"', hero)
        self.assertIn('<title>Typing during cleanup</title>', hero)
        self.assertIn('2 GitHub issues closed with this release', hero)
        self.assertNotIn('data-issue="', hero)
        self.assertIn('GitHub issues closed with this release · 2', output)
        glance = output[output.index('data-section="glance"'):output.index('data-section="users"')]
        self.assertLess(glance.index('id="map-data"'), glance.index('class="stats"'))
        for jargon in ('issue-bearing', 'verified closure', 'closure stitch', 'no listed issues'):
            self.assertNotIn(jargon, output.lower())

    def test_changes_by_area_rejects_zero_rows_miscounts_and_repeated_issues(self):
        row = '| Sessions | 2 | Typing during cleanup; Peer ownership board |'
        for name, bad in {
            'zero-change row': BY_AREA.replace(row, row + '\n| Commander | 0 | Nothing |'),
            'miscount': BY_AREA.replace('| Sessions | 2 |', '| Sessions | 3 |'),
            'repeated ledger issue': BY_AREA.replace('[#1222]', '[#1215]'),
        }.items():
            with self.subTest(name), self.assertRaises(ValueError):
                renderer.render(bad)


if __name__ == '__main__':
    unittest.main()
