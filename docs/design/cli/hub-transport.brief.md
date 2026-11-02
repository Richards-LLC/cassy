# Hub transport policy and refusals

| Field | Contract |
| --- | --- |
| First two lines | A required-transport refusal names the policy and says whether start or restart was refused; an Auto fallback says Serve is inactive and the hub is loopback-only. |
| Scannable | Tailscale diagnostics keep CLI and socket on labeled rows; legacy config writes identify the effective `hub.transport_mode` and publication value. |
| Readable | A required restart refusal leaves the existing hub running and gives one way to retry with a signed-in daemon; Auto fallback explains that publication retries automatically. |
| Machine output | Hub status retains one JSON document with the existing `tailscale_serve.status`, `message`, and `remedy` fields; config set retains `{key, old_value, new_value}` JSON; start refusals remain nonzero with stderr diagnostics. |
| Omitted | Raw Tailscale output, account details, auth state, and alternate daemon discovery never appear in these diagnostics. |

The existing `--no-tailscale-serve` flag remains available; `--loopback` is its
discoverable alias. An explicit `TAILSCALE_SOCKET` follows both service and
detached launches. In Auto mode every publication failure keeps the
hub on loopback with the cause in status and logs while publication retries in
the background. Required mode still refuses
publication failure and offers an explicit loopback alternative.

Both launchd and systemd Auto services expose loopback immediately, then publish
in the background with a 1-second retry doubled to a 30-second cap. Recovery
restores the endpoint and its live metadata without a restart. Required service
starts retain their bounded startup retries and explicit refusal. `hub stop`
removes the service job before stopping any remaining process.

Foreign Serve mappings remain untouched, including unproven legacy dead
shims. The refusal names the selected CLI/socket and port-scoped removal;
the operator must inspect ownership before explicitly clearing that port.

Required-mode start and restart refusals keep the signed-in-socket retry on its
own line and the explicit loopback alternative on the next line. Each command
remains complete and copyable, and each remedy line fits 80 columns. CLI/socket
cause rows and JSON refusal semantics remain unchanged.

## Finding disposition

- F01: fixed. Required-mode refusal leads with the policy and refused operation,
  retains the CLI/socket cause rows, and suppresses the prior Auto warning.
- F02: fixed. Setting the legacy key under an explicit transport mode reports
  that it is shadowed and prints the effective publication value.
- F03: declined as stale. Merged cas-337b already removes the override on reset
  and restores the legacy fallback; its named regression test is in
  `cas-cli/src/cli/config/read_ops.rs`.
- F04: fixed. Auto fallback output is split into short name, cause, and retry
  rows; the retry checks Tailscale and restarts with the saved policy.
- F05: fixed. `--no-tailscale-serve` help names `hub.transport_mode` and says
  the flag overrides policy for one launch.
- F06: fixed. Invalid `hub.transport_mode` reports its key and accepted values
  without the generic parse suggestion.
- F07: declined. The test covers behavior, not copy: `publication_failure_is_fatal`
  still decides whether failed Tailscale publication is fatal during service
  startup versus interactive startup. Keep the test so those policy branches
  remain covered.

## Critique

terminal-qa: PASS hub-restart-auto-fallback · 7 runs · 0 fail · 0 warn · 0 allowed · &lt;artifacts&gt;/cas-4c14/terminal-qa/hub-restart-auto-fallback/report.json

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The first two lines report the stop and successful restart; the following warning names loopback-only service and gives one retry command. |
| Fit | 5 | The warning is a three-line name → cause → retry block, with one remedy rather than a paragraph of alternatives. |
| Craft | 5 | All seven captures pass at 80 columns; the longest line is 55 cells, with no wrap or clipped token. |
| Theme safety | 5 | Dark, light, both Solarized palettes, `NO_COLOR=1`, and `LC_ALL=C` pass with readable plain output. |
| Machine contract | 5 | The piped run exits 0 and appends the warning cleanly; the targeted JSON restart test passes with one JSON value. |

## Required refusal critique (cas-1471)

terminal-qa: PASS cas-1471-start · 12 runs · 0 fail · 0 warn · 0 allowed · &lt;artifacts&gt;/cas-1471/terminal-qa/start/report.json

terminal-qa: PASS cas-1471-restart · 12 runs · 0 fail · 0 warn · 0 allowed · &lt;artifacts&gt;/cas-1471/terminal-qa/restart/report.json

Supervisor QA used candidate `894434db7607a4f634b3d30a672bafa0037ca7f0`
with isolated Required-mode, signed-out probes. Both commands refused before
starting a hub; all process and runtime-file readbacks were empty. A follow-up
with `--strict` explicitly set reproduced both passing reports with no warnings.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The first two lines name Required transport and the refused operation, followed by the signed-out cause and labeled diagnostics. |
| Fit | 5 | Full retry and loopback commands occupy separate lines; all human captures fit 80 columns (maximum 57 cells for start, 59 for restart). |
| Craft | 5 | Both operations pass all twelve captures with no warnings, clipping, or split command tokens. |
| Theme safety | 5 | Four palettes at 80 and 120 columns, C locale, and no-color captures retain the same readable refusal. |
| Machine contract | 5 | Each JSON capture is one error document; every probe preserves the candidate's nonzero refusal exit. |
