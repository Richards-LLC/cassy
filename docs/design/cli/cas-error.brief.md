# CLI error lines

| Field | Contract |
| --- | --- |
| First two lines | A command failure opens with one red error mark and its message; a `CasError` suggestion follows only when available. |
| Scannable | The error mark is visually distinct from the message, with any suggestion label and remedy beneath it. |
| Readable | The mark uses the foreground-safe `text_error` token; the message stays in the terminal foreground, and C locale uses `[ERROR]`. |
| Machine output | Human errors go to stderr; JSON remains one stdout document and existing exit codes carry the result. |
| Omitted | No stack trace, extra metadata, or new error advice is added; suggestions remain limited to `CasError` values. |

## Critique

terminal-qa: PASS cas-error-renderer · 11 runs · 0 fail · 0 warn · 0 allowed · <artifacts>/cas-d0b1/terminal-qa/error-renderer/report.json

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The 80-column light and dark lines begin with `✗` and state the OS cause immediately; the generic renderer does not invent a remedy. |
| Fit | 5 | The failed import is one diagnostic line in both terminal and piped captures, with no repeated detail. |
| Craft | 5 | The rendered line is 40 cells at 80 columns and does not wrap. |
| Theme safety | 5 | All four palettes pass; the gate reports no contrast findings, `NO_COLOR` strips SGR, and `LC_ALL=C` renders `[ERROR]`. |
| Machine contract | 4 | The pipe capture contains one line and the command exits 1 with the diagnostic on stderr; this run does not exercise `--json`. |

The 80-column palette render and light/dark text captures are in
[`captures/after/cas-error-renderer`](captures/after/cas-error-renderer); the full raw runs and
JSON report remain in the task artifact directory named by the receipt.
