# Slack draft — Commander polish pass (PR #436 → main, b92d7610)

Channel: #cas-internal (C0B44GUKDK2)
Order: user top-level → user reply (threaded) → dev top-level → dev reply (threaded)

=== MESSAGE 1 (user top-level) ===
**Live on production — User:** Commander is now genuinely usable one-handed on a phone — the message button is reachable, the keyboard stays open while you type, and worker panes are readable rows instead of empty black boxes.

=== MESSAGE 2 (user reply, thread of message 1) ===
Was → Now:
• The "Message supervisor" button sat below the bottom edge of the phone screen — you could not tap it. Now it's a thumb-sized two-button pill that's always on screen.
• Typing into a terminal on the phone closed the soft keyboard every few seconds. Now the keyboard stays open while the screen live-updates.
• Opening the machine drawer on a phone squeezed the terminal into a sliver a few pixels wide. Now the drawer opens over a clean single-column layout.
• Connection dots showed the same grey whether a machine was live, retrying, or failed. Now: green when live, clear warning colours when not.
• Worker panes on the phone were large empty black wells. Now they're compact rows — tap one to read it full-size, tap again to go back.
• Pairing details crushed a long hub URL into a ~100px column. Now everything in the pairing flow is readable.

=== MESSAGE 3 (dev top-level) ===
**Live on production — Dev:** hub-web polish pass fixing six measured phone/desktop defects — control reachability, input-focus retention across renders, layout collapse under the drawer, and dead connection-state CSS (PR #436).

=== MESSAGE 4 (dev reply, thread of message 3) ===
Was → Now:
• `#mobile-message-toggle` measured at y=844 in an 844px viewport (three controls in a two-cell grid) → 96px two-cell pill, 48px targets, rail reserves the width.
• `renderSessionState` re-appended every pane card each heartbeat, blurring the terminal textarea every ~5s → panes re-append only on real position changes; focus measured surviving 12s of live heartbeats.
• `.shell.drawer-open` outranked the compact layout: grid-template-columns computed "328px 14px 48px" at 390px and resized the PTY on every toggle → single-column at phone width.
• CSS styled connection classes the app never emits (dot permanently idle grey) → lifecycle phases drive the colour.
• Secondary phone panes were 240px canvasless wells → 40px rows with reversible tap-to-promote.
• Proof: 134/134 hub-web tests (+3 new invariants), tsc clean, dist rebuilt once and byte-identical from committed src; bundle gzip +0.45 kB total against strictly reduced per-render DOM work. (PR #436)

## POSTED

- UTC: 2026-08-16T04:53:49Z
- Channel: #cas-internal (C0B44GUKDK2)
- Message 1 (top-level): https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786856004082539
- Message 2 (reply): https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786856012143259?thread_ts=1786856004.082539&cid=C0B44GUKDK2
- Message 3 (top-level): https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786856016244629
- Message 4 (reply): https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786856024531659?thread_ts=1786856016.244629&cid=C0B44GUKDK2
