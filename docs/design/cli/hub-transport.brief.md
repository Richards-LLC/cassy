# Hub transport policy and refusals

| Field | Contract |
| --- | --- |
| First two lines | A required-transport refusal names the policy and says whether start or restart was refused; an Auto fallback says Serve is inactive and the hub is loopback-only. |
| Scannable | Tailscale diagnostics keep CLI and socket on labeled rows; legacy config writes identify the effective `hub.transport_mode` and publication value. |
| Readable | A required restart refusal leaves the existing hub running and gives one way to retry with a signed-in daemon; Auto fallback gives one command to check Tailscale and retry. |
| Machine output | Hub status retains one JSON document with the existing `tailscale_serve.status`, `message`, and `remedy` fields; config set retains `{key, old_value, new_value}` JSON; start refusals remain nonzero with stderr diagnostics. |
| Omitted | Raw Tailscale output, account details, auth state, and alternate daemon discovery never appear in these diagnostics. |

The existing `--no-tailscale-serve` flag remains available; `--loopback` is its
discoverable alias. An explicit `TAILSCALE_SOCKET` follows both service and
detached launches. A missing CLI retains the existing loopback fallback;
an installed CLI that is signed out or cannot be probed refuses `hub start`.

The service retries startup publication within one shared deadline. If those
attempts fail, launchd supervises the next attempt; `hub stop` removes the
bootstrap job before stopping any remaining process.

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

terminal-qa: PASS hub-restart-auto-fallback · 7 runs · 0 fail · 0 warn · 0 allowed · /Users/pippenz/.cas/artifacts/cassy-ee10031b42e3c2024023c076dfb2edd8118e39b8baee6f0832367b9e8ef4c205/cas-4c14/terminal-qa/hub-restart-auto-fallback/report.json

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | The first two lines report the stop and successful restart; the following warning names loopback-only service and gives one retry command. |
| Fit | 5 | The warning is a three-line name → cause → retry block, with one remedy rather than a paragraph of alternatives. |
| Craft | 5 | All seven captures pass at 80 columns; the longest line is 55 cells, with no wrap or clipped token. |
| Theme safety | 5 | Dark, light, both Solarized palettes, `NO_COLOR=1`, and `LC_ALL=C` pass with readable plain output. |
| Machine contract | 5 | The piped run exits 0 and appends the warning cleanly; the targeted JSON restart test passes with one JSON value. |
