# Brief: `cas hub status` and `cas doctor --host` transport diagnostics

| Field | Sentence |
| --- | --- |
| First two lines | Whether the hub is healthy or wedged, followed immediately by the one copyable restart command that clears a lock holder with no live runtime endpoint. |
| Scannable | `cas hub status` keeps the hub verdict first and adds one Tailscale Serve row; `cas doctor --host` adds one `hub transport` check row with the same status, holder PID/age, and remedy. |
| Readable | The report names a wedged holder by PID and age, names expected and actual loopback targets only when they differ, and gives one restart command. |
| Machine output | `cas hub status --json` returns one object with `running`, `record`, `binary`, optional `lock_holder: {pid, age, phase, command}`, and `tailscale_serve: {status, message, expected_target?, actual_target?, remedy?}`; `cas doctor --json --host` remains one array of check objects and never prints a human banner. |
| Omitted | Raw Tailscale status JSON, receipt timestamps, and unrelated Serve handlers remain omitted; process-table archaeology stays behind the holder PID/age summary. |

## Rendering decisions

- A healthy hub retains the existing status line; the transport line is a single actionable
  finding. Healthy Serve routes print the classifier's route-target message, while a missing
  route stays the compact loopback-only line and an unavailable publication stays a warning.
- The JSON report keeps status data separate from the persisted process record so a stale
  loopback target cannot be mistaken for the current hub port.
- The doctor row reuses the hub transport classifier, keeping the status and doctor verdict,
  cause, and remedy identical.

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

### Wedged-holder recovery rendering

terminal-qa: PASS cas-be89-hub-status · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-be89/terminal-qa/report.json

terminal-qa: PASS cas-be89-doctor-hub-transport-allowlisted · 12 runs · 0 fail · 0 warn · 12 allowed · /home/pippenz/.cas/artifacts/cas-be89/terminal-qa-doctor-allowlisted/report.json

The doctor receipt has a raw companion failure at `/home/pippenz/.cas/artifacts/cas-be89/terminal-qa-doctor/report.json`; its 12 findings are pre-existing truncations in unrelated stale-registration and cloud-purge diagnostics, each explicitly allowlisted in `/home/pippenz/.cas/artifacts/cas-be89/terminal-qa-doctor-allowlist.json`. The hub transport row remains one compact `✓ hub transport` check in the captured doctor output, and the status surface retains a three-line verdict/owner/transport hierarchy.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | `cas hub status` leads with running/not-ready, then PID/version, then the transport verdict; wedged status leads with holder PID/age and its force remedy. |
| Fit | 4 | Human output remains compact while JSON adds optional `lock_holder` detail without banners or mixed documents. |
| Craft | 4 | Holder age, phase, and one copyable force restart remedy use the existing hanging-indentation grammar; status and doctor share the classifier. |
| Theme safety | 5 | Both status and doctor gates pass dark, light, Solarized, `NO_COLOR`, and C-locale runs; doctor unrelated truncations are the only allowlisted findings. |
| Machine contract | 5 | JSON runs produced one document and preserved the command exit verdict in both gates. |

Scored by the worker on 2026-09-10; floor holds.

### Healthy-route rendering correction

The healthy route now renders `Tailscale Serve: OK - route targets the live hub shim at …`,
while the unavailable and no-route messages retain their warning and compact loopback forms.
The updated terminal capture is stored under `/home/pippenz/.cas/artifacts/cas-5b1b/terminal-qa/`.

terminal-qa: PASS cas-hub-status-cas-5b1b · 12 runs · 0 fail · 0 warn · 0 allowed · /home/pippenz/.cas/artifacts/cas-5b1b/terminal-qa/cas-hub-status/report.json

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The existing hub verdict remains first and the corrected Serve verdict is the immediately following line. |
| Fit | 5 | The healthy route is one compact row; the capture has three lines and no repeated explanatory block. |
| Craft | 5 | The route row is 79 cells at the 80-column gate, with no wrapping or split token. |
| Theme safety | 5 | The gate passes dark, light, both Solarized palettes, `NO_COLOR`, and C-locale runs. |
| Machine contract | 5 | The JSON run remains one document and reports the same healthy route message as the human row. |

Scored by the worker on 2026-09-08; floor holds.
