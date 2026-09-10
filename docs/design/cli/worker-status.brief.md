# Worker status

| Field | Contract |
| --- | --- |
| First two lines | Each worker leads with its observed liveness, so an ended turn is immediately visible. |
| Scannable | Summary mode emits exactly one plain-text row per worker, ordered by name. |
| Readable | Full mode gives the event, its age, process state and last artifact write age beside the verdict. |
| Machine output | The existing MCP text response carries `liveness: executing`, `waiting_for_input`, `stalled`, or `dead`; summary_mode selects compact rows. No new JSON envelope. |
| Omitted | Summary skips Git, task history, context and heartbeat diagnostics; full worker_status retains those details. |

Missing evidence maps to stalled with an explicit unavailable reason. Sleeping or CPU-idle processes alone do not prove a completed turn. A terminal event wins over heartbeat, mtime and old unmatched tool calls. Dead requires a missing or exited identity-checked process.

CLI parity: `cas factory worker-status --summary` uses the same roster and
classification as MCP summary_mode. `--json` returns one array with `name`,
`liveness`, and `evidence`; full mode retains the unabridged diagnostic line.

The CPU sample is a delta between polls; a first sample says `unsampled`.
On Linux, PID start time, process state, wait channel and observable stdin
read are local evidence. A stdin reader also exists during active threaded
harness turns, so it does not override turn events. On platforms without
procfs, PID existence remains available and detailed process probes are
explicitly unavailable. Missing turn/process evidence is `stalled`, not an
inferred execution claim. Full legacy worker_status still includes the
existing Git/task diagnostics; its `summary_mode=true` path is the bounded
fleet poll.

## Critique

terminal-qa: PASS worker-liveness · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-7b7b/terminal-qa/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | Every compact row starts with liveness; no heartbeat inference is needed. |
| Fit | 5 | All 36 observed worker rows fit 80 columns. |
| Craft | 4 | One row per worker; detailed event and process evidence is available in full mode. |
| Theme safety | 5 | Plain text passed four palettes, NO_COLOR and C locale. |
| Machine contract | 5 | CLI JSON is one array of name, liveness and evidence records. |

Scored by Codex on 2026-09-10 against the real fleet. Cold MCP summary poll
with five registered workers and a 4 MiB rollout each measured 20.91 ms.
The replay asserts all four verdicts while every registry heartbeat is fresh.
