# Commander v1 assembled acceptance gate

**Verdict: NOT RELEASABLE. Confidence: high.** The immutable public `v2.55.4` release is
authentic, byte-identical on two real machines, portable, and passes the clean-home and supported
TLS/HSTS rows. The first assembled phone-class browser row nevertheless fails: after pairing the
controller hub locally, real Chrome `151.0.7922.75` at `390×844` cannot complete the documented
direct pairing to the second machine. Chrome reports `MissingAllowOriginHeader` on the cross-origin
pairing exchange. The gate stopped immediately; unexecuted rows are not inferred green.

The normative contract is
[`docs/specs/2026-08-08-commander-security-architecture.md`](../specs/2026-08-08-commander-security-architecture.md),
especially `H7-ADV-01`, `H7-FLEET-02`, `H7-INVARIANT-03`, and `H7-CRASH-04`.

## Overview

| Field | Executed result |
| --- | --- |
| Question | Is Commander v1 releasable from exact public `v2.55.4` bytes? |
| Verdict | **No — controller-origin Chrome cannot complete direct pairing to machine B.** |
| Confidence | High; the failure repeated in two fresh isolated profiles and includes Chrome CDP's exact CORS classification. |
| Source | Clean `HEAD == origin/main == Commander epic` at `4c03e116d508adf69574f927bd46650f86106a07`; immutable release peel `408673b44a86ab306ab6fc59b578bf214b24d483`. |
| Public release | Annotated tag object `207b9cfe…`; official run `31315462369` terminal success; release `367487518`. |
| Linux asset | 21,794,612 bytes; archive SHA-256 `a9c8a28a…`; binary SHA-256 `d6f352a3…`. |
| Machines | `soundwave` and distinct `unicron`, Linux x86_64, same installed public binary. |
| Browser | Real Google Chrome `151.0.7922.75`; isolated phone metrics `390×844`. |
| Physical Android | Offline and explicitly **not claimed**. |
| Evidence window | 2026-08-09 13:45–13:55 UTC. |
| Author | H7 assembled release gate (`cas-3d85`). |

## Evidence

| Observation | Redacted source | What it proves |
| --- | --- | --- |
| Fresh Git and GitHub reads matched the immutable tag object, peel, official successful run, public release, asset IDs, sizes, and API SHA-256 digests. Both installed binaries independently report `cas 2.55.4 (408673b 2026-08-09)` and SHA-256 `d6f352a3…`. | Exact Git object reads; GitHub run/release API; local and batch-SSH `sha256sum` plus `cas --version` | The gate used identical installed public bytes on two real machines, not a local build. |
| On quiescent unicron, `~/.cas` was genuinely absent. The public binary created owner-only security state, served cross-machine TLS 1.3 health `200` with exactly one `Strict-Transport-Security: max-age=31536000`, stopped its owned Serve mapping, and restored the original tree byte-identically (`6824c3ec…`). | Reversible clean-home isolation; mode checks; soundwave→unicron TLS curl; before/after canonical tree digest | The clean-home startup and owned teardown row passes without an undocumented manual prerequisite. |
| Both supported TLS origins returned health `200`, unauthenticated sessions `401`, catch-all `405`, and exact HSTS. Direct loopback requests with spoofed Forwarded, X-Forwarded, or Tailscale identity headers remained plaintext-class with no HSTS. Starting a non-loopback plaintext listener exited `1`. | Forced TLS 1.3 response headers; direct-loopback negatives; installed-public CLI refusal | TLS/HSTS is listener-bound and non-loopback plaintext fails closed. |
| Wrong-Origin API, preflight, CSRF mutation, and cross-site WebSocket probes received no CORS grant or session data. | Redacted curl response codes/headers/bodies against the public soundwave origin | Pre-auth hostile-origin requests fail closed before the browser pairing failure. |
| In each of two fresh Chrome profiles, local pairing to soundwave succeeded and its authenticated session/event streams returned `200`. Direct pairing to unicron then received a `204` preflight, but Chrome stopped the Fetch with `net::ERR_FAILED` and `corsErrorStatus={corsError: MissingAllowOriginHeader}`; the UI never added unicron and remote active-device count remained zero. | `/tmp/cas-3d85-browser-v2554.exit` = `1`; `/tmp/cas-3d85-browser-v2554.log`; recoverable attempt-one receipt | This is a repeatable real-browser failure in the required controller-origin → target-hub pairing path. |
| The exact same authorized unicron preflight, issued independently, returned `204` with `Access-Control-Allow-Origin` equal to the soundwave controller origin, `Vary: Origin`, allowed `POST`/`Content-Type`, and HSTS. | `/tmp/cas-3d85-v2554-good-preflight.headers` | The browser/curl contrast isolates the missing CORS grant to the subsequent pairing-exchange response path, not the preflight or controller-origin spelling. |
| Before→after counts were soundwave Claude/Codex/Grok `0/3/0 → 0/4/0` with `4→4` logical sessions and unicron `0/0/0 → 0/0/0` with `0→0`. The sole added Codex PID is the supervisor-authorized corrective worker for `cas-ac47`; Commander created none. | Exact process counts/start times, `cas list --json`, CAS agent registry | Commander caused no model-process or logical-session multiplication; the global Codex delta is externally attributed. |
| Both attempt devices were revoked; every current invitation/profile secret moved from `/tmp` to recoverable Trash; the registered no-agent fixture stopped; the original session metadata and prior daemon-exit receipt compare byte-identically; both hubs/Serve maps stopped; active-device counts, task Chrome processes, tested listeners, and runtime records are zero. | Auth filters, server registry, `cmp`, process/socket/status assertions | The failed gate left zero active authority, service, listener, or session-state residue. |

