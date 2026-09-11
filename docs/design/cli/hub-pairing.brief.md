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

### Update recovery and Tailscale cause preservation

`cas hub authorize` now keeps the persisted Tailscale Serve failure in its
error, names where the CLI was sought when the macOS app fallback is missing,
and offers `cas hub restart --tailscale-serve` without telling an actively
supervised hub to uninstall its service. Real-build captures and the honest
demo ledger are under `/Users/pippenz/.cas/artifacts/cas-5722/`.

terminal-qa: PASS cas-5722-authorize-cause · 11 runs · 0 fail · 0 warn · 0 allowed · `/Users/pippenz/.cas/artifacts/cas-5722/terminal-qa-authorize-cause/report.json`

| Criterion | Score | Evidence / follow-up |
| --- | ---: | --- |
| First two lines | 4/4 | The error names the missing public URL first and gives one restart command. |
| Scannable | 4/4 | The Tailscale failure cause is retained without launchd internals. |
| Readable | 4/4 | The macOS fallback search locations and restart remedy are explicit. |
| Machine output | 4/4 | JSON success remains one document; failure diagnostics remain on stderr. |
| Width/locale | 4/4 | Terminal QA passes the human error across its width, palette, pipe, and locale matrix. |

Scored by the worker on 2026-09-11; floor holds. The demo happy path was not
exercised because no valid Commander code was available; deterministic relay
tests cover invitation delivery.
