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

## Webhook value validation (cas-2e34)

| Field | Contract |
| --- | --- |
| First two lines | The edit dialog keeps the setting and entered value visible; invalid URL details remain inside that dialog. |
| Scannable | Show the validation cause beside the edit, with Enter and Esc recovery controls still visible at 80/120 columns. |
| Readable | Wrap the existing validation message inside the dialog and allow enough height for the cause and controls. |
| Machine output | No CLI JSON or config serialization changes; invalid entries do not update in-memory values or disk. |
| Omitted | Do not echo webhook URLs or payloads into refusal logs; stored-value refusal gets one warning naming the setting. |

The edit dialog uses 80% of terminal width and 50% of its height to keep the
wrapped cause and Enter/Esc controls visible on an 80-by-24 terminal.

Curl ignores default config with first-argument `-q` and has a ten-second total
transfer budget. Existing destination normalization, protocol restrictions,
glob suppression and option separation remain in place. Clearing the URL stays
a quiet no-op. Successful edits remain unsaved until the existing save action,
including its operator-only policy check.

### Webhook validation critique

terminal-qa: PASS notification-webhook-refusal-capture · 11 runs · 0 fail · 0 warn · 0 allowed

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Actual TUI captures retain the setting, input, URL cause and Enter/Esc controls inside one dialog; the hook warning names the refused channel and setting. |
| Fit | 4 | Real 80/120-column dialogs show the wrapped cause and recovery controls; the captured warning is 76 columns and passes both widths. |
| Craft | 4 | Invalid HTTP, whitespace and malformed URL edits remain editable; correction stays unsaved and the adjacent operator-only save refusal remains intact. |
| Theme safety | 4 | The new default-foreground ASCII warning passes four palettes, pipe, NO_COLOR and C locale; TUI styles remain inherited, with actual dark/light text inspected. |
| Machine contract | 5 | Refused stored URL keeps hook stdout `{}`, emits one redacted stderr warning, and invokes no curl; empty URL remains quiet. |

Scored by wise-sparrow-74 on 2026-10-22 against supervisor candidate
`f079e45bd117eebf56693e9aa65ad07971f0604d`. The gate replays the actual hook
stderr retained in `notification-webhook-refusal.capture.txt`; it is not eleven
fresh hook sends. The real candidate also sent exactly one POST to the intended
loopback path with a hostile HOME/.curlrc adding another URL/header: neither
addition took effect. Curl argv began with `-q` and included `--max-time 10`.
The full inherited TUI theme/locale matrix is not newly claimed by this gate.
