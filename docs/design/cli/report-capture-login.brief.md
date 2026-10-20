# Brief: report capture login preflight

First two lines: state that login failed because its success URL is missing,
then give a copyable `--success-url` example for the caller to adapt.

Scannable: one diagnostic followed by one remedy, before credentials are read
or a browser starts.

Readable: explain that the example must describe the signed-in route, because
a sign-in URL can carry its destination as a query parameter.

Machine output: no `--json` mode exists; stdout stays empty, stderr carries the
diagnostic, and exit 2 means invalid arguments. No capture manifest is written.

Omitted: credentials, browser setup and per-shot redirect errors; preflight
can resolve this mistake without those details.

## Critique

terminal-qa: PASS report-capture-login · 11 runs · 0 fail · 0 warn · 0 allowed

Receipt: cas-9dee artifacts, `terminal-qa/report-capture-login/report.json`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The first line says login failed and names the missing flag; the second gives its example. |
| Fit | 5 | Two lines resolve one argument mistake before browser or credential work. |
| Craft | 5 | Both lines fit 80 columns; maximum width is 74. |
| Theme safety | 5 | Four palettes, NO_COLOR and C locale preserve the same plain ASCII text. |
| Machine contract | 5 | stdout is empty; stderr has the diagnostic; all 11 runs exit 2. |

Scored by keen-eagle-97 on 2026-10-20; the floor holds.
