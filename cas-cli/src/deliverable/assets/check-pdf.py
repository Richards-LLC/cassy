#!/usr/bin/env python3
"""Render every page of a release-report PDF to PNG and check printable bounds.

Usage: python3 check-pdf.py REPORT.pdf ARTIFACT_DIR
Needs PyMuPDF (`pip install pymupdf`). Writes one PNG per page and receipt.json
into ARTIFACT_DIR, prints the receipt, and exits non-zero on a bounds finding,
a footer-only last page, an empty assets table header without its row, or a
link to a local file.
"""
import json, re, sys
from pathlib import Path
import pymupdf
pdf = pymupdf.open(sys.argv[1])
title = ' '.join((pdf.metadata or {}).get('title', '').split())


def running_folio(text):
    """The footer the PDF printer draws in the bottom margin: the document
    title and/or "n / N"."""
    rest = ' '.join(re.sub(r'\d+\s*/\s*\d+\s*$', '', text.strip()).split()).rstrip('.…')
    return not rest or bool(title) and title.startswith(rest)

output = Path(sys.argv[2]); output.mkdir(parents=True, exist_ok=True)
findings, links, files = [], [], []
page_texts = []
for number, page in enumerate(pdf, 1):
    page_texts.append(page.get_text())
    page.get_pixmap(matrix=pymupdf.Matrix(1.5, 1.5)).save(output / f'{number:02d}.png')
    links.extend(link['uri'] for link in page.get_links() if 'uri' in link)
    # Chromium prints a relative href as a launch link naming the local file.
    files.extend(link['file'] for link in page.get_links() if link.get('file'))
    for x0, y0, x1, y1, text, *_ in page.get_text('blocks'):
        if not text.strip():
            continue
        # 15mm margins (~42.5pt), with tolerance; folio lives in bottom margin.
        folio = y0 > page.rect.height - 40 and running_folio(text)
        if not folio and (x0 < 40 or y0 < 40 or x1 > page.rect.width - 40 or y1 > page.rect.height - 40):
            findings.append({'page': number, 'bounds': [x0,y0,x1,y1], 'text': text[:100]})
# GH #1305: a last page holding only the closing folio stays inside the bounds
# but prints an empty sheet. Report content must appear above the folio.
blocks = [block for block in pdf[-1].get_text('blocks') if block[4].strip()]
closing = [block[1] for block in blocks if 'Markdown source' in block[4]]
# Anchor on the folio itself, the last match: an evidence paragraph may also
# open with "Markdown source:" higher on the page.
footer_only = len(pdf) > 1 and bool(closing) and not any(block[3] <= max(closing) - 2 for block in blocks)
# A file:// target, or one into a Cassy worktree, is dead for
# every reader of the published PDF.
local_links = files + [uri for uri in links if uri.lower().startswith('file:') or '/.cas/worktrees/' in uri]
has_empty_assets_row = any('No published assets' in text for text in page_texts)
orphaned_assets_headers = [number for number, text in enumerate(page_texts, 1)
    if has_empty_assets_row
    and re.search(r'\bRelease assets\s+ASSET\s+DIGEST\s+SIZE\b', text, re.I)
    and 'No published assets' not in text]
receipt = {'pages': len(pdf), 'bounds_findings': findings, 'footer_only_last_page': footer_only,
           'links': links, 'local_links': local_links,
           'orphaned_assets_headers': orphaned_assets_headers}
(output / 'receipt.json').write_text(json.dumps(receipt, indent=2))
print(json.dumps(receipt))
if findings:
    sys.exit('PDF content exceeds printable bounds')
if footer_only:
    sys.exit('PDF ends on a page holding only the closing folio')
if local_links:
    sys.exit('PDF holds a local file link no reader can open: ' + ', '.join(local_links))
if orphaned_assets_headers:
    sys.exit('PDF leaves the empty assets header without its first row on page(s): '
             + ', '.join(map(str, orphaned_assets_headers)))
