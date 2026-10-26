#!/usr/bin/env python3
"""Behavioral checks for the portable release renderer: python3 scripts/test-release-report.py."""
import contextlib
import importlib.util
import io
import re
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
        # This repo's root DESIGN.md supplies the tokens: hub variables behind the
        # report roles, Commander's faces inlined, the brand mark, no self-alias.
        self.assertIn('--verdict:var(--color-verdict)', output)
        self.assertEqual(output.count('url("data:font/woff2;base64,'), 2)
        self.assertIn('<img class="mark" src="data:image/svg+xml;base64,', output)
        self.assertNotIn('--ink:var(--ink)', output)
        # Print is the light set without the beforeprint hook: dark blocks are screen-only.
        self.assertIn('@media screen and (prefers-color-scheme: dark)', output)
        self.assertNotRegex(output, r'@media\s*\(\s*prefers-color-scheme\s*:\s*dark')

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
            # A DESIGN.md source that would reach the network falls back, with a warning.
            (root / 'DESIGN.md').write_text('---\nsource: [tokens.css]\nmaps:\n  bg: "--paper"\n---\n')
            for remote in ('--art:url(https://example.org/a.png)', '}@import "https://example.org/t.css";:root{'):
                (root / 'tokens.css').write_text(':root{--paper:#fff;' + remote + '}')
                warning = io.StringIO()
                with self.subTest(remote=remote), contextlib.redirect_stderr(warning):
                    output = renderer.render(EXEMPLAR, project_root=root)
                    self.assertIn('--verdict:#123456', output)
                    self.assertNotIn('example.org/a.png', output)
                    self.assertIn('warning: DESIGN.md CSS sources may hold only tokens', warning.getvalue())

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

        # Later reconciliation belongs in prose, outside the counted table.
        source = BY_AREA.replace(
            '| [#1215](https://example.org/issues/1215) | Peer report | 2026-10-16T13:03:40Z | Sessions |\n'
            '| [#1222](https://example.org/issues/1222) | Receipts | 2026-10-16T13:03:41Z | Tasks and merges |',
            '\nLater reconciliation: [#1215](https://example.org/issues/1215) closed after the counting window.')
        source = source.replace('| GitHub issues closed | 2 |', '| GitHub issues closed | 0 |')
        output = renderer.render(source)
        hero = output[output.index('data-section="hero"'):output.index('data-section="glance"')]
        self.assertEqual(hero.count('data-change="'), 3)
        self.assertIn('data-total="3"', hero)
        self.assertIn('No GitHub issues are counted in this report', hero)
        self.assertNotIn('No GitHub issues were closed with this release.', output)
        self.assertIn('GitHub issues closed with this release · 0', output)
        self.assertIn('closed after the counting window.', output)

    def test_eight_changes_draw_on_one_row(self):
        titles = '; '.join(f'Change {n}' for n in range(1, 9))
        source = BY_AREA.replace('| Sessions | 2 | Typing during cleanup; Peer ownership board |',
                                 f'| Sessions | 8 | {titles} |').replace('| Total | 3 |', '| Total | 9 |')
        output = renderer.render(source)
        group = output[output.index('data-area="Sessions"'):output.index('data-area="Tasks and merges"')]
        self.assertEqual(len(set(re.findall(r'<circle cx="\d+" cy="(\d+)"', group))), 1)

    def test_change_with_before_and_after_captures_shows_the_pair(self):
        with tempfile.TemporaryDirectory() as tmp:
            for side in ('before', 'after'):
                (Path(tmp) / f'{side}.png').write_bytes(b'\x89PNG\r\n\x1a\n' + side.encode())
            source = BY_AREA.replace('Now: it does not.\n', 'Now: it does not.\n\nBefore: before.png\n\nAfter: after.png\n')
            output = renderer.render(source, project_root=Path(tmp))
        card = output[output.index('<article class="change"><h4>Typing during cleanup'):]
        card = card[:card.index('</article>')]
        self.assertEqual(re.findall(r'<img src="data:image/png;base64,[^"]+" alt="(\w+): Typing during cleanup"', card), ['Before', 'After'])

    def test_change_without_a_real_pair_shows_no_captures(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / 'after.png').write_bytes(b'\x89PNG\r\n\x1a\nafter')
            for lone in ('Now: it does not.\n', 'Now: it does not.\n\nAfter: after.png\n'):
                warning = io.StringIO()
                with self.subTest(lone=lone), contextlib.redirect_stderr(warning):
                    output = renderer.render(BY_AREA.replace('Now: it does not.\n', lone), project_root=Path(tmp))
                    self.assertNotIn('class="captures"', output)
                    self.assertNotIn('<img src="data:image/png', output)
                    self.assertEqual('After:' in lone, 'needs both Before: and After:' in warning.getvalue())

    def test_changes_by_area_rejects_zero_rows_miscounts_and_repeated_issues(self):
        row = '| Sessions | 2 | Typing during cleanup; Peer ownership board |'
        for name, bad in {
            'zero-change row': BY_AREA.replace(row, row + '\n| Commander | 0 | Nothing |'),
            'miscount': BY_AREA.replace('| Sessions | 2 |', '| Sessions | 3 |'),
            'repeated ledger issue': BY_AREA.replace('[#1222]', '[#1215]'),
        }.items():
            with self.subTest(name), self.assertRaises(ValueError):
                renderer.render(bad)



