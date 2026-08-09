# Commander v1 runtime release — v2.55.0 unposted Slack drafts

> **DO NOT POST.** `v2.55.0` is the policy-derived release candidate, not a published Commander
> version. The H7 verdict remains **NOT YET RELEASABLE**. Release ownership must first merge the
> reviewed commit to `main`, publish a new immutable `v2.55.0` tag whose peeled commit contains
> `bfa1e0af`, record both asset digests, install the same public Linux asset on two real machines, and
> receive a green assembled acceptance report. Re-review this file only after all of those gates pass.

Destination after the gate passes: `#cas-internal` (`C0B44GUKDK2`). These are the two distinct
top-level runtime-release posts required by `docs/RELEASE_SLACK_RUBRIC.md`; they are not threaded
replies. Status: **unposted**.

## User-perspective top-level post

**Live on production · User · v2.55.0**

Was: checking work across machines meant opening each terminal separately. → Now: Commander gives you
one phone-friendly view of your paired CAS machines, with live panes and deliberate, secure control.

- Pair a machine once, then see its sessions and terminal output directly over your private network.
- Watch the same session from more than one screen while one clearly identified controller holds input.
- Reconnect after hub or daemon restarts without hiding what stopped or inventing a recovery state.
- Revoke a device when needed; expired, replayed, cross-site, or over-scoped access is refused.
- Commander observes existing work without creating additional sessions or model requests.

## Dev-perspective top-level post

**Live on production · Dev · v2.55.0**

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

## Pre-post fill and verification

- [ ] Confirm the immutable published tag is exactly `v2.55.0`; do not reuse or move an earlier tag.
- [ ] Link the private posting checklist to the exact tag peel, GitHub release, workflow, asset names,
      SHA-256 digests, and green final acceptance report.
- [ ] Confirm the public release contains the Commander source boundary and both machines identify the
      same downloaded version.
- [ ] Confirm the final acceptance verdict is green, not merely the source-only guards.
- [ ] Re-read both posts for zero task IDs, zero agent/factory narration, impact-first Was → Now prose,
      and exactly two top-level posts.
- [ ] Post User first, then Dev; record their Slack timestamps in the release receipt, not in this draft.
