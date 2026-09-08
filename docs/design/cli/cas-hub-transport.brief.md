# Brief: `cas hub status` and `cas doctor --host` transport diagnostics

| Field | Sentence |
| --- | --- |
| First two lines | Whether the running hub's CAS-created Tailscale Serve route reaches its current ephemeral shim, followed by one copyable restart remedy when it does not. |
| Scannable | `cas hub status` keeps the hub verdict first and adds one Tailscale Serve row; `cas doctor --host` adds one `hub transport` check row with the same status and cause. |
| Readable | The report names the expected and actual loopback targets only when they differ, then gives the single restart command that republishes the route. |
| Machine output | `cas hub status --json` returns one object with the existing `running`, `record`, and `binary` fields plus `tailscale_serve: {status, message, expected_target?, actual_target?, remedy?}`; `cas doctor --json --host` remains one array of check objects and never prints a human banner. |
| Omitted | Raw Tailscale status JSON, receipt timestamps, and unrelated Serve handlers are omitted from normal output; operators can inspect the route with the existing Tailscale CLI. |

## Rendering decisions

- A healthy hub retains the existing status line; the transport line is a single actionable
  finding and appends only one command-sized remedy on failure.
- The JSON report keeps status data separate from the persisted process record so a stale
  loopback target cannot be mistaken for the current hub port.
- The doctor row reuses the hub transport classifier, keeping the status and doctor verdict,
  cause, and remedy identical.

## Critique

Terminal QA is run against the rebuilt binary after the scoped integration test. The capture
directory is `/home/pippenz/.cas/artifacts/cas-4c4b/terminal-qa/` per the task workspace
contract.

## Critique

terminal-qa: PASS cas-hub-status · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-4c4b/terminal-qa/cas-hub-status/report.json

terminal-qa: PASS cas-doctor-hub-transport · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-4c4b/terminal-qa/cas-doctor-hub-transport/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | `cas hub status` keeps the hub verdict first and puts the transport verdict immediately below it. |
| Fit | 4 | Status uses compact rows; doctor folds the transport result into its existing grouped check layout. |
| Craft | 4 | Mismatch details use hanging `expected`, `actual`, and `remedy` lines; the captured outputs fit at 80 columns. |
| Theme safety | 5 | Both gates pass dark, light, Solarized, `NO_COLOR`, and C-locale runs. |
| Machine contract | 5 | Both JSON runs produce one document and preserve the command exit verdict. |

Scored by the worker on 2026-09-08; floor holds.
