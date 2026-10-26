# PDF rendering and verification

## Render

`cas release report <version> --pdf` finalizes the report through
`cas deliverable finalize`: Playwright's Chromium prints A4 and Letter in print
media and light scheme, with disclosures expanded, network requests blocked
(a blocked request fails the run) and 15 mm margins; both are checked below,
and the A4 copy is written as `v<version>.pdf`. Its running folio prints the
document title and "n / N" in the page's `--font-mono` stack and `--ink-muted`
colour. If Playwright is not importable it installs it into a disposable
workspace.

The template declares margins, not a fixed `@page size`. Print CSS uses
`break-before:auto` for sections and `break-inside:avoid` for cards, rows and
figures. Keep a table header with its first row while allowing later rows to
paginate. Repeat table headers, expand disclosures, wrap long commands/hashes,
print URL destinations and keep headings with their following content. Never fix
short pages by shrinking the whole document or cropping text.

## Render every page back to PNG and check bounds

`finalize` runs the PDF check on every printed format and fails on a finding.
To keep its page PNGs for inspection, finalize the report's HTML yourself:

```bash
cas deliverable finalize docs/release-reports/v<version>.html --audience internal \
  --out <artifacts_root>/<project-key>/<task-id>/v<version>.html --pdf a4,letter \
  --check-dir <artifacts_root>/<project-key>/<task-id>/pdf
```

The check needs PyMuPDF (`import pymupdf`). It writes one PNG per page and a
`receipt.json` with the page count, out-of-bounds text blocks, a
`footer_only_last_page` flag, `orphaned_assets_headers` pages, link targets and
`local_links`. Text in the bottom margin counts as the running folio only when
it is the PDF's title and/or "n / N". It exits non-zero on a bounds finding,
an empty assets header separated from its row, a footer-only last page, or a
link to a local file (a `file://` target or a checkout path), which no reader
of the published PDF can open. `cas release report` names the source and print edition by their GitHub
URLs on `main` when a GitHub repository is known; reports are committed after
their release tag is created. Resolve the repository from `issues.repo`, falling
back to the project's GitHub `origin`. The template keeps
that folio inside the final section so print never starts a page with it.

Inspect every PNG, including the final page; contact sheets aid scanning but do
not replace reading suspicious full-size pages. Check orphan headings, repeated
headers, split cards/rows, clipped SVG/text, missing disclosures and unexplained
half-empty pages. Compare extracted text and URI annotations against the Markdown:
all issue links, complete asset hashes and copyable install commands must survive.
Check for missing/duplicated words across page boundaries. Record the page count,
bounds receipt and the visual inspection in the brief. A passing bounds script
alone does not prove correct pagination or fidelity.
