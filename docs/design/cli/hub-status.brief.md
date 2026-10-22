# Hub lifecycle status concept brief

- **First two lines:** Name whether the current hub is starting, stuck in startup, stopping, running without answering, ready, or exited; a live machine-lock holder takes precedence over a dead recorded PID; give the recovery command immediately when it is not ready.
- **Scannable:** One verdict line names the live lock holder's PID and elapsed seconds when the process record is stale, followed by one remedy line and the existing transport summary.
- **Readable:** Launcher, start time, record version, and binary version follow the remedy when they describe the current process; stale process metadata stays in JSON.
- **Machine output:** `--json` remains one object with `running`, `record`, `binary`, and transport fields; `state` adds `kind`, `pid`, `age_secs`, `message`, and `remedy`; when a live lock holder supersedes a dead record, `state` and `lock_holder` identify it while `record` retains the historical PID. A lock alone leaves `running` false and the exit status nonzero.
- **Omitted:** Process samples and hub logs remain operator commands and files; status does not dump them into the terminal or present a dead record's launcher/version as metadata for its live replacement.

## Critique

The macOS terminal QA run used a task-local adapter for BSD `script` so all 80- and 120-column PTY captures contained the actual command output. The normal QA runner's util-linux `script -c` invocation produced empty macOS captures and a false PASS. See the task note and artifact adapter.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The first line names PID and state; the second gives the recovery command. |
| Fit | 5 | Widest 80-column state capture is 74 cells, with no wrapping. |
| Craft | 4 | Launcher/version evidence follows the remedy; transport state is separate. |
| Theme safety | 4 | All palettes and NO_COLOR pass; the existing global error mark under C locale has a documented allowlist entry. |
| Machine contract | 5 | `--json` is one document with `state.kind`, PID, age, message, and remedy. |

Terminal QA receipts (each 12 runs, zero failures and warnings): exited, starting, startup wedged, and unresponsive PASS with one documented global C-locale allowance each; running PASS with zero allowances. Captures and reports: `<artifacts>/cas-c67c/terminal-*/report.md`. Scored by the implementing worker on 2026-09-23.
