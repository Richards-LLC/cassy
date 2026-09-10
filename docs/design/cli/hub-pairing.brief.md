# Hub pairing CLI concept brief

| Field | Contract |
| --- | --- |
| **First two lines** | A failed pairing names the failed hub/code state first, then prints one copyable command that repairs that state or obtains a fresh code. |
| **Scannable** | Human errors are short name → cause → remedy lines; doctor has one `hub transport` row whose severity distinguishes loopback-only from supervised-but-not-publishable. |
| **Readable** | The operator can tell whether the code was never claimed, expired/consumed, or is blocked by hub supervision, without relay internals or secrets. |
| **Machine output** | `cas hub authorize --json` keeps one JSON document on stdout for successful completion/decline; failures remain structured CLI errors on stderr with no pairing nonce or invitation secret. |
| **Omitted** | Raw launchd/Tailscale transcripts and relay request details remain in `hub.log`/verbose diagnostics; the default screen carries only the next action. |

## Critique

| Criterion | Score | Evidence / follow-up |
| --- | ---: | --- |
| First two lines | 4/4 | Supervised-unavailable and consumed/expired-code errors put the state first and provide one copyable recovery command. |
| Scannable | 4/4 | Doctor labels the supervised transport failure; relay errors avoid raw transport details. |
| Readable | 4/4 | Messages distinguish unavailable publication, unclaimed pairing, consumed codes, and expired codes. |
| Machine output | 3/4 | Existing JSON remains structured; no pairing secrets are added. A full successful relay requires live Cloud credentials and was not exercised in the isolated-home sweep. |
| Width/locale | 3/4 | `terminal-qa: PASS cas-hub-authorize-help-120 · 5 runs · 0 fail · 0 warn · 0 allowed` (`/home/pippenz/.cas/artifacts/cas-3fb7/terminal-qa-help120/report.json`). The service dry-run still has a long absolute `ExecStart` line at 80 columns and an em dash under `LC_ALL=C`; retained as a documented pre-existing presentation finding. |

The CLI surface gate is a real-build terminal capture. Full launchd execution and a successful Cloud pairing need a supervised macOS/Cloud environment, so those paths are covered by deterministic unit/integration tests and the bounded isolated-home matrix rather than claimed as locally exercised.
