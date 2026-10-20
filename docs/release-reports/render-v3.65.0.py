#!/usr/bin/env python3
"""Render this edition from its Markdown with a source-owned hero caption.

The shared renderer equates an empty counting-window ledger with no release
closures. This edition has later reconciliation, so use its Markdown reading
note verbatim. Keep the fixed template and data-section/map contracts intact.
"""
from html import escape
import importlib.util
from pathlib import Path
import re

root = Path(__file__).resolve().parents[2]
source = Path(__file__).with_name('v3.65.0.md')
spec = importlib.util.spec_from_file_location('release_renderer', root / 'cas-cli/src/builtins/skills/cas-release-report/scripts/render.py')
renderer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(renderer)
markdown = source.read_text()
caption = re.search(r'## Change map\s+[^\n]+\n\n([^\n]+)\n\n\| Area', markdown).group(1)
html = renderer.render(markdown, project_root=root, source_href=source.name, pdf_href='v3.65.0.pdf')
old = 'One dot, one shipped change. No GitHub issues were closed with this release.'
assert html.count(old) == 1, 'review shared renderer changes before re-rendering this edition'
source.with_suffix('.html').write_text(html.replace(old, escape(caption)))
