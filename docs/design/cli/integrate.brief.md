# Integrate terminal receipts (cas-46b0)

| Field | Contract |
| --- | --- |
| First two lines | Keep the platform/action/status verdict first and bring any `next:` remedy immediately after it. |
| Scannable | Summary rows remain labelled; continuation lines hang beneath each row, fitting the actual terminal at 40, 80 and 120 columns. |
| Readable | Long prose wraps at spaces; an indivisible path or token is shortened in the middle, with an explicit `--full` escape. |
| Machine output | Violet's existing single JSON report remains unchanged and retains complete paths; human rendering never appends to it. |
| Omitted | Color, progress, configuration mechanics and new setup actions are outside this formatting correction. |

TTY width takes precedence over an inherited COLUMNS value. Pipes use a valid
COLUMNS value or 80 columns; values under 40 or invalid values fall back to 80.
Non-UTF-8 locales use ASCII punctuation and escaped non-ASCII content. The
same renderer handles successful receipts and Violet's refusal diagnostics.
`--full` preserves complete tokens, even when they exceed the terminal width.

## Critique

Native RED at 2e8352813: 16 terminal runs, 61 failures, 0 warnings, 0 allowed.
Native GREEN at eb7f20baa: 16 runs, 0 failures, 0 warnings, 0 allowed.
The additional project-policy capture also passes 16/16 and deliberately sets
COLUMNS=200 inside each PTY: the actual 40/80/120-column width still wins.
Reports: task cas-46b0, `rapid-lynx-72/terminal-green/report.json` and
`terminal-qa/integrate/report.json`. The 80-column light/dark HTML and piped
text were reviewed. These receipts bind the renderer before this documentation
and supplemental-test commit; renderer inputs are unchanged.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | `violet init: stale` gives one verdict; `next: Set ...; run cas login` follows immediately, before file receipts. |
| Fit | 4 | Labelled credentials and registration rows scan separately; project policy is readable prose, and long paths have an explicit `--full` escape. |
| Craft | 4 | Continuations hang four spaces in; all default human lines fit 40/80/120 cells, including the long project note. |
| Theme safety | 5 | Four palette captures pass without exceptions; no color is required, and C/NO_COLOR captures remain readable. |
| Machine contract | 5 | JSON is one complete report with unshortened paths; pipes have no controls, and the existing 401 refusal contract is retained. |

The existing status vocabulary and healthy rows remain intact. `--full`
intentionally permits long tokens to exceed the width so an operator can copy
them without losing characters. No setup writes or credential semantics change.
