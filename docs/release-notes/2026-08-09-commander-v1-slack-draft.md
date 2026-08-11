# Commander v1 runtime release — v2.62.0 RELEASABLE, DO NOT POST WITHOUT APPROVAL

> **DO NOT POST WITHOUT EXPLICIT USER APPROVAL.** Public `v2.62.0` passed the fresh assembled gate on
> soundwave and unicron with real Chrome 151 at `390×844`. The held-viewer restart that failed on
> `v2.61.1` now starts one replacement, preserves the public URL, reconnects viewers, and drops the old
> lease. Security, revocation, `SIGILL`, compatibility, no-multiplication, and zero-residue rows passed.
> The bodies below are ready for review but remain unposted.

Intended destination after a future green gate and explicit approval: `#cas-internal` (`C0B44GUKDK2`).
Status: **DO NOT POST WITHOUT EXPLICIT USER APPROVAL; currently unposted**. The final paired report is
green at exact public `v2.62.0`; historical `v2.61.1` failure evidence remains preserved.

## User-perspective top-level post

**Live on production · User · v2.62.0**

Was: checking work across machines meant opening each terminal separately. → Now: Commander gives you
one phone-friendly view of your paired CAS machines, with live panes and deliberate, secure control.

- Pair a machine once, then see its sessions and terminal output directly over your private network.
- Once you use Commander's private HTTPS address, your browser remembers to stay on HTTPS for future
  visits instead of accepting a plaintext downgrade.
- Start on a newly installed machine without manually creating Commander state directories first.
- Watch the same session from more than one screen while one clearly identified controller holds input.
- Restart a Commander hub while viewers are attached; they reconnect to one replacement and must
  deliberately reacquire control.
- Reconnect after hub or daemon restarts with an evidence-backed explanation when the operating system
  reports how a daemon stopped, while missing or stale evidence stays honestly unknown.
- Revoke a device when needed; expired, replayed, cross-site, or over-scoped access is refused.
- Commander observes existing work without creating additional sessions or model requests.

## Dev-perspective top-level post

**Live on production · Dev · v2.62.0**

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
- Hub restart gives live viewer connections a bounded drain window, force-closes only residual stale
  sessions, waits for the old PID and machine lock, then starts exactly one replacement. Reconnecting
  viewers do not regain a controller lease automatically.
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

- [x] Publish immutable `v2.62.0` with the held-viewer drain fix and complete a fresh full H7
      continuation with live viewers and zero residue.

- [x] Publish immutable `v2.61.0` containing app-bundle discovery corrective `cas-a13a`; exact public
      asset, tag peel, installed binary, and selected absolute app CLI are recorded.
- [x] On stock prowl with no wrapper or stray PATH entry, rerun only macOS hub restart plus the Serve
      spot check: the upgrade row failed on the legacy receipt shape and reached HTTPS `502`; the hub
      was restored only after an explicitly recorded manual reset of the exact CAS-owned stale mapping.
- [x] Publish immutable `v2.61.1` containing backward-compatible legacy-receipt parsing; do not move
      `v2.61.0` or any earlier tag.
- [x] Rerun the stock prowl upgrade restart and require zero manual reset, stable
      machine identity/URL, the absolute app-bundle CLI in the new receipt, and HTTPS health `200`.
- [x] Confirm both final post headers name `v2.61.1` only after the corrective row and paired report
      are green.
- [x] Link the private posting checklist to the exact tag peel, GitHub release, workflow, asset names,
      SHA-256 digests, and green final acceptance report.
- [x] Confirm public `v2.61.1` contains the focused corrections and runs on prowl; soundwave's exact
      public `v2.60.0` evidence carries forward and its operator installation remains untouched.
- [x] Confirm controller-origin Chrome completes direct pairing to machine B and that successful and
      generic denied exchange responses expose only the exact authorized origin.
- [x] Confirm the documented paired-client Tailscale Serve restart reaches ready without a wrapper,
      preserves the stable machine identity and URL, reconnects clients, and does not restore a lease automatically.
- [x] Confirm the final acceptance verdict is green, not merely the source-only guards. Public
      `v2.62.0` is **RELEASABLE**.
- [x] Re-read both posts for zero task IDs, zero agent/factory narration, impact-first Was → Now prose,
      and exactly two top-level posts.
- [x] Refresh both bodies to exact public `v2.62.0` after the green gate.
- [ ] Obtain explicit user approval to post.
- [ ] Only then post User first, then Dev; record Slack timestamps in the release receipt, not here.
