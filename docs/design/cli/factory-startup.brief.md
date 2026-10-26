# Factory daemon startup refusal

| Field | Contract |
| --- | --- |
| First two lines | State that startup was refused, then name the one correction before any factory process, store registration or socket is created. |
| Scannable | Three short lines: refusal, remedy, and the byte limit or project ownership rule. |
| Readable | Socket errors distinguish GUI/TUI and count filesystem bytes including space for the terminating NUL; foreign-root errors identify CAS_ROOT as the rejected override. |
| Machine output | `--json` uses the existing single `{status: error, message: string}` stdout document; the human refusal stays on stderr. |
| Omitted | Full long paths and launch internals are omitted because the caller supplied HOME/session/--cwd and the original invocation already names them. |

The socket remedy is a directory/session choice, so it is stated directly rather than inventing a path that might move the operator's state. The foreign-root remedy is the copyable `unset CAS_ROOT` command before retrying the original launch. Existing Formatter verdict colours, C-locale fallback and NO_COLOR handling apply.

## Critique

Candidate `1277218e2152f3bf934aab80b684a76f3609ff77`; captures use the real executable built by the targeted CLI regression. Later changes only format the same admission and add this critique/capture. Native execution is Linux; the regression carries explicit macOS/Linux boundary expectations for assembly on each OS.

```text
terminal-qa: PASS socket-too-long · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-4927/terminal-qa/socket-too-long/report.json
terminal-qa: PASS foreign-root · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-4927/terminal-qa/foreign-root/report.json
```

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Line one says startup refused; line two supplies the shorter-HOME choice or `unset CAS_ROOT`. |
| Fit | 5 | Both refusals occupy three lines at 80 columns, with no extra banner or long path. |
| Craft | 4 | The same refusal/remedy/evidence grammar covers both causes; socket evidence names bytes and the NUL-inclusive address capacity. |
| Theme safety | 5 | Both 80-column HTML captures were visually read on all four palettes; the strict gate reports no contrast, width, monochrome or C-locale findings. |
| Machine contract | 5 | Each JSON stdout is one error document, with exit 1 and a separate human stderr; pipes contain no cursor controls. |

The 24 captures leave each attempted HOME absent and the private foreign database byte-for-byte unchanged with zero agents (`terminal-proof.json`). The committed [pipe capture](factory-startup.80.txt) preserves the two diagnostic rows; the existing single regression asserts their JSON/stderr contracts and side-effect refusal. Browser MCP launch failed on its system Chrome sandbox setup; local headless Chrome rendered the same static HTML for the visual critique. Full ANSI/HTML captures and screenshots live with the task artifacts.
