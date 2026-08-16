# Slack draft — Commander UX pass (PR #437 → main, 7fac2f32)

Channel: #cas-internal (C0B44GUKDK2)
Order: user top-level → user reply (threaded) → dev top-level → dev reply (threaded)

=== MESSAGE 1 (user top-level) ===
**Live on production — User:** You can now actually finish typing a message to your supervisor on the phone — drafts survive the live refresh, sends confirm themselves, and stale data admits it's stale.

=== MESSAGE 2 (user reply, thread of message 1) ===
Was → Now:
• The message you were typing was wiped every few seconds by the live refresh — a 41-character draft measured gone after 8 seconds. Now your draft, cursor, and focus all survive.
• Sending gave no feedback and left the text sitting there, so success looked exactly like failure. Now it clears, confirms, and refuses an empty send with a reason.
• Confirmations could be deleted by the next refresh moments after appearing. Now they stay until you've seen them.
• Greyed-out controls (Take control, Interrupt) only explained themselves on mouse hover — nothing on touch. Now they tell you why on tap.
• One outage produced six-plus identical alert cards. Now they group into one card with a ×N count.
• Workers & tasks kept rendering as if live during an outage. Now they're labelled stale, aged from the last live moment, and dimmed.
• First run was a dead end pointing at a button on the wrong machine. Now an unpaired Commander offers "Pair a machine", the pairing code has a copy button, and the next step is written out.

=== MESSAGE 3 (dev top-level) ===
**Live on production — Dev:** Heuristic UX pass over hub-web: heartbeat-render draft destruction fixed, toasts moved to a body-level overlay, attention fingerprint coalescing, stale-state labelling, and first-run/pairing affordances (PR #437).

=== MESSAGE 4 (dev reply, thread of message 3) ===
Was → Now:
• Composer state lived inside the re-rendered shell: every ~5s heartbeat destroyed the draft and dropped focus to <body> → draft, caret, and focus preserved across renders, verified over 9s of live heartbeats at 390px and 1440px.
• Toasts were children of the re-rendered tree → body-level overlay outside the render root.
• Attention cards had no identity, so repeated events stacked duplicates → stable fingerprints coalesce with ×N counts.
• No liveness signal on Workers & Tasks → stale sections labelled and aged from last live data; lease loss announced with the stale controller identity cleared.
• Disabled controls relied on title/sr-only → spoken reason on tap; empty send refused with a reason; pairing success confirmed; Copy command on the pairing code.
• Proof: 140/140 hub-web tests (7 new invariants), tsc clean, dist rebuilt once, byte-identical on clean rebuild. Bundle +3,340 B raw (+0.17% of payload), declared and accepted — no new request, dependency, font, or startup step. Full heuristic evaluation with 20 screenshots in the task artifacts. (PR #437)

## POSTED

- UTC: 2026-08-16T12:29:07Z
- Channel: #cas-internal (C0B44GUKDK2)
- Message 1 (user top-level): https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786883324402059
- Message 2 (user reply): https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786883331739589?thread_ts=1786883324.402059&cid=C0B44GUKDK2
- Message 3 (dev top-level): https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786883335834349
- Message 4 (dev reply): https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786883343413109?thread_ts=1786883335.834349&cid=C0B44GUKDK2
