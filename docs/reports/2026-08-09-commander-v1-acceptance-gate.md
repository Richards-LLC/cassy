# Commander v1 assembled acceptance gate

**Verdict: RELEASABLE at public `v2.62.0`. Confidence: high.** A fresh 2026-08-11 run used the
byte-identical public Linux artifact on soundwave and unicron plus real Chrome 151 at `390×844`.
It completed pairing, direct two-machine access, three-viewer fan-out, controller arbitration,
security abuse cases, held-viewer hub restart, revocation, `SIGILL` attribution, daemon restart, and
protocol-v1 compatibility. The restart row that failed on `v2.61.1` now completes with one replacement
hub, the same public URL, viewer reconnection, and no restored controller lease. Cleanup left no live
device, hub, Serve mapping, fixture, browser profile, invitation, or temporary session.

The normative contract is
[`docs/specs/2026-08-08-commander-security-architecture.md`](../specs/2026-08-08-commander-security-architecture.md),
especially `H7-ADV-01`, `H7-FLEET-02`, `H7-INVARIANT-03`, and `H7-CRASH-04`.

## Overview

| Field | Executed result |
| --- | --- |
| Question | Does a fresh full two-machine phone-class rerun against exact public `v2.62.0` complete every binding H7 row? |
| Verdict | **Yes. Every binding row completed, including the formerly failing held-viewer restart.** |
| Confidence | High; immutable release identity, installed bytes, real Chrome DOM/network behavior, fixture ledgers, process-bound crash evidence, security responses, source guards, and zero-residue cleanup were observed directly. |
| Public source boundary | Annotated tag object `6fdbe6278ac6e23bb1e7c837bd4a2d923be0f091` peels `8e91de67c6071de03c8ff7b685e766a888b0629f`; it contains held-viewer regression/fix commits `1949e1cf` and `e2f93c8b`. |
| Public release | `v2.62.0`; GitHub release `368756555`, published 2026-08-11 18:03:59 UTC, neither draft nor prerelease; official run `31518292281` completed successfully. |
| Linux x86-64 asset | 22,075,487 bytes; archive SHA-256 `1af795fd10ede756039a0a33cbee3d9bd5f56d07a902ecc93aad852f29255ba3`; extracted binary SHA-256 `4ab8facad0413706f21fd505463c59e64ba2f38766d9b872c685f95350152f28`. |
| macOS ARM64 asset | 19,453,201 bytes; archive SHA-256 `4e8fb19e7a757b4b564db937ecf46956e83858e53d99fcd5c73cc1bf1c52ac35`. |
| Binding machines | `soundwave` controller hub and distinct `unicron` target hub, both running the exact public Linux binary. |
| Browser | Google Chrome `151.0.7922.108`; isolated phone metrics `390×844`; controller origin `soundwave`. Physical Android was offline and is not claimed. |
| Evidence window | 2026-08-11 18:11–18:20 UTC. |
| Author | H7 assembled release gate (`cas-3d85`), worker `sharp-falcon-63`. |

## Evidence

