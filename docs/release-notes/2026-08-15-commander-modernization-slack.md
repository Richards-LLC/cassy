# Slack draft — Commander UI modernization, 2026-08-15

Channel: #cas-internal (C0B44GUKDK2)

**Status:** Draft. Merged to main as `b6da4319` (PR #406) and already live on the
operator's hub; not yet a tagged runtime release. Post after the operator has
reviewed, or fold into the next version announcement.

## User thread

**Top-level:**
Live on production — **User** — Commander now shows you what your sessions are doing instead of a wall of identical grey boxes, and terminals actually open.

**Reply (Was → Now):**
- Was: terminals rendered as mangled, mid-word-wrapped text, and the main pane often sat on "Connecting to terminal…" forever with no explanation. Now: terminals render at the size of the pane you are looking at, and a connection that is failing tells you which step it is stuck on, how long it has been trying, and offers Retry and Diagnose.
- Was: opening a session could transfer over 44 MB before anything appeared, which simply failed on some sessions and was brutal on a phone. Now: about 17 KB before the supervisor pane is ready — roughly 2,600 times smaller — with history fetched only when you scroll.
- Was: every alert looked the same, so a dead daemon was indistinguishable from a routine note, each one prefixed by two lines of machine and session name. Now: alerts are grouped by session, ranked critical / warning / info, repeated failures collapse into one card with a count, and you can dismiss a whole group at once.
- Was: the supervisor pane — the one you actually watch — got exactly as much space as everything else. Now: it is the dominant pane by default, panes can be promoted and reordered, and the layout is remembered.
- Was: pairing from the browser could only ever grant read-only access, so "Take control" was permanently greyed out with no explanation. Now: control can be requested at runtime, and any control that is unavailable says why and what to do about it.
- Also fixed: a hub that reported itself healthy after it had actually died, which is what caused Commander to serve errors and lose terminals entirely.

## Dev thread

**Top-level:**
Live on production — **Dev** — Commander attach is now an authoritative ANSI keyframe instead of a replayed byte tail, and PTY geometry is decoupled from input authority.

**Reply (Was → Now):**
- Was: `snapshot_to_ansi` replayed a bounded raw tail of the pane ring. That is not a valid terminal state — a slice can begin mid-escape or omit mode-setting, so panes garbled intermittently. Now: attach emits `PaneKeyframe` generated from current terminal state under the same pane lock as tap registration, then live binary frames; scrollback is paged on demand. Initial payload fell from 44,718,217 B to 17,087 B before supervisor-ready (546 B metadata Welcome + 16,541 B keyframe) against a 256 KiB budget. Layered ceilings replace one global cap: 32/64 KiB metadata, 128/256 KiB per keyframe, 256/512 KiB total-before-ready.
- Was: `ResizePane` was gated behind `pane-input` scope plus an active lease, so a read-only viewer could attach but never send its viewport size — a 281×65 PTY rendered into a ~51-column mount. Now: geometry is separate from input authority. With no lease any `pane-read` viewer may size; with a lease the holder owns geometry, so an observer cannot reflow a controller's terminal.
- Was: connection state was a boolean with two independent 10s timers, and pairing scopes could only narrow. Now: an explicit staged lifecycle (resolving → dialing → auth → attaching → live) with per-stage deadlines, per-session `AttachSnapshot`, jittered backoff, heartbeat latency, authenticated Diagnose, runtime scope escalation against a grant ceiling without re-pairing, and expired-vs-revoked token handling. Transport is proto-2 multiplexed per machine with raw binary PTY frames.
- Was: `server_stop` returned success while a PTY-wrapped child survived, and `shared` servers died with their worker. One root cause: the shared cgroup was nested beneath `cas-worker-*` instead of beside it, and the wrapper shared the worker PGID so a PID-only kill lost the reparented child. Now: shared scopes are siblings, private scopes children, launcher moves before fork, and teardown is verified rather than assumed.
- Optional AI enrichment for session cards and attention events ships **default off**, with redaction applied inside the provider so callers cannot bypass it.

## Posting sequence

1. Confirm with the operator whether this posts standalone or folds into the next tagged release.
2. Post the User top-level, then its single reply.
3. Post the Dev top-level, then its single reply.
4. Append the receipt below with UTC timestamps and all four permalinks.

## POSTED

_Intentionally empty until publication._
