# CLI error lines

| Field | Contract |
| --- | --- |
| First two lines | A command failure opens with one error mark and its message, red when stderr is a terminal and color is enabled; a `CasError` suggestion follows only when available. |
| Scannable | The error mark is visually distinct from the message, with any suggestion label and remedy beneath it. |
| Readable | The mark uses the foreground-safe `text_error` token; the message stays in the terminal foreground, and C locale uses `[ERROR]`. |
| Machine output | Human errors go to stderr; JSON remains one stdout document and existing exit codes carry the result. |
| Omitted | No stack trace, extra metadata, or new error advice is added; suggestions remain limited to `CasError` values. |

The top-level human diagnostic detects color from stderr, its destination.
Redirecting stderr produces plain text even when stdout remains a terminal;
terminal stderr remains styled even when stdout is piped. `NO_COLOR` suppresses
styling in either case. Error text, JSON stdout, and exit codes remain unchanged.

## Command-owned stderr (cas-62e0)

The same destination rule applies to diagnostics emitted inside auth, cloud,
config, factory and update commands, including update rollback warnings.
Their stderr handles use `Formatter::stderr`; stdout handles keep
`Formatter::stdout`. The first diagnostic line, its remedy, stream and exit
code stay the same. Piped stderr is plain with terminal stdout, terminal
stderr can be styled independently, and `NO_COLOR` remains authoritative.

The representative regression extends the existing mixed-stream PTY table
with `config export --format unsupported`: stdout is a real TTY, stderr a
pipe, and the command's own error must have no escapes. A source audit tracks
all 19 direct stderr sinks; it is not claimed as runtime coverage of every
authentication, network or update failure. Fresh candidate evidence and
critique belong to the cas-62e0 task ledger; earlier receipts below retain
their original scope.

## Critique

terminal-qa: PASS cas-error-renderer · 11 runs · 0 fail · 0 warn · 0 allowed · &lt;artifacts&gt;/cas-d0b1/terminal-qa/error-renderer/report.json

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

## Stream-aware error critique (cas-d6ab)

terminal-qa: PASS cas-d6ab · 11 runs · 0 fail · 0 warn · 0 allowed · &lt;artifacts&gt;/cas-d6ab/terminal-qa/status/report.json

Supervisor candidate `6af456e4002f54739bf562b6d5c94c7d0e1696e9` passes
`terminal-qa --strict`. A real mixed-stream PTY walk confirms piped stderr has
no escapes with terminal stdout, terminal stderr remains styled with piped
stdout, and `NO_COLOR` suppresses styling. All three preserve exit1 and leave
the disposable project unmodified.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 5 | One error line states the missing store and names the `cas init` remedy. |
| Fit | 5 | Human output is one line: 37 cells styled, 43 cells in the plain pipe capture. |
| Craft | 5 | All eleven captures pass without warnings; stream redirection preserves text and the existing exit code. |
| Theme safety | 5 | Four palettes at 80 and 120 columns pass; C locale and no-color retain readable plain output. |
| Machine contract | 5 | Raw mixed-stream captures prove piped stderr has no ANSI; status refusal retains its existing stderr-only contract. |

## Command-owned stderr critique (cas-62e0)

terminal-qa: PASS cas-62e0 · 11 runs · 0 fail · 0 warn · 0 allowed

Candidate `cas 3.72.0 (ca6209b 2026-10-22)` maps to
`ca6209bf1181fa7eb41b02abd7f6b94c7dab624c`. The actual config-export error
passes the strict inherited-stream gate at 80/120 columns, four palettes,
pipe, C locale and NO_COLOR. Separate actual-command PTY readbacks verify
plain piped stderr with terminal stdout, styled terminal stderr with piped
stdout, and NO_COLOR suppression; all preserve exit1. A separate successful
JSON-export control returns one document with no escapes and exit0. It is not
presented as a JSON error response for the unsupported-format command.

The probe preserves the worker identity and uses disposable HOME/project
fixtures. All 13 readback files confirm config bytes are unchanged. Source
audit records 19 direct stderr sinks before and zero after; this supplements
the representative runtime path rather than claiming every network, auth or
update failure was exercised. Captures and raw split-stream bytes are in the
cas-62e0 task's `terminal-qa/config-export/` and `stderr-*/` artifacts.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 5 | One error mark names the unknown format and gives the supported formats immediately. |
| Fit | 5 | The diagnostic is one line, with no extra metadata or repeated advice. |
| Craft | 5 | The line is 51 cells styled and 57 plain, without wrapping at 80 columns. |
| Theme safety | 5 | The strict four-palette gate has no findings; C locale and NO_COLOR remain readable. |
| Machine contract | 5 | Three actual split-stream checks prove destination-based styling, and the separate successful JSON control remains clean. |

Scoped text and mechanical captures reviewed by fierce-marten-72 on 2026-10-22.
No live auth/update flow or GUI screenshot is claimed. Subsequent brief-only
edits do not change the candidate's runtime, test inputs or probe.
