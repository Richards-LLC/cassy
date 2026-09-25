# PDF rendering and verification

## Render

`cas release report <version> --pdf` renders the PDF with Playwright's Chromium in
print media and light scheme, with disclosures expanded, network requests blocked
and 15 mm margins, and writes the A4 copy as `v<version>.pdf`. If Playwright is not
importable it installs it into a disposable workspace. Without the CLI, print the
HTML from Chromium with the same settings.

The template declares margins, not a fixed `@page size`. Print CSS uses
`break-before:auto` for sections and `break-inside:avoid` for cards, rows and
figures. Repeat table headers, expand disclosures, wrap long commands/hashes,
print URL destinations and keep headings with their following content. Never fix
short pages by shrinking the whole document or cropping text.

## Render every page back to PNG and check bounds

```bash
python3 <skills-dir>/cas-release-report/scripts/check-pdf.py \
  docs/release-reports/v<version>.pdf <artifacts_root>/<task-id>/pdf
```

The script needs PyMuPDF (`import pymupdf`). It writes one PNG per page and a
`receipt.json` with the page count, out-of-bounds text blocks and link targets,
and exits non-zero on a bounds finding.

Inspect every PNG, including the final page; contact sheets aid scanning but do
not replace reading suspicious full-size pages. Check orphan headings, repeated
headers, split cards/rows, clipped SVG/text, missing disclosures and unexplained
half-empty pages. Compare extracted text and URI annotations against the Markdown:
all issue links, complete asset hashes and copyable install commands must survive.
Check for missing/duplicated words across page boundaries. Record the page count,
bounds receipt and the visual inspection in the brief. A passing bounds script
alone does not prove correct pagination or fidelity.
