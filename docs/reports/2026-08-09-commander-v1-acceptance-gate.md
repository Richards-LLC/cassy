# Commander v1 assembled acceptance gate

**Verdict: NOT RELEASABLE. Confidence: high.** The immutable public `v2.55.3` release is
authentic, byte-identical on two real machines, portable, and fixes the prior clean-home,
`SIGILL`-evidence, and HSTS defects at source. The assembled phone-class run nevertheless found a
new binding release blocker: with three paired browser profiles attached, the documented public
`cas hub restart --tailscale-serve` command exited `1` because the replacement hub did not become
ready. The post-failure log reported that another hub instance still held the machine lock. The gate
stopped immediately; unexecuted rows are not inferred green.

The normative contract is
[`docs/specs/2026-08-08-commander-security-architecture.md`](../specs/2026-08-08-commander-security-architecture.md),
especially `H7-ADV-01`, `H7-FLEET-02`, `H7-INVARIANT-03`, and `H7-CRASH-04`.

## Overview

| Field | Executed result |
| --- | --- |
| Question | Is Commander v1 releasable from exact public `v2.55.3` bytes? |
| Verdict | **No — documented hub restart failed in the assembled path.** |
| Confidence | High; the failure is a durable exit-`1` receipt from the installed public binary. |
| Source | Clean `HEAD == origin/main == Commander epic == v2.55.3 peel` at `bae58630428d314a6c17361c3439f1c7d38b9f9b`. |
| Public release | Annotated tag object `dfd5ce96…`; official run `31311272403` terminal success; release `367465091`. |
| Linux asset | 21,793,999 bytes; archive SHA-256 `04c853c6…`; binary SHA-256 `44d72b47…`. |
| Machines | `soundwave` and distinct `unicron`, Linux x86_64, same installed public binary. |
| Browser | Real Google Chrome `151.0.7922.75`; isolated phone metrics `390×844`. |
| Physical Android | Offline and explicitly **not claimed**. |
| Evidence window | 2026-08-09 12:05–12:22 UTC. |
| Author | H7 assembled release gate (`cas-3d85`). |

## Evidence

| Observation | Redacted source | What it proves |
| --- | --- | --- |
| Fresh downloads matched public API asset IDs, sizes, and SHA-256 digests. Both archives contain only `cas` and `LICENSE`; the Linux binary reports `cas 2.55.3 (bae5863 2026-08-09)` and passed the strict full-executable zero-EVEX audit. | Git tag/object reads, GitHub run/release API, independent downloads under `/tmp/cas-3d85-v2553-public.4S9kQo`, repository ISA guard | The tested artifact is the immutable public release, not a local build. |
| Installed binaries on soundwave and unicron independently matched public SHA-256 `44d72b47…` and exact embedded version. | Local and batch-SSH `sha256sum` plus `cas --version` | Both real machines ran identical public bytes. |
| On quiescent unicron, the original 32 KB `~/.cas` tree was moved aside after hashing. The public binary started with `~/.cas` genuinely absent, created owner-only state, served cross-machine TLS health `200`, stopped with owned Serve teardown, and left the original tree byte-identical after restoration (`73ece4a7…`). | Reversible clean-home isolation; mode scan; cross-machine TLS 1.3 curl; before/after tree digest | The previous undocumented clean-host prerequisite is corrected in the public artifact. |
| Both public TLS origins emitted exactly `Strict-Transport-Security: max-age=31536000` with CSP, referrer, nosniff, and frame controls. Direct loopback requests with spoofed Forwarded, X-Forwarded, and Tailscale identity headers remained plaintext-class and omitted HSTS. | Forced TLS 1.3 response headers from both machines plus direct-loopback negative probes | The corrective HSTS behavior is bound to the supported TLS listener rather than spoofable headers. |
| Three isolated Chrome profiles paired under the soundwave controller origin: controller A, controller B, and a read-only observer. The phone-class path opened the deterministic two-pane session, established one upstream, acquired and transferred control, emitted resize, targeted interrupt, and attributed semantic-message variants. | Durable browser attempt `.exit.3`/`.log.3`; sanitized fixture JSONL | The public browser/hub path reached real pairing, multi-viewer observation, arbitration, and control before the failing restart row. |
| Fixture telemetry recorded maximum concurrent upstream connections `1`, one `Attach`, six `ResizePane`, one `InterruptPane`, one `SendMessage`, then one disconnect. | `/tmp/cas-3d85-fixture-events.jsonl`, 11 sanitized rows, 12:18 UTC | Multiple downstream viewers reused one daemon/session upstream; Commander did not multiply the fixture connection. |
| The exact public command `/home/pippenz/.local/bin/cas --json hub restart --tailscale-serve` exited `1`: `cas hub did not become ready`. After the command returned, the runtime record and listener were absent and Serve was `{}`; `hub.log` contained `another cas hub instance already holds the machine lock`. No hub process or lock-owning file descriptor was discoverable then. | `/tmp/cas-3d85-browser-v2553.exit.3` = `1`; `/tmp/cas-3d85-browser-v2553.log.3`; public hub status, Serve status, socket/process/FD probes, hub log | The documented assembled restart/reconnect row fails from exact public bytes. The evidence does not yet claim the deeper lock-race root cause. |
| Before counts were soundwave Claude/Codex/Grok `0/3/0` with five exact session metadata names, and unicron `0/0/0` with zero. After cleanup they were soundwave `0/4/0`, five; unicron `0/0/0`, zero. The sole added Codex PID `3212064` is independently registered as the supervisor-authorized restart-lock fix worker; it was not created by Commander. | Exact `ps -C` PIDs/start times, session filename inventory, CAS agent registry | Commander created no model process or logical session. The global Codex delta is externally attributed rather than mislabeled unchanged. |
| Final cleanup revoked every device issued during this pass on both hubs; removed all pairing invitation and Chrome-profile secrets from `/tmp` into recoverable Trash; stopped the registered fixture and remote hub; restored the pre-run session JSON byte-for-byte; and proved empty Serve maps, no 39459/4173/backend listener, runtime record, active Serve receipt, or registered server. | Auth-list filters, CAS server registry, `diff`, socket/process/status probes | The failed gate left no active authority, service, listener, or session-metadata mutation. |