| Observation | Redacted source | What it proves |
| --- | --- | --- |
| Fresh GitHub reads tied tag object `6fdbe627…`, peel `8e91de67…`, successful run `31518292281`, and both public asset metadata/digests together. Fresh Linux download contained only `cas` and `LICENSE`; the ELF reported `2.62.0 / 8e91de6`. | GitHub tag/release/workflow reads; fresh archive manifest and hashes | The gate used immutable public bytes, not post-release source or a local build. |
| Soundwave's dirty same-version binary SHA `d82597dd…` and unicron's public `v2.61.1` SHA `fb0fc976…` were preserved under SHA-addressed backups, then atomically replaced. Both machines ran public SHA `4ab8faca…`. | Install command receipts and post-install hashes | Both real machines executed one byte-identical published artifact; rollback evidence exists. |
| Three isolated browser profiles paired to soundwave (two controllers, one read-only observer); controller A acquired the lease, B observed then force-took it, A became observer, and the read-only device's lease/interrupt/message controls remained disabled. A also paired directly to unicron. | `browser-result-v2620.json`; phone screenshot | `H7-FLEET-02`: direct two-machine phone-class access and explicit single-controller arbitration work. |
| Resize, input, targeted interrupt, and attributed message reached the deterministic two-pane daemon. Across three local viewers the fixture recorded maximum concurrent upstreams `1`; after hub reconnect it again recorded one upstream. | `fixture-events-v2620.jsonl` | `H7-INVARIANT-03`: downstream fan-out does not multiply daemon/session upstreams. |
| With live viewers attached, exact public `cas hub restart --tailscale-serve` replaced PID `513028` with PID `546134`, preserved port `4173` and `https://soundwave-linux.tailf5a734.ts.net/`, reconnected B/C, and did not restore the lease. | `browser-result-v2620.json` `hubRestart` | The `v2.61.1` 10-second failure is corrected in `v2.62.0`; restart preserves singleton ownership and security state. |
| DPoP first use returned `200`; the replay, copied-key proof, method mismatch, observer scope escalation, consumed pairing replay, and second WebSocket-ticket use returned `401`/rejected. Revoking A moved it to `auth-blocked` while B stayed connected. | Browser `auth` and `arbitration` results | `H7-ADV-01`: replay, copied state, scope escalation, and revocation fail closed without taking down another authorized viewer. |
| Malicious-Origin preflight, malicious-Origin GET, missing-Origin GET, and cross-site WebSocket upgrade each returned `401` with no CORS grant. The HTTPS root emitted CSP, no-referrer, nosniff, and exact HSTS `max-age=31536000`; a `0.0.0.0` plaintext start exited `1`. | Live curl/header matrix; `nonloopback.stderr` | Hostile Origin/CORS/CSRF/CSWSH and non-loopback plaintext paths are rejected while the trusted TLS surface retains required headers. |
| Exact fixture PID `536258` / starttime `5835684` died by signal `4`; its durable receipt says `core_dumped=true`. Chrome rendered actionable `SIGILL` remediation, other sessions stayed `200`, and direct unicron health stayed `200`. | `sigill-receipt-v2620.json`; browser `crash` result | `H7-CRASH-04`: diagnosis is bound to exact process evidence and failure is isolated. |
| A protocol-v1 fixture then restarted against the same v2.62.0 client profile; Chrome reconnected at `390×844` and rendered both panes. | `browser-compat-result-v2620.json`; legacy fixture ledger | Daemon restart and old-daemon/new-client compatibility remain additive. |
| The browser trace contains hub event streams and explicit session actions only; it contains no agent/model endpoint. Real logical session records remained `5→5` on soundwave and `0→0` on unicron; same-method ambient process counts decreased `25/24/3→22/19/3` on soundwave and stayed unchanged on unicron. | Browser `networkSummary`; before/after process/session receipts | Commander observed existing work without creating a model process, request, or logical CAS session. |
| Exact public ELF passed the strict zero-EVEX/AVX-512 audit. Vendored Ghostty WASM hashes matched `6b1df1a9…` and `75cb147e…`; Ghostty and T3 MIT notices are present. The official release jobs and fresh migration/web/hub guards are green. | ISA script, vendor hashes/notices, official run, scoped guard logs | Release, portability, vendoring, attribution, and assembled source gates are satisfied. |
| Every task device was revoked, both hubs stopped, both Serve maps returned `{}`, no `39459`/`4173` listener remained, real session counts stayed unchanged, and credential-bearing profiles/invitations plus the task-local hub home were moved to recoverable Trash. | Final auth/hub/Serve/listener/session receipts | The completed run left zero live authority or runtime residue. |

## Reasoning chain

1. Immutable tag, workflow, asset, installed-byte, and ISA checks bind the live behavior to public
   `v2.62.0` and rule out a local-source substitution.
2. Real Chrome on the soundwave origin paired to both real machines and exercised fan-out,
   arbitration, control, and exact one-upstream behavior before restart.
3. The same held-viewer condition that blocked `v2.61.1` now produces one replacement hub with stable
   address and reconnecting viewers. Loss of the pre-restart lease confirms restart does not silently
   restore authority.
