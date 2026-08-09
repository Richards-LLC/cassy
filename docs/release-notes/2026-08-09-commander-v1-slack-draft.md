# Commander v1 runtime release — v2.55.5 candidate, unposted Slack drafts

> **DO NOT POST.** The immutable public `v2.55.4` artifact is authentic and identically installed on
> two real machines, but its assembled H7 verdict is **NOT RELEASABLE**. In two fresh Chrome 151
> phone-class attempts, local controller pairing succeeded and the authorized machine-B preflight
> returned the exact controller origin, but the subsequent cross-origin pairing exchange failed in
> Chrome with `MissingAllowOriginHeader`. Direct multi-machine pairing never completed, so the gate
> stopped before later acceptance rows. All published tags, releases, and assets remain unchanged.
> Source candidate `v2.55.5` contains the reviewed controller-origin CORS correction; it is not
> tagged, published, installed, or accepted yet. **H7 remains NOT YET RELEASABLE. Do not post these
> drafts.** Re-review only after a new immutable corrective release is independently verified and
> passes the complete two-machine H7 matrix from its exact public bytes. Slack remains unposted until
> the user elects to post after a green gate.

Destination after the gate passes: `#cas-internal` (`C0B44GUKDK2`). These are the two distinct
top-level runtime-release posts required by `docs/RELEASE_SLACK_RUBRIC.md`; they are not threaded
replies. Status: **unposted**.

## User-perspective top-level post

**Live on production · User · [PUBLISHED VERSION]**

Was: checking work across machines meant opening each terminal separately. → Now: Commander gives you
one phone-friendly view of your paired CAS machines, with live panes and deliberate, secure control.

- Pair a machine once, then see its sessions and terminal output directly over your private network.
- Once you use Commander's private HTTPS address, your browser remembers to stay on HTTPS for future
  visits instead of accepting a plaintext downgrade.
- Start on a newly installed machine without manually creating Commander state directories first.
- Watch the same session from more than one screen while one clearly identified controller holds input.
- Restart a Commander hub without racing its still-exiting predecessor or launching a competing owner.
- Reconnect after hub or daemon restarts with an evidence-backed explanation when the operating system
  reports how a daemon stopped, while missing or stale evidence stays honestly unknown.
- Revoke a device when needed; expired, replayed, cross-site, or over-scoped access is refused.
- Commander observes existing work without creating additional sessions or model requests.

## Dev-perspective top-level post

**Live on production · Dev · [PUBLISHED VERSION]**

Was: CAS exposed machine-local daemon state without a browser-safe fleet boundary. → Now: each machine
runs a loopback Commander hub with tailnet TLS, exact-origin proof-of-possession auth, one upstream per
daemon session, and bounded downstream fan-out.

- Controller-origin IndexedDB stores a non-extractable P-256 key and origin-bound device credential;
  pairing capabilities and WebSocket tickets are short-lived and single-use.
- Exact Origin/CORS handling, DPoP method/URI/credential binding, replay caches, per-operation scopes,
  revocation, and attributed audit fail closed.
- Cross-origin pairing exchanges carry the exact authorized controller-origin CORS grant on both
  successful and bound generic denied responses, so controller-origin Chrome can pair each target
  directly while unrelated refused requests stay fail-closed.
- The verified Tailscale Serve TLS path uses a separate server-owned loopback backend and emits exactly
  `Strict-Transport-Security: max-age=31536000` across success, auth-error, catch-all, and preflight
  responses; the documented plaintext listener ignores spoofed proxy and identity headers.
- One upstream daemon WebSocket serves multiple pane viewers; controller leases make concurrent input
  explicit, and slow viewers do not create another upstream or stall healthy viewers.
- Hub restart propagates stop failures, waits for both the old PID and authoritative machine-lock
  release, and acquires the lock before stale cleanup or replacement launch. A bounded timeout fails
  without starting a competing hub, and concurrent start/restart attempts retain exactly one owner.
- Clean-home startup creates the missing hub hierarchy owner-only for ordinary and Tailscale Serve
  flows, preserves safe existing state, and fails closed on symlink, ownership, mode, or ancestor
  collisions without leaking filesystem paths.
- Daemon-exit receipts are bound to session, PID, and process-start fingerprint. The connector consumes
  only exact evidence after disconnect, rejects stale epochs, and surfaces real `SIGILL` remediation
  without turning absent or malformed evidence into a diagnosis.
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

- [ ] Replace `[PUBLISHED VERSION]` only after one new immutable corrective tag exists; do not reuse or
      move `v2.55.4` or any earlier tag.
- [ ] Link the private posting checklist to the exact tag peel, GitHub release, workflow, asset names,
      SHA-256 digests, and green final acceptance report.
- [ ] Confirm the public release contains the Commander source boundary and both machines identify the
      same downloaded version.
- [ ] Confirm controller-origin Chrome completes direct pairing to machine B and that successful and
      generic denied exchange responses expose only the exact authorized origin.
- [ ] Confirm the documented paired-client Tailscale Serve restart reaches ready, preserves the stable
      machine identity and URL, reconnects clients, and does not restore a lease automatically.
- [ ] Confirm the final acceptance verdict is green, not merely the source-only guards.
- [ ] Re-read both posts for zero task IDs, zero agent/factory narration, impact-first Was → Now prose,
      and exactly two top-level posts.
- [ ] Post User first, then Dev; record their Slack timestamps in the release receipt, not in this draft.
