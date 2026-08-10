# Commander v1 assembled acceptance gate

**State: IN PROGRESS — no release verdict. Confidence: high.** The immutable public `v2.60.0`
release is authentic, and soundwave pairing from the prowl controller origin succeeded in real Chrome
`151.0.7922.108` at `390×844`. During execution the operator rebound the binding two-machine scope to
**prowl + soundwave only**. The attempted unicron exchange reported `MissingAllowOriginHeader`, but
unicron is now explicitly outside Commander v1 acceptance; that result is preserved as an anomaly,
not a failed gate row. The binding live matrix remains incomplete.

The normative contract is
[`docs/specs/2026-08-08-commander-security-architecture.md`](../specs/2026-08-08-commander-security-architecture.md),
especially `H7-ADV-01`, `H7-FLEET-02`, `H7-INVARIANT-03`, and `H7-CRASH-04`.

## Overview

| Field | Executed result |
| --- | --- |
| Question | What is the current state of Commander v1 acceptance from exact public `v2.60.0` bytes? |
| Verdict | **In progress — prowl + soundwave is the binding topology; its live matrix is incomplete.** |
| Confidence | High; the scope decision and every executed receipt are explicit, while deferred rows are not inferred. |
| Source | Clean `HEAD == origin/main` at `4636518f121054b612bef56ddf770b2d8a72ef63`; immutable release peel `0cb8962d`. |
| Public release | Annotated tag object `20c42c8f…`; official run `31412591350` terminal success. |
| Linux asset | 21,991,526 bytes; archive SHA-256 `b2533266…`; binary SHA-256 `8ec9dea6…`. |
| Binding machines | `prowl` controller hub and distinct `soundwave` target hub. Unicron and shield are out of scope. |
| Browser | Real Google Chrome `151.0.7922.108`; isolated phone metrics `390×844`; controller origin `prowl`. |
| Physical Android | Offline and explicitly **not claimed**. |
| Evidence window | 2026-08-10 19:29–19:37 UTC. |
| Author | H7 assembled release gate (`cas-3d85`). |

## Evidence

| Observation | Redacted source | What it proves |
| --- | --- | --- |
| Fresh public-release metadata and installed-binary checks matched the immutable tag peel, official successful run, asset size/digests, and exact binary identity. Both installed binaries independently report `cas 2.60.0 (0cb8962 2026-08-10)` and SHA-256 `8ec9dea6…`. | Git/GitHub release receipts; local and batch-SSH `sha256sum` plus `cas --version` | The gate used identical installed public bytes on two real machines, not a local build. |
| Unicron began quiescent, then its public hub created the sole owned Serve mapping. A fresh handshake verified TLS 1.3 / `TLS_AES_128_GCM_SHA256`; health was `200` with exactly one `Strict-Transport-Security: max-age=31536000`. Owned teardown returned Serve to `{}`. | `cas hub status`; `tailscale serve status --json`; OpenSSL; HTTPS health; owned stop receipt | The second-machine public TLS lane was real, verified, and left no mapping or listener residue. |
| Both supported TLS origins returned health `200`, unauthenticated sessions `401`, catch-all `405`, and exact HSTS. Direct loopback requests with spoofed Forwarded, X-Forwarded, or Tailscale identity headers remained plaintext-class with no HSTS. Starting a non-loopback plaintext listener exited `1`. | Forced TLS 1.3 response headers; direct-loopback negatives; installed-public CLI refusal | TLS/HSTS is listener-bound and non-loopback plaintext fails closed. |
| Wrong-Origin API, preflight, CSRF mutation, and cross-site WebSocket probes received no CORS grant or session data. | Redacted curl response codes/headers/bodies against the public soundwave origin | Pre-auth hostile-origin requests fail closed before the browser pairing failure. |
| In one fresh Chrome run, pairing to soundwave succeeded from exact controller origin `https://prowl…` and authenticated sessions/events returned `200`. The subsequent out-of-scope unicron attempt received preflight `204`, then Fetch failed with `MissingAllowOriginHeader`; unicron was never added. | Durable mode-0600 `artifacts/cas-3d85/wild-leopard-18/browser.exit` = `1` and `browser.log` | The binding prowl→soundwave pairing prerequisite is green. The unicron result is preserved only as a separate anomaly. |
| The deterministic no-agent two-pane fixture was ready with exact PID/start fingerprint and protocol-v2 capabilities, but its event ledger recorded zero connections before the fleet gate failed. | `fixture-state.json`; `fixture-events.jsonl`; preserved session metadata | Fan-out, arbitration, restart, and crash rows were not accidentally exercised or inferred green. |
| Before→after counts were soundwave Claude/Codex/Grok `2/0/0 → 1/0/0` with `7→6` logical sessions and unicron `0/0/0 → 0/0/0` with `0→0`. The independently named `soundwave-config-loyal-jaguar-56` session and one Claude process ended during the window; Commander created none. | Exact process counts and session-name diff | Commander caused no model-process or logical-session multiplication; the only delta is an externally owned session ending. |
| The sole test-created soundwave device was revoked; unicron created no device. Exact invitations and three isolated Chrome profiles moved to recoverable Trash; fixture metadata and prior exit receipt compare byte-identically; task-started unicron hub/Serve stopped; soundwave's persistent service remained PID `2256851`, active/running, `NRestarts=0`. | Guarded auth filters, `cmp`, process/socket/status assertions | The checkpoint left zero active test authority, browser process, fixture process, or unicron hub/Serve residue. |

