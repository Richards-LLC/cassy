# Report visual QA — PASS

2026-10-17, Playwright 1.64.1 and system HeadlessChrome 155. Applied the cas-html-reports, cas-ui-craft and cas-dataviz report contracts.

- `node scripts/visual-qa.mjs --strict --viewport 1280x800 --artifact-dir <task>/visual-qa file://<worktree>/docs/analysis/2026-10-17-lane-ci/report.html`: PASS, light+dark, zero findings, no allowlist.
- Same command at `--viewport 390x844`, output `<task>/visual-qa-mobile`: PASS, light+dark, zero findings, no allowlist.
- `node <task>/render-check.mjs`: PASS, all four JS-disabled renders retain 20499 characters, six tables, one h1, no page overflow, zero external requests. Opened and inspected all four screenshots.
- Print/PDF: eight A4 pages; opened print render and rasterized PDF page 2; repeated headers, table wrapping and link targets remain readable.
- `python3 <task>/validate.py`: PASS, 30 unique merged heads, 28 SHA-matching Rust receipts, all measured/model medians reproduced. Re-rendered HTML from Markdown before browser checks.

`<task>` is `/home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-0e24`. Full QA JSON, screenshots, PDF, raw run evidence and scripts remain there. No web product or journey inputs changed; no application suite was run for this report.
