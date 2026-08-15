# Slack — Commander UI modernization, 2026-08-15

Channel: #cas-internal (C0B44GUKDK2)
Deploy target: **Live on production** (merged to `main` as `b6da4319`, PR #406)

Four messages, in this order: user top-level → user reply → dev top-level (NEW
top-level, not a reply) → dev reply.

## User thread

**Top-level:**

Live on production — **User** — Commander now shows you what your sessions are actually doing, and terminals open instead of sitting on "Connecting…".

**Reply:**

Was → Now:

• Was: terminals came up as mangled, mid-word-wrapped text, sized to something other than the pane you were looking at. Now: they render at the size of the pane on your screen, and text stays where it belongs.

• Was: opening a session could pull about 44.7 MB before a single character appeared — slow on a laptop, often fatal on a phone. Now: roughly 17 KB gets your main pane on screen, and history is fetched only when you scroll back for it.

• Was: a connection that was failing sat on "Connecting to terminal…" indefinitely, with nothing to tell you whether to wait or give up. Now: it names the step it is stuck on and how long it has been trying, and offers Retry and Diagnose.

• Was: every alert looked identical, so a dead session was indistinguishable from a routine note. Now: alerts are grouped by session and ranked critical / warning / info, repeated failures collapse into a single card with a count, and you can dismiss a whole group at once.

• Was: the pane you actually watch got exactly as much room as everything else. Now: it is the dominant pane by default, panes can be promoted and reordered, and your layout is remembered next time.

• Was: pairing from the browser could only ever grant read-only access, so "Take control" stayed greyed out with no explanation. Now: control can be requested when you need it, and anything still unavailable tells you why.

## Dev thread

**Top-level:**

Live on production — **Dev** — Attach now sends an authoritative ANSI keyframe built from current terminal state instead of replaying a sliced byte tail, and pane geometry is no longer gated behind input authority.

**Reply:**

Was → Now:

• Was: attach replayed a bounded raw tail of each pane's ring buffer. A byte slice is not a valid terminal state — it can begin mid-escape or omit mode-setting, so panes garbled intermittently. Now: attach emits a pane keyframe generated from current terminal state under the same lock as tap registration, then streams live binary frames, with scrollback paged on demand. Measured on a real session: 44,718,217 B → 17,087 B before the main pane is ready (546 B metadata + 16,541 B keyframe). Proven against a deliberately constructed worst case whose final 256 KiB began mid-SGR with no clear or home marker; the resulting keyframe still opened with RIS and re-established reset, clear and home.

• Was: one global size cap, which only prevented transport failure and would let a payload near 500 KiB regress by ~100x undetected. Now: layered ceilings — 32/64 KiB metadata, 128/256 KiB per keyframe, 256/512 KiB total before the main pane is ready — so a regression names the stage that bloated.

• Was: `ResizePane` required pane-input scope plus an active lease, so a read-only viewer could attach but never report its viewport, and a wide PTY rendered into a mount a fraction of its width. Now: reporting the viewport carries pane-read scope, with lease policy enforced separately — an unleased pane follows its observer, a leased pane follows its controller, so an observer cannot reflow a controller's terminal.

• Was: connection state was a boolean with two independent timers. Now: an explicit staged lifecycle (resolving → dialing → auth → attaching → live) with per-stage deadlines, jittered backoff, heartbeat latency, authenticated Diagnose, and expired-vs-revoked token handling surfaced distinctly.

• Was: stopping a registered server reported success while a wrapped child process survived it. Now: teardown is verified rather than assumed.

• Was: nothing summarized a session or an alert for you. Now: optional AI enrichment can label session cards and attention events, shipping default off, with redaction applied inside the provider so callers cannot bypass it, and a deterministic severity floor that enrichment may raise but never lower.

## POSTED

Published to **#cas-internal** (`C0B44GUKDK2`) on **2026-08-15**.
Structure verified by a separate channel read: two top-level messages, each with
exactly one threaded reply attached to the correct parent.

| # | Message | Type | UTC | Permalink |
|---|---------|------|-----|-----------|
| 1 | User top-level | top-level | 2026-08-15 12:43:39Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786797819883899 |
| 2 | User detail | reply to 1 | 2026-08-15 12:43:48Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786797828984089?thread_ts=1786797819.883899&cid=C0B44GUKDK2 |
| 3 | Dev top-level | top-level | 2026-08-15 12:43:52Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786797832748759 |
| 4 | Dev detail | reply to 3 | 2026-08-15 12:44:04Z | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786797844758099?thread_ts=1786797832.748759&cid=C0B44GUKDK2 |

Announced as a `main` landing (`b6da4319`, PR #406), not a tagged runtime
release. The next version tag should not re-announce this content.
