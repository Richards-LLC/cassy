# Integrate terminal receipts (cas-46b0)

| Field | Contract |
| --- | --- |
| First two lines | Keep the platform/action/status verdict first and bring any `next:` remedy immediately after it. |
| Scannable | Summary rows remain labelled; continuation lines hang beneath each row, fitting the actual terminal at40,80 and120 columns. |
| Readable | Long prose wraps at spaces; an indivisible path or token is shortened in the middle, with an explicit `--full` escape. |
| Machine output | Violet's existing single JSON report remains unchanged and retains complete paths; human rendering never appends to it. |
| Omitted | Color, progress, configuration mechanics and new setup actions are outside this formatting correction. |

TTY width takes precedence over an inherited COLUMNS value. Pipes use a valid
COLUMNS value or80 columns; values under40 or invalid values fall back to80.
Non-UTF-8 locales use ASCII punctuation and escaped non-ASCII content. The
same renderer handles successful receipts and Violet's refusal diagnostics.
`--full` preserves complete tokens, even when they exceed the terminal width.

## Critique

Native RED at2e8352813:16 terminal runs,61 failures,0 warnings,0 allowed.
GREEN native terminal receipt and critique will be recorded after implementation.