4. The run continued through adversarial authentication, revocation, exact `SIGILL` evidence, failure
   isolation, and protocol-v1 daemon recovery; no historical row was substituted for a fresh binding row.
5. Network, process, and session evidence shows no model/session multiplication. Cleanup proves no
   surviving test authority or runtime can make a later observation look green.

## Acceptance matrix

| Binding gate | State | Executed conclusion |
| --- | --- | --- |
| Public artifact | **PASS** | Immutable v2.62.0 tag/run/assets and byte-identical two-host installs proved. |
| `H7-ADV-01` | **PASS** | Origin/CORS/CSRF/CSWSH, DPoP replay/key/method, pairing/ticket replay, scope escalation, and revocation rejected. |
| `H7-FLEET-02` | **PASS** | Soundwave + unicron, phone Chrome, direct view/control, TLS, arbitration, and attribution passed. |
| `H7-INVARIANT-03` | **PASS** | Three viewers used one upstream; network/session/process evidence shows no agent or session multiplication. |
| `H7-CRASH-04` | **PASS** | Exact SIGILL receipt produced actionable diagnosis while other sessions and the remote hub remained healthy. |
| Hub/daemon restart and compatibility | **PASS** | Held-viewer hub restart recovered; lease stayed dropped; protocol-v1 daemon restart reattached. |
| Assembled release/vendor/ISA | **PASS** | Official release, fresh scoped guards, WASM pins/notices, migration guard, and exact public ELF audit passed. |
| Cleanup and authority | **PASS** | Zero active devices, hubs, Serve mappings, fixture/browser processes, listeners, or temporary sessions. |

## What would falsify this

This verdict would be overturned by a reproducible public-`v2.62.0` run in the documented topology
that starts a second daemon upstream, restores a lease after restart, accepts any denied security case,
misattributes an exact crash, multiplies model/session state, or leaves active authority after cleanup.

## Next actions

| Action | Owner | Completion proof |
| --- | --- | --- |
| Review the paired report and release draft. | Commander release owner | Markdown/HTML fidelity and redaction checks pass on the pushed commit. |
| Keep Slack unposted until explicit user approval. | Release owner | Approval receipt, then User and Dev posts in rubric order. |
| Preserve v2.61.1 as historical failed-gate evidence. | Maintainers | No tag rewrite; this report names the exact v2.62.0 contrast. |

## Cleanup and redaction

- No pairing capability, long-lived credential, proof, private key, WebSocket ticket, Authorization
  value, tailnet IP, terminal content, prompt content, or raw secret is retained in the report or repo.
- All H7 devices are revoked. Both hubs and the fixture are stopped; Tailscale Serve is `{}` on both
  hosts; no task listener, runtime record, Chrome process, or temporary logical session remains.
- Credential-bearing browser profiles, consumed invitations, and the task-local hub home were moved to
  recoverable Trash after the non-secret crash receipt was preserved.
- Pre-run binaries remain in SHA-addressed backups. Physical Android remains unclaimed. Slack remains
  explicitly unposted.

## Provenance

- Markdown source: `docs/reports/2026-08-09-commander-v1-acceptance-gate.md`
- Human review surface: `docs/reports/2026-08-09-commander-v1-acceptance-gate.html`
- Explicitly unposted draft: `docs/release-notes/2026-08-09-commander-v1-slack-draft.md`
- Fresh durable artifacts: `.cas/artifacts/cas-3d85/sharp-falcon-63/`, observed 2026-08-11
  18:11–18:20 UTC: checked harnesses, redacted browser results, fixture ledgers, exact crash receipt,
  phone render, non-loopback refusal, and cleanup assertions.
- Commands: Git tag/ancestry reads; GitHub release/workflow reads; fresh public archive hashing; exact
  installed-binary reads; batch SSH; hub start/restart/stop; Tailscale Serve status; Chrome CDP at
  `390×844`; curl hostile-origin matrix; guarded revocation; strict ISA, vendor pin/license, migration,
  scoped hub, typecheck, browser-unit, and production-build guards.