@unittest.skipUnless(importlib.util.find_spec('pymupdf'), 'check-pdf needs PyMuPDF')
class PdfCheckTests(unittest.TestCase):
    def test_empty_assets_header_requires_its_first_row_on_the_same_page(self):
        """A printable PDF may pass bounds while leaving a table header alone."""
        import pymupdf, subprocess, sys
        check = ROOT / 'cas-cli/src/builtins/skills/cas-release-report/scripts/check-pdf.py'

        with tempfile.TemporaryDirectory() as tmp:
            for orphaned in (True, False):
                pdf = pymupdf.open()
                first = pdf.new_page()
                first.insert_text((60, 80), 'Install')
                header_page = first if orphaned else pdf.new_page()
                header_page.insert_text((60, 730 if orphaned else 80), 'Release assets')
                header_page.insert_text((60, 750 if orphaned else 100), 'Asset       Digest       Size')
                row_page = pdf.new_page() if orphaned else header_page
                row_page.insert_text((60, 80 if orphaned else 120),
                                     'No published assets       unavailable       unavailable')
                path = Path(tmp) / 'report.pdf'
                pdf.save(path)
                result = subprocess.run([sys.executable, str(check), str(path), tmp],
                                        capture_output=True, text=True)
                if orphaned:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('assets header', result.stderr)
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)

    def test_footer_only_last_page_is_refused(self):
        """GH #1305: a last page holding only the closing folio stays inside the
        bounds yet prints an empty sheet; check-pdf must refuse it."""
        import pymupdf, subprocess, sys
        check = ROOT / 'cas-cli/src/builtins/skills/cas-release-report/scripts/check-pdf.py'

        def run(last_page_body):
            with tempfile.TemporaryDirectory() as tmp:
                pdf = pymupdf.open()
                pdf.new_page().insert_text((60, 80), 'Evidence and scope')
                last = pdf.new_page()
                if last_page_body:
                    last.insert_text((60, 80), last_page_body)
                last.insert_text((60, 120), 'Cassy v9.99.1 / Release report   Markdown source · Print edition')
                last.insert_text((290, 820), '2 / 2')
                pdf.save(Path(tmp) / 'report.pdf')
                return subprocess.run([sys.executable, str(check), str(Path(tmp) / 'report.pdf'), tmp],
                                      capture_output=True, text=True)

        refused = run('')
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn('closing folio', refused.stderr)
        self.assertEqual(run('Source: release assets.').returncode, 0)
        # The closing folio, not an evidence line that also names the source, is the anchor.
        self.assertEqual(run('Markdown source: the v9.99.1 CHANGELOG section.').returncode, 0)

    def test_local_file_links_are_refused_cas_692a(self):
        """cas-692a: a link into the generating checkout is dead for every reader."""
        import pymupdf, subprocess, sys
        check = ROOT / 'cas-cli/src/builtins/skills/cas-release-report/scripts/check-pdf.py'

        def run(link):
            with tempfile.TemporaryDirectory() as tmp:
                pdf = pymupdf.open()
                page = pdf.new_page()
                page.insert_text((60, 80), 'Evidence and scope. Markdown source')
                page.insert_link({'from': pymupdf.Rect(60, 70, 200, 85), **link})
                pdf.save(Path(tmp) / 'report.pdf')
                return subprocess.run([sys.executable, str(check), str(Path(tmp) / 'report.pdf'), tmp],
                                      capture_output=True, text=True)

        # Chromium prints a relative link as a launch link to the local file.
        launch = run({'kind': pymupdf.LINK_LAUNCH, 'file': '/home/x/.cas/worktrees/w/docs/release-reports/v9.99.1.md'})
        self.assertNotEqual(launch.returncode, 0)
        self.assertIn('local file link', launch.stderr)
        uri = run({'kind': pymupdf.LINK_URI, 'uri': 'file:///home/x/docs/release-reports/v9.99.1.pdf'})
        self.assertNotEqual(uri.returncode, 0)
        published = 'https://github.com/o/r/blob/main/docs/release-reports/v9.99.1.md'
        self.assertEqual(run({'kind': pymupdf.LINK_URI, 'uri': published}).returncode, 0)

if __name__ == '__main__':
    unittest.main()
