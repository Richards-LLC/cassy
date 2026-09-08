# Release-report template verification

The reusable builtin `cas-release-report` was exercised against the approved
v3.19.0 Markdown using its generic Python renderer and bundled HTML template.
The historical exemplar remains unchanged. Durable evidence:
`/home/pippenz/.cas/artifacts/cas-8719/`.

- `python3 scripts/test-release-report.py`: 7 passed. Covers exemplar accounting,
  a second project with partial tokens, totals, duplicate/missing issues, unsafe
  links, missing sections and a release with zero issue closures.
- `node scripts/visual-qa.mjs .report-build/v3.19.0.html --strict`: exit 0 PASS,
  zero findings/allowlists at 1280×800 and 390×800, both color schemes.
  Exact 390×844 run also exits 0 PASS; receipts in `visual-qa/` and
  `visual-qa-phone/` under the evidence directory.
- Browser source-fidelity proof: all 256 Markdown fragments and all source links
  retained, 36 Was/Now articles, 21 issue rows and dots. The figure bottom is
  y=536.47 desktop and y=650.41 phone. No HTTP requests with JS disabled;
  keyboard skip focus, Enter disclosure toggle and print reopening pass.
- The recipe in `references/pdf.md` generated both formats with Chromium
  153.0.8010.12 / Node 24.19.0. A4 and Letter are each 8 pages, compared with the
  historical 10. Both have zero content outside the 40pt bounds check, all 21
  issue URI annotations, both full SHA-256 values and the install command.
  Every page PNG and both contact sheets inspected. Sections flow without hard
  breaks; developer cards remain together. Final-page whitespace is ordinary
  document ending, with no forced per-section blank half-pages.

## Critique

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Distinctiveness | 5 | Sandstone/serif verdict and directly labelled closure stitches retained. |
| Fit | 4 | Equal dots account for breadth and the largest group without inventing impact. |
| Hierarchy | 5 | Entire verdict and figure above fold at both requested sizes. |
| Craft | 4 | Strict QA passes; continuous print flow with intact cards, rows and links. |
| Accessibility | 5 | Both schemes, JS-off fidelity, keyboard controls, SVG description and open data table pass. |

The builtin skill uses 58 lines. All seven resources are registered in each
Claude/Codex/Grok catalog and the OpenCode projection. Fresh project sync and
MCP show/list_all are covered by integration tests. Root Claude/Codex skill
copies are ignored generated projections per repository policy.
