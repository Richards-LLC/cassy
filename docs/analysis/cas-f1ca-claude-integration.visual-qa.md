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

Separate genuine Chrome 155.0.8059.84 report inspection: four first screens,
Markdown/main-text fidelity, seven sections, five tables and 48 links; no
page overflow, page errors or external requests. Visible 3px keyboard focus;
JS-disabled offline read retains content. Eleven A4 preview pages inspected,
zero print-bounds findings. See the adjacent brief for scored critique and limits.