## Reasoning chain

1. Tag, workflow, asset metadata, and executable identity bind the observed behavior to immutable
   public `v2.60.0` bytes.
2. Banked prowl/soundwave TLS and hostile-origin receipts remain the accepted pre-auth subset.
3. Real Chrome paired soundwave from the prowl controller origin and reached authenticated session and
   event APIs, proving the binding fleet prerequisite through pairing.
4. The operator explicitly removed unicron and shield from Commander v1 scope. The captured unicron
   CORS result therefore cannot decide H7 and is retained only as anomaly evidence.
5. The fixture received zero upstream connections, so fan-out, arbitration, restart, replay,
   compatibility, `SIGILL`, and final counts remain unexecuted. The honest state is in progress.

## Acceptance matrix

| Binding gate | State | Executed conclusion |
| --- | --- | --- |
| Public artifact | **PASS** | Exact immutable public `v2.60.0` identity and soundwave installed bytes proved; prowl public hub identity is banked. |
| Clean-home and TLS/HSTS | **PASS** | Genuine absent-state startup, cross-machine TLS 1.3, listener-bound HSTS, plaintext refusal, and owned teardown passed. |
| `H7-ADV-01` | **IN PROGRESS** | Banked hostile-origin subset and binding soundwave pairing are green; replay/copied-key/scope/expiry/revocation remain to execute. |
| `H7-FLEET-02` | **IN PROGRESS** | Binding prowl + soundwave topology and phone-class Chrome are present; direct soundwave pairing is green, while observation/control remains to execute. |
| `H7-INVARIANT-03` | **NOT EXECUTED in this pass** | The registered fixture remained unused; fan-out/backpressure/one-upstream and final count proof remain deferred. |
| `H7-CRASH-04` | **NOT EXECUTED in this pass** | Exact `SIGILL` followed the blocked fleet path and was correctly skipped. |
| Old/new compatibility and full assembled guards | **NOT EXECUTED in this pass** | Deferred to the fresh prowl + soundwave continuation; no green inference is made. |

## Continuation protocol

A fresh worker must use prowl as the controller origin and soundwave as the sole target machine. Reuse
the banked public-release, TLS/pre-auth, soundwave pairing, and cleanup receipts; do not re-enter unicron
or shield. Revive the deterministic fixture, mint fresh soundwave invitations, and run real Chrome at
`390×844` through multi-viewer fan-out, controller arbitration, replay/copied-key/scope/revocation,
restart/reconnect, compatibility, no-polling/count, and exact `SIGILL` diagnostics. Then run scoped
assembled/vendor/release/ISA proof and update the verdict from executed evidence only.

## Next actions

| Action | Owner | Completion proof |
| --- | --- | --- |
| Investigate the out-of-scope unicron pairing CORS anomaly separately. | Hub/auth follow-up | Reproduction and root cause are tracked outside the Commander v1 acceptance verdict. |
| Complete the rebound prowl + soundwave binary H7 matrix. | Fresh H7 worker | Every binding row executed from public bytes; paired MD/HTML updated; zero residue. |
| Keep Slack unposted until that rerun passes and the user elects to post. | Release owner | Explicit human posting decision after a green report. |

## Cleanup and redaction

- No pairing capability, device credential, proof, private key, WebSocket ticket, Authorization value,
  tailnet IP, terminal input, prompt content, or raw audit payload is retained in this report.
- The one soundwave attempt device was revoked; the remote exchange created no device. Active device
  count is zero on both hubs.
- Four invitation files, their temporary links, and three Chrome profiles were moved into recoverable
  user Trash after the attempt.
- The registered fixture supervisor and child are stopped. The exact pre-run session JSON and daemon-exit
  receipt are restored byte-for-byte. The task-started unicron hub and Serve mapping are stopped;
  soundwave's operator service is unchanged; active test devices and task Chrome processes are absent.
- Physical Android remains unclaimed. Slack remains explicitly unposted. The adjacent draft is a future
  post template, not an announcement.

## Provenance

- Markdown source: `docs/reports/2026-08-09-commander-v1-acceptance-gate.md`
- Human review surface: `docs/reports/2026-08-09-commander-v1-acceptance-gate.html`
- Explicitly unposted draft: `docs/release-notes/2026-08-09-commander-v1-slack-draft.md`
- Executed source boundary: `4636518f121054b612bef56ddf770b2d8a72ef63`
- Immutable public peel: `0cb8962d`
- Commands: exact Git tag/object and GitHub release/run queries; public installed-binary SHA-256/version;
  local and batch-SSH count/state probes; reversible clean-home startup; forced TLS 1.3, HSTS, spoof,
  plaintext-refusal, wrong-Origin/CORS/CSRF/CSWSH curl probes; registered deterministic daemon fixture;
  one isolated Chrome 151 CDP attempt at `390×844`; auth/server/process/socket/receipt/session cleanup.
- Durable failure receipts: `artifacts/cas-3d85/wild-leopard-18/browser.exit`, `browser.log`,
  `fixture-state.json`, and `fixture-events.jsonl`, observed 2026-08-10 19:29–19:37 UTC.