## Reasoning chain

1. Tag, workflow, asset metadata, executable identity, and two-host digests bind the observed behavior
   to immutable public `v2.55.4` bytes.
2. Clean-home, TLS/HSTS, spoof, plaintext-refusal, and hostile-origin probes pass independently, so
   prior release defects are not being recycled as blockers.
3. Local pairing succeeds, proving the same browser, app, credential-generation path, and public hub
   can complete the ceremony without a cross-origin response.
4. Cross-machine pairing fails twice in Chrome with the same named CORS error, while an independent
   curl sees a correct preflight. This rules out a consumed invitation, target outage, spelling mismatch,
   or preflight-policy failure as an explanation for the missing browser grant.
5. Direct multi-hub pairing is a prerequisite of `H7-FLEET-02` and part of `H7-ADV-01`; one failed
   binding row makes the release verdict red. Later fan-out, restart, replay, expiry, revocation,
   backpressure, compatibility, `SIGILL`, and full assembled guards were correctly stopped.

## Acceptance matrix

| Binding gate | State | Executed conclusion |
| --- | --- | --- |
| Public artifact and two-machine install | **PASS** | Exact immutable `v2.55.4` identities and identical installed bytes proved. |
| Clean-home and TLS/HSTS | **PASS** | Genuine absent-state startup, cross-machine TLS 1.3, listener-bound HSTS, plaintext refusal, and owned teardown passed. |
| `H7-ADV-01` | **FAIL / stopped** | Hostile-origin negatives passed, but the authorized cross-origin pairing exchange omitted the CORS grant required by real Chrome. Replay/copied-key/scope/expiry/revocation rows were stopped. |
| `H7-FLEET-02` | **FAIL** | Two machines and phone-class Chrome were present, but direct machine-B pairing could not complete; multi-hub observation/control therefore could not begin. |
| `H7-INVARIANT-03` | **NOT EXECUTED in this pass** | The registered fixture remained unused; fan-out/backpressure/one-upstream proof was stopped after pairing failure. Count invariants alone are green. |
| `H7-CRASH-04` | **NOT EXECUTED in this pass** | Exact `SIGILL` followed the blocked fleet path and was correctly skipped. |
| Old/new compatibility and full assembled guards | **NOT EXECUTED in this pass** | Fail-closed policy stopped the run; no green inference is made. |

## What would falsify this verdict

A new immutable public artifact must let controller-origin Chrome complete the exact direct pairing
exchange to machine B, with the successful or generic denied exchange response carrying the authorized
origin's CORS grant as required. Corrective P0 `cas-ac47` owns that path. A fresh H7 pass must then rerun
every remaining adversarial, fan-out/backpressure, arbitration, restart/reconnect, crash, compatibility,
count, vendoring, workspace, release, ISA, and cleanup row. A local source fix, curl-only preflight, or
focused test alone cannot overturn this report.

## Next actions

| Action | Owner | Completion proof |
| --- | --- | --- |
| Correct and regress the pairing-exchange CORS response (`cas-ac47`). | Hub/auth | Real controller-origin Chrome pairs a second public hub; success and generic denial responses carry only the exact authorized ACAO plus `Vary: Origin`. |
| Publish one new immutable corrective artifact without moving prior tags. | Release owner | Exact tag peel, terminal official workflow, asset digests, strict final-ELF audit, and identical two-host install. |
| Rerun the complete binary H7 matrix. | Fresh H7 worker | Every binding row green from new public bytes; paired MD/HTML updated; zero residue. |
| Keep Slack unposted until that rerun passes and the user elects to post. | Release owner | Explicit human posting decision after a green report. |

## Cleanup and redaction

- No pairing capability, device credential, proof, private key, WebSocket ticket, Authorization value,
  tailnet IP, terminal input, prompt content, or raw audit payload is retained in this report.
- Both local attempt devices were revoked; neither remote exchange created a device. Active device count
  is zero on both hubs.
- Eight invitation files and six Chrome profiles from the two attempts were moved from `/tmp` into
  recoverable user Trash. The reversible clean-home state was also moved there after owned teardown.
- The registered fixture supervisor and child are stopped. The exact pre-run session JSON and daemon-exit
  receipt are restored byte-for-byte. Both hubs and Serve mappings are stopped; tested ports, runtime
  records, active devices, and task Chrome processes are absent.
- Physical Android remains unclaimed. Slack remains explicitly unposted. The adjacent draft is a future
  post template, not an announcement.

## Provenance

- Markdown source: `docs/reports/2026-08-09-commander-v1-acceptance-gate.md`
- Human review surface: `docs/reports/2026-08-09-commander-v1-acceptance-gate.html`
- Explicitly unposted draft: `docs/release-notes/2026-08-09-commander-v1-slack-draft.md`
- Executed source boundary: `4c03e116d508adf69574f927bd46650f86106a07`
- Immutable public peel: `408673b44a86ab306ab6fc59b578bf214b24d483`
- Commands: exact Git tag/object and GitHub release/run queries; public installed-binary SHA-256/version;
  local and batch-SSH count/state probes; reversible clean-home startup; forced TLS 1.3, HSTS, spoof,
  plaintext-refusal, wrong-Origin/CORS/CSRF/CSWSH curl probes; registered deterministic daemon fixture;
  two isolated Chrome 151 CDP attempts at `390×844`; auth/server/process/socket/receipt/session cleanup.
- Durable failure receipt: `/tmp/cas-3d85-browser-v2554.exit` and
  `/tmp/cas-3d85-browser-v2554.log`, observed 2026-08-09 13:53 UTC.
- Preflight contrast: `/tmp/cas-3d85-v2554-good-preflight.headers`, observed 2026-08-09 13:52 UTC.