## Reasoning chain

1. Tag, workflow, archive, executable, and two-host digest evidence establish that the observed behavior
   belongs to immutable public `v2.55.3` bytes.
2. Clean-home and HSTS corrective rows pass independently, so neither earlier defect is being carried
   forward as a false blocker.
3. The browser reached the assembled multi-viewer control path and the fixture saw one upstream before
   the restart command ran. This rules out a source-only or idle-hub substitution.
4. The documented restart command returned exit `1` and did not produce a ready replacement hub.
   Restart/reconnect is explicit acceptance scope, so this failure alone makes the verdict binary red.
5. Per fail-closed gate policy, later adversarial expiry/backpressure/compatibility/`SIGILL` rows and full
   source guards were not run after the failure. Prior releases and local tests cannot fill those cells.

## Acceptance matrix

| Binding gate | State | Executed conclusion |
| --- | --- | --- |
| Public artifact and two-machine install | **PASS** | Exact immutable `v2.55.3` identities and identical installed bytes proved. |
| Clean-home and TLS/HSTS | **PASS** | Genuine absent-state startup, cross-machine TLS 1.3, listener-bound HSTS, and owned teardown passed. |
| `H7-ADV-01` | **PARTIAL / stopped** | Real pairing and exact-origin TLS path executed; replay/copied-key/scope/revocation work was staged but the gate stopped before a durable result. Expiry was not run. |
| `H7-FLEET-02` | **FAIL** | Two machines, phone-class Chrome, pairing, observation, and arbitration reached the documented restart, which failed to become ready. |
| `H7-INVARIANT-03` | **PARTIAL / stopped** | One upstream and zero Commander-created process/session multiplication proved. Slow-viewer/backpressure completion was not run after restart failure. |
| `H7-CRASH-04` | **NOT EXECUTED in this pass** | The fixture remained available, but the planned exact `SIGILL` step came after restart and was correctly skipped. |
| Old/new compatibility and full assembled guards | **NOT EXECUTED in this pass** | Fail-closed policy stopped the run before these rows; no green inference is made. |

## What would falsify this verdict

A new immutable public artifact must make the same documented Tailscale Serve restart reach a ready
hub without a lock collision, preserve the stable machine identity and public URL, and reconnect the
phone-class clients without restoring a controller lease automatically. A fresh H7 run must then pass
every remaining adversarial, fan-out/backpressure, crash, compatibility, count, vendoring, workspace,
release, and cleanup row. A local source fix or focused test alone cannot overturn this report.

## Next actions

| Action | Owner | Completion proof |
| --- | --- | --- |
| Correct and regress the public hub restart lock handoff (`cas-bb90`). | Hub/runtime | Real Tailscale Serve start → paired load → restart → ready receipt, with stable identity/URL and no orphan lock/listener. |
| Publish one new immutable corrective artifact without moving prior tags. | Release owner | Exact tag peel, terminal official workflow, asset digests, strict final-ELF audit, and identical two-host install. |
| Rerun the complete binary H7 matrix. | Fresh H7 worker | Every binding row green from the new public bytes; paired MD/HTML report updated; zero-residue proof. |
| Keep Slack unposted until that rerun passes and the user elects to post. | Release owner | Explicit human posting decision after a green report. |

## Cleanup and redaction

- No pairing capability, device credential, proof, private key, WebSocket ticket, Authorization value,
  tailnet IP, terminal input, prompt content, or raw audit payload is retained in this report.
- A stale unrevoked H7 credential left by the previously crashed worker was found at baseline and
  revoked before this run. The interrupted harness device and all devices created by the durable run
  were also revoked; active task device count is zero on both hubs.
- Thirteen task-scoped invitation/profile paths were moved out of `/tmp` into the user Trash rather
  than irreversibly deleted. They are recoverable until Trash is emptied.
- The fixture supervisor and child are stopped, the registered server reports stopped, the exact
  pre-run session JSON is restored, both hubs and Serve mappings are stopped, and tested ports are
  unbound. Physical Android remains unclaimed.
- Slack remains explicitly unposted. The adjacent draft is a future post template, not an announcement.

## Provenance

- Markdown source: `docs/reports/2026-08-09-commander-v1-acceptance-gate.md`
- Human review surface: `docs/reports/2026-08-09-commander-v1-acceptance-gate.html`
- Explicitly unposted draft: `docs/release-notes/2026-08-09-commander-v1-slack-draft.md`
- Binding source commit: `bae58630428d314a6c17361c3439f1c7d38b9f9b`
- Commands: Git tag/object and GitHub release/run queries; independent `curl`, SHA-256, tar, file,
  version, and ISA checks; local and batch-SSH hub/status/count probes; forced TLS 1.3 and spoofed
  loopback requests; registered deterministic daemon fixture; Chrome 151 CDP at `390×844`; exact
  public `cas --json hub restart --tailscale-serve`; CAS auth/server cleanup and final process/socket/
  receipt/session-diff assertions.
- Durable browser failure receipt: `/tmp/cas-3d85-browser-v2553.exit.3` and
  `/tmp/cas-3d85-browser-v2553.log.3`, observed 2026-08-09 12:18 UTC.
