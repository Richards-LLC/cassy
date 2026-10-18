# Visual QA — PASS

Report: `cas-f1ca-claude-integration.html`. Research only; no harness runtime acceptance.

Strict skill runner: Playwright 1.64.1, light/dark, 1280×800 and 390×800;
additional light/dark 390×844. Six screenshots, zero findings, zero allowlists.

```sh
node /home/pippenz/.codex/skills/cas-ui-craft/scripts/visual-qa.mjs --strict --artifact-dir <taskroot>/visual-qa docs/analysis/cas-f1ca-claude-integration.html
node /home/pippenz/.codex/skills/cas-ui-craft/scripts/visual-qa.mjs --strict --viewport 390x844 --artifact-dir <taskroot>/visual-qa-844 docs/analysis/cas-f1ca-claude-integration.html
```

Both commands exited 0 and printed `PASS`. The portable task bundle contains
original generated Markdown/JSON receipts and screenshots in those directories.

Final verification after adding the Codex crosswalk and 18 October source check:

- Pinned `scripts/markdownlint.py`: 3 Markdown files, 0 errors.
- `visual-qa/visual-qa.md` and `visual-qa-844/visual-qa.md`: strict PASS,
  six captures total, 0 findings and 0 allowlists.
- `browser/receipt.json`: Chrome 155.0.8059.84 / Playwright 1.64.1; Markdown to
  HTML text fidelity PASS, 7 sections, 6 keyboard-focusable table regions,
  60 links, 0 page overflow, 0 external requests and 0 page errors. JavaScript
  disabled retained all 7 sections, 6 tables and 60 links.
- `print-a4-final-20261018-r2/receipt.json`: 12 A4 pages; 3,416 word boxes within page
  bounds, 0 bounds findings. All 12 page images were inspected; table rows
  remain together across page breaks.
- `research-verification.json`: PASS; 266 coverage rows, 239 unique docs,
  15 reviewed release entries / 1,266 bullets, 16 decision rows and four
  current release/channel metadata checks.

These are report-rendering and source-coverage checks. They do not validate a
Claude runtime, native capability, hook, message or authorization behavior.
