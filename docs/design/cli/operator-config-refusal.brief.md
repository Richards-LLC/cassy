# Operator-only config refusal

| Field | Contract |
| --- | --- |
| First two lines | The first line says the change was refused; the next line begins the indented reason. |
| Scannable | One short refusal headline comes before the wrapped explanation and remedy. |
| Readable | The original cause, affected setting and remedy remain readable in indented detail lines. |
| Machine output | `--json` keeps the existing `{status: "error", message: <original one-line refusal>}` document. |
| Omitted | No agent-marker inventory or extra policy explanation is added to the refusal. |

## Critique

Candidate: `cas 3.71.0 (b0d23e3 2026-10-22)`, revision `b0d23e34c7968f330345fe4fefc2fe8621c6c93f`, SHA-256 `1e6262449d0fca9013a99c69c4f887e9cf6e5c0edeb949d96b987cd98a5cd784`.

Receipt: `terminal-qa: PASS config-set-force-bypass-refusal · 12 runs · 0 fail · 0 warn · 0 allowed` (`cas-eade/terminal-qa/config-set-force-bypass-refusal/report.json`).

Locale-width receipt: `terminal-qa: PASS config-set-force-bypass-c-locale-80-120 · 6 runs · 0 fail · 0 warn · 0 allowed` (`cas-eade/terminal-qa/config-set-force-bypass-c-locale-80-120/report.json`).

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The first line says “Operator-only config change refused”; the reason follows immediately, with the remedy on its own command line. |
| Fit | 4 | The refusal uses eight short lines for one cause, one command and the inherited-agent context. |
| Craft | 5 | UTF-8 and C-locale 80/120 captures peak at 73 cells; the command and every wrapped detail line remain intact and indented. |
| Theme safety | 5 | Four palettes, `NO_COLOR`, and `LC_ALL=C` pass with no contrast, color or Unicode findings. |
| Machine contract | 5 | `--json` emits one error document with the original one-line refusal message. |

Tracked terminal captures are under `captures/operator-config-refusal/`; both complete reports and the HTML captures are in the task artifact directory.
