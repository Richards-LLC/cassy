#!/usr/bin/env python3
"""Render every page of a release-report PDF to PNG and check printable bounds.

Usage: python3 check-pdf.py REPORT.pdf ARTIFACT_DIR
Needs PyMuPDF (`pip install pymupdf`). Writes one PNG per page and receipt.json
into ARTIFACT_DIR, prints the receipt, and exits non-zero on a bounds finding.
"""
import json, sys
from pathlib import Path
import pymupdf
pdf = pymupdf.open(sys.argv[1])
output = Path(sys.argv[2]); output.mkdir(parents=True, exist_ok=True)
findings, links = [], []
for number, page in enumerate(pdf, 1):
    page.get_pixmap(matrix=pymupdf.Matrix(1.5, 1.5)).save(output / f'{number:02d}.png')
    links.extend(link['uri'] for link in page.get_links() if 'uri' in link)
    for x0, y0, x1, y1, text, *_ in page.get_text('blocks'):
        if not text.strip():
            continue
        # 15mm margins (~42.5pt), with tolerance; folio lives in bottom margin.
        folio = y0 > page.rect.height - 40 and text.strip().replace('/', '').replace(' ', '').isdigit()
        if not folio and (x0 < 40 or y0 < 40 or x1 > page.rect.width - 40 or y1 > page.rect.height - 40):
            findings.append({'page': number, 'bounds': [x0,y0,x1,y1], 'text': text[:100]})
receipt = {'pages': len(pdf), 'bounds_findings': findings, 'links': links}
(output / 'receipt.json').write_text(json.dumps(receipt, indent=2))
print(json.dumps(receipt))
if findings:
    sys.exit('PDF content exceeds printable bounds')
