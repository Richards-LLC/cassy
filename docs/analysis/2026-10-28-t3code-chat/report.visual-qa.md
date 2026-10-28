# T3 Code study — visual and acceptance receipt

Rendered surface: `report.html`, generated from `report.md`, branded from `DESIGN.md@b43940e43` inheriting `hub-web/DESIGN.md@ce0d8adc9`. Task artifacts use the project key `cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a`, task `cas-b4e8`.

- `node scripts/visual-qa.mjs --strict --viewport 1280x800 --artifact-dir <task-artifacts>/visual-qa-desktop docs/analysis/2026-10-28-t3code-chat/report.html`: PASS, light and dark, zero findings and zero allowlists. Includes print-loss and JavaScript-disabled comparison.
- Same strict command at `--viewport 390x844`, output `<task-artifacts>/visual-qa`: PASS, light and dark, zero findings and zero allowlists. Fold screenshots with JavaScript disabled confirm the whole figure ends at 540px (440px desktop).
- `cas deliverable finalize <task-artifacts>/report-draft.html --audience internal --out docs/analysis/2026-10-28-t3code-chat/report.html --pdf a4,letter --check-dir <task-artifacts>/pdf --json`: PASS. Final HTML: 198345 bytes, embedded local fonts/mark, no render-time network dependencies. A4: 8 pages; Letter: 10. Both `check-pdf.py` receipts: zero bounds findings, zero local links, no footer-only last page, no orphaned assets header.
- `python3 <task-artifacts>/check-report.py`: PASS, 12 techniques, 25 pinned citation ranges checked against actual files and line counts, full Markdown/HTML/PDF text fidelity, no upstream install directories. Running folios and CSS case transformation are excluded from fidelity comparison.
- `python3 scripts/markdownlint.py --config .markdownlint-cli2.jsonc docs/analysis/2026-10-28-t3code-chat/*.md`: PASS after fixing blank lines, heading punctuation and a bare URL.

Visual inspection: desktop light and phone dark folds, full-page light/dark captures, all A4/Letter page contact sheets, final A4 page and final two Letter pages at full size. Native tables retain their copyable data, figure labels remain distinct without color, code commands stay in one block, and the final page carries substantive provenance. Technique boundaries intentionally leave some print whitespace.

No product code changed. No application journeys, upstream scripts, upstream builds or upstream tests were run. The report demonstrates its own rendered artifact; all claims about T3 Code behavior are source-inferred. See `search-manifest.md` and the task’s `LEDGER.md` for scope and unexercised app journeys.
