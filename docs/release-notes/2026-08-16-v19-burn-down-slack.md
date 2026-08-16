# Slack draft — issue burn-down v19 batch (PR #438 → main, bf75cce2)

Channel: #cas-internal (C0B44GUKDK2)
Order: user top-level → user reply (threaded) → dev top-level → dev reply (threaded)

=== MESSAGE 1 (user top-level) ===
**Live on production — User:** Eleven long-standing reliability complaints fixed in one landing — honest message delivery status, workers that always start on fresh code, memory search filters that actually filter, and epics that close cleanly when the work shipped.

=== MESSAGE 2 (user reply, thread of message 1) ===
Was → Now:
• A message to a busy worker could show "confirmed" while the worker provably never read it. Now unread-but-active shows as its own weaker state, and a message that can't get through gets escalated instead of silently waiting.
• Workers could start on weeks-old code and waste their first hour rediscovering fixes. Now a stale starting branch is refreshed automatically before the worker is created.
• Searching memories by tag silently returned everything. Now tag, tier, and scope filters actually narrow results, and the count matches what you see.
• Machine-to-machine chatter was being saved as durable memory, drowning out real instructions. Now only genuine operator instructions are captured.
• Finished projects could refuse to close with false "unmerged work" warnings. Now shipped-then-improved work is recognized as shipped.
• Ending a session could crash the server; asking for an isolated workspace got a refusal with broken instructions; and following the standard upgrade steps could cut off your own tools. All three fixed.

=== MESSAGE 3 (dev top-level) ===
**Live on production — Dev:** v19 burn-down batch: truthful message_status staging with wake-decline escalation, WorkTarget-precedence branch resolution end to end, epic-close content reconciliation for squash-then-evolved deliveries, provenance-envelope prompt capture, and async-safe session_end (PR #438).

=== MESSAGE 4 (dev reply, thread of message 3) ===
Was → Now:
• inferred_from_reply granted top-line confirmed on unrelated activity → assumed_seen as a distinct stage; three durable wake-gate declines flag undelivered_after_wake_declines; explicit exact-message ack is what discharges an urgent halt.
• Spawn base derived an epic branch from the epic title slug, worktree_merge read a legacy branch field, and children defaulted to trunk → one WorkTarget precedence chain (task-WT > live epic branch > epic-WT > trunk) across spawn, merge, create-with-epic, and dep_add parent-link; stale-but-clean bases fast-forward and push before the worktree is cut.
• Epic close compared recorded anchors byte-for-byte and stored commit counts → live lane refs, squash re-anchoring, and the child-level hunk-survival checker, with unproven anchors retained fail-closed.
• Prompt capture parsed rendered text for provenance → delivery registers a typed one-shot envelope consumed by hook dispatch; operator capture restored, relay capture gone.
• session_end ran block_on inside the async dispatcher and panicked → synchronous hook work moved off-dispatcher.
• Plus: target_lock test reaps its sleeps (clean CI teardown), worktree_create tells the truth and prints valid TOML, supervisor-checklist mirrors sequence the binary rebuild safely, and a reconciliation pass fixed two real cross-lane regressions the scoped proofs missed. (PR #438)

## POSTED

- UTC timestamp: 2026-08-16T13:19:23Z
- Channel: #cas-internal (C0B44GUKDK2)
- Message 1: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786886336799609
- Message 2: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786886345369039?thread_ts=1786886336.799609&cid=C0B44GUKDK2
- Message 3: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786886349668379
- Message 4: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786886359313259?thread_ts=1786886349.668379&cid=C0B44GUKDK2
