# Hub lifecycle status concept brief

- **First two lines:** Name whether the recorded PID exited, is starting, is stuck in startup, is stopping, is running without answering, or is ready; give the recovery command immediately when it is not ready.
- **Scannable:** One verdict line names the PID and elapsed seconds, followed by one remedy line and the existing transport summary.
- **Readable:** Launcher, start time, record version, and binary version follow the remedy for diagnosis.
- **Machine output:** `--json` remains one object with `running`, `record`, `binary`, and transport fields; `state` adds `kind`, `pid`, `age_secs`, `message`, and `remedy`.
- **Omitted:** Process samples and hub logs remain operator commands and files; status does not dump them into the terminal.

## Critique

The macOS terminal QA run used a task-local adapter for BSD `script` so all 80- and 120-column PTY captures contained the actual command output. The normal QA runner's util-linux `script -c` invocation produced empty macOS captures and a false PASS. See the task note and artifact adapter.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The first line names PID and state; the second gives the recovery command. |
| Fit | 5 | Widest 80-column state capture is 74 cells, with no wrapping. |
| Craft | 4 | Launcher/version evidence follows the remedy; transport state is separate. |
| Theme safety | 4 | All palettes and NO_COLOR pass; the existing global error mark under C locale has a documented allowlist entry. |
| Machine contract | 5 | `--json` is one document with `state.kind`, PID, age, message, and remedy. |

Terminal QA receipts (each 12 runs, zero failures and warnings): exited, starting, startup wedged, and unresponsive PASS with one documented global C-locale allowance each; running PASS with zero allowances. Captures and reports: `/Users/pippenz/.cas/artifacts/cas-c67c/terminal-*/report.md`. Scored by the implementing worker on 2026-09-23.
