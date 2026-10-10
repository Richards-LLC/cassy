# Visual QA readiness diagnostics

| Field | Contract |
| --- | --- |
| First two lines | A strict capture reports PASS or a named failure, with the readiness cause directly below a failed surface. |
| Scannable | Each capture lists text nodes, characters, main landmarks, content elements and loading indicators in two short rows. |
| Readable | A readiness failure names the timeout and suggests rerunning with a selector for the application shell. |
| Machine output | `visual-qa.json` retains findings and screenshots and adds a per-capture `readiness` array with numeric content measurements, readiness reason, selector and timeout. There is no `--json` stdout mode. |
| Omitted | Browser module cache paths stay out of the summary; detailed findings and screenshot links remain in the JSON and Markdown artifacts. |

Readiness verifies resting pages, including the resting page of a declared journey.
`--ready-selector CSS` requests a visible shell; `--ready-timeout-ms MS` sets the
positive finite wait bound (default 5000 ms). Missing or hidden targets, empty
surfaces and dominant loading states produce `page-not-ready`, which a visual
allowlist cannot suppress. A partly rendered `aria-busy` region can retain real
content. Declared journey states can intentionally exercise loading after startup.

## Critique

terminal-qa: PASS visual-qa-loading · 11 runs · 0 fail · 0 warn · 0 allowed

terminal-qa: PASS visual-qa-ready · 11 runs · 0 fail · 0 warn · 0 allowed

Reports and palette captures: task `cas-c577` artifacts, `terminal-loading/`
and `terminal-ready/` (`report.json` and `report.md` in each directory).

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Verdict first, readiness cause second, content counts next, browser version last. |
| Fit | 4 | Both command outputs pass at 80 and 120 columns without allowlists. |
| Craft | 4 | Loading failure includes its timeout and a command with the selector option. |
| Theme safety | 5 | Four palettes, C locale, NO_COLOR and piped captures pass. |
| Machine contract | 4 | Both CLI copies publish parseable JSON with the selector and numeric content measurements; regression tests read those artifacts. |

Scored by cosmic-fox-76 on 2026-10-10. Existing detailed visual-defect output
is outside this readiness diagnostic change.
