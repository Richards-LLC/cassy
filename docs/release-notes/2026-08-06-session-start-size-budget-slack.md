# Release notes — SessionStart size budget (main, cbb0f403)

**Channel:** `#cas-internal` (`C0B44GUKDK2`)
**Status:** POSTED 2026-08-06 20:52:00 EDT to `#cas-internal` (`C0B44GUKDK2`).
- User top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063920575319
- User reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063925920379
- Dev top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063929037139
- Dev reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063935118609

Post order: user top-level → capture `ts` → user reply → dev top-level → capture `ts` → dev reply.

=== MESSAGE 1 — User thread, top-level ===
Live on production — **User** — Sessions no longer start blind: the startup briefing now always fits on screen and always arrives.

=== MESSAGE 2 — User thread, reply (to MESSAGE 1 ts) ===
**Was →** on projects with a lot going on (stale codemap, leftover uncommitted files, orphaned processes), the session-start briefing could grow past the chat window's size limit. It got shunted to a file, you saw a one-line stub, and the assistant itself only received the first couple of KB — so a fresh session looked like it loaded nothing and sat there without its instructions.
**Now →** the briefing keeps itself under the size limit. When there's too much to show, the bulky sections shrink to a count plus the exact command that brings the full detail back — and the core guidance is never the part that gets cut. What you see at session start is now the whole signal, every time.

=== MESSAGE 3 — Dev thread, top-level ===
Live on production — **Dev** — SessionStart `additionalContext` is now assembled under an aggregate 9KB byte budget with deterministic per-section degradation instead of unbounded string concatenation.

=== MESSAGE 4 — Dev thread, reply (to MESSAGE 3 ts) ===
**Was →** `handle_session_start` concatenated the base context (role guidance, ready tasks, memories) with the codemap-freshness, WIP-triage, orphan/GC, and issue-triage banners with no total cap. Only the guidance component had a size test; the assembled payload could pass ~10KB, at which point the harness persists it to a tool-results file and inlines a 2KB preview — silently starving the session of the rest.
**Now →** a `SessionContextAssembler` (new `session_budget` module) renders the payload under a 9,216-byte budget. Guidance, the context header, and safety banners are protected verbatim; every variable-length banner registers a pre-authored compact form (counts + remediation command) and degrades largest-saver-first, deterministically, whole-segment only — never mid-line truncation. A worst-case regression test drives the real renderers (198-change codemap, 240-file WIP tree, full issue list) and asserts the bound. Real-world supervisor payload: 11.9KB → 9.1KB.
