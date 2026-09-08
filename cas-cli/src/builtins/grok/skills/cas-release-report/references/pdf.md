# PDF rendering and verification

Run this recipe from the project with Playwright available to Node and its
Chromium browser installed. Save build helpers in the worktree and page images
under the task's artifact directory. The final HTML has no runtime dependency.

## Render both formats

Save as `render-pdf.mjs`; run `node render-pdf.mjs REPORT.html OUTPUT_DIR`.
Use a pinned project Playwright dependency when present. If it is unavailable,
install Playwright and Chromium in the project's disposable build directory.
Record `node --version`, Playwright version and `browser.version()` in the receipt.

```javascript
import { chromium } from 'playwright';
import { mkdir } from 'node:fs/promises';
import { resolve, join, basename } from 'node:path';
import { pathToFileURL } from 'node:url';
const [input, output] = process.argv.slice(2);
if (!input || !output) throw new Error('expected REPORT.html OUTPUT_DIR');
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true });
try {
  console.log('Chromium', browser.version());
  const page = await browser.newPage();
  await page.route(/^https?:/, route => route.abort());
  await page.goto(pathToFileURL(resolve(input)).href, { waitUntil: 'load' });
  await page.emulateMedia({ media: 'print', colorScheme: 'light' });
  await page.evaluate(async () => {
    await document.fonts.ready;
    document.querySelectorAll('details').forEach(panel => { panel.open = true; });
  });
  for (const format of ['A4', 'Letter']) {
    await page.pdf({
      path: join(output, `${basename(input, '.html')}-${format}.pdf`),
      format, printBackground: true, preferCSSPageSize: false,
      margin: { top: '15mm', right: '15mm', bottom: '15mm', left: '15mm' },
      displayHeaderFooter: true, headerTemplate: '<span></span>',
      footerTemplate: '<div style="font:8px sans-serif;width:100%;text-align:center;color:#555">'
        + '<span class="pageNumber"></span> / <span class="totalPages"></span></div>',
    });
  }
} finally { await browser.close(); }
```

The template declares margins, not a fixed `@page size`; the recipe selects A4
or Letter. Print CSS uses `break-before:auto` for sections and `break-inside:avoid`
for cards, rows and figures. Repeat table headers, expand disclosures, wrap long
commands/hashes, print URL destinations and keep headings with their following
content. Never fix short pages by shrinking the whole document or cropping text.

## Render every page back to PNG and check bounds

With PyMuPDF installed, save as `check-pdf.py` and run
`python3 check-pdf.py REPORT-A4.pdf ARTIFACT_DIR/a4` (repeat for Letter).

```python
import json, sys
from pathlib import Path
import fitz
pdf = fitz.open(sys.argv[1])
output = Path(sys.argv[2]); output.mkdir(parents=True, exist_ok=True)
findings, links = [], []
for number, page in enumerate(pdf, 1):
    page.get_pixmap(matrix=fitz.Matrix(1.5, 1.5)).save(output / f'{number:02d}.png')
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
assert not findings, 'PDF content exceeds printable bounds'
```

Inspect every PNG, including the final page; contact sheets aid scanning but do
not replace reading suspicious full-size pages. Check orphan headings, repeated
headers, split cards/rows, clipped SVG/text, missing disclosures and unexplained
half-empty pages. Compare extracted text and URI annotations against the Markdown:
all issue links, complete asset hashes and copyable install commands must survive.
Check for missing/duplicated words across page boundaries. Record both page counts,
bounds receipts and the visual inspection in the brief. A passing bounds script
alone does not prove correct pagination or fidelity. Copy the chosen format to
`docs/release-reports/<version>.pdf`; keep the second format as review evidence.
