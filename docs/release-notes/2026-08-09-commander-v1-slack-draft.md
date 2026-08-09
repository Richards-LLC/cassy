# Commander v1 runtime release — v2.55.1 unposted Slack drafts

> **DO NOT POST.** The immutable public `v2.55.0` Linux artifact fails the final-executable portable
> x86_64 audit and is superseded before installation; its tag, release, and assets remain unchanged.
> `v2.55.1` is published and identically installed on two real machines, but the H7 verdict is
> **NOT RELEASABLE**. The assembled run found two binding blockers: the public hub cannot bootstrap
> on a clean installed host without an undocumented manual `~/.cas` creation step, and an exact
> daemon `SIGILL` is reported to real Chrome as `unknown`. Do not post these drafts for `v2.55.1`.
> Re-review only after a new immutable release contains both fixes and the complete H7 matrix reruns
> green from that published artifact.

Destination after the gate passes: `#cas-internal` (`C0B44GUKDK2`). These are the two distinct
top-level runtime-release posts required by `docs/RELEASE_SLACK_RUBRIC.md`; they are not threaded
replies. Status: **unposted**.

## User-perspective top-level post

**Live on production · User · v2.55.1**

Was: checking work across machines meant opening each terminal separately. → Now: Commander gives you
one phone-friendly view of your paired CAS machines, with live panes and deliberate, secure control.

- Pair a machine once, then see its sessions and terminal output directly over your private network.
- Watch the same session from more than one screen while one clearly identified controller holds input.
- Reconnect after hub or daemon restarts without hiding what stopped or inventing a recovery state.
- Revoke a device when needed; expired, replayed, cross-site, or over-scoped access is refused.
- Commander observes existing work without creating additional sessions or model requests.

## Dev-perspective top-level post

**Live on production · Dev · v2.55.1**

Was: CAS exposed machine-local daemon state without a browser-safe fleet boundary. → Now: each machine
runs a loopback Commander hub with tailnet TLS, exact-origin proof-of-possession auth, one upstream per
daemon session, and bounded downstream fan-out.

- Controller-origin IndexedDB stores a non-extractable P-256 key and origin-bound device credential;
  pairing capabilities and WebSocket tickets are short-lived and single-use.
- Exact Origin/CORS handling, DPoP method/URI/credential binding, replay caches, per-operation scopes,
  revocation, and attributed audit fail closed.
- One upstream daemon WebSocket serves multiple pane viewers; controller leases make concurrent input
  explicit, and slow viewers do not create another upstream or stall healthy viewers.
- The embedded phone-responsive client uses pinned Ghostty WASM with retained MIT notices, strict CSP,
  offline assets, push-driven status, additive protocol negotiation, targeted interrupt, and attributed
  semantic messaging.
- Release acceptance covers two real machines, a 390×844 Chrome viewport, hostile browser cases,
  restart/crash truthfulness, old/new compatibility, portable x86_64 ISA, and unchanged model/session
  counts.
- The Linux artifact uses CAS's selected ring TLS provider only. The unused AWS-LC provider (including
  its post-quantum-capable and AVX-512 runtime-dispatch code) is no longer linked, and the exact staged
  executable must pass the strict no-EVEX audit before upload. BLAKE3 keeps its SSE through AVX2
  accelerated paths; an audited 1.8.6 build override prevents its upstream runtime-only `no_avx512`
  switch from compiling the inactive AVX-512 archive into the portable artifact. Fingerprints remain
  byte-identical, while AVX-512-capable machines may see lower BLAKE3-heavy indexing throughput.

## Pre-post fill and verification

- [ ] Replace `v2.55.1` with the future immutable corrective version; do not reuse or move any tag.
- [ ] Link the private posting checklist to the exact tag peel, GitHub release, workflow, asset names,
      SHA-256 digests, and green final acceptance report.
- [ ] Confirm the public release contains the Commander source boundary and both machines identify the
      same downloaded version.
- [ ] Confirm the final acceptance verdict is green, not merely the source-only guards.
- [ ] Re-read both posts for zero task IDs, zero agent/factory narration, impact-first Was → Now prose,
      and exactly two top-level posts.
- [ ] Post User first, then Dev; record their Slack timestamps in the release receipt, not in this draft.
