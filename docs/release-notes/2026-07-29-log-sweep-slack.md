# Slack drafts — v2.36.0 (2026-07-29)

Channel: #cas-internal (C0B44GUKDK2)
Two distinct TOP-LEVEL posts. Not threaded. Post after the tag is pushed.

STATUS: POSTING APPROVED by operator 2026-07-29 (standing authorization for this wave).

---

## POST 1 — User perspective (top-level)

**Was:** a long-running session wrote all of today's logs into yesterday's file, punctuation-heavy notes silently skipped duplicate detection, and a finished group of work could be blocked — or waved through — by bookkeeping that no longer matched reality.
**Now:** logs land where you'd look for them, duplicate detection works on real-world text, and completion checks judge the actual work.

- Logs now roll over at midnight even for a session that has been up for days, and log cleanup can never delete the file currently being written. Previously an all-day session put everything under yesterday's date, which also made "what happened today" searches come up empty.
- Saving a note full of technical punctuation (paths, version numbers, colons) no longer silently skips the duplicate check — those are exactly the notes most likely to be duplicated.
- Closing out a group of work no longer demands a commit reference that stopped existing after a routine cleanup — the check now looks at the real branches, explains any discrepancy it finds, and still refuses when work is genuinely unmerged. The recovery command also works after the workspace it referenced was cleaned up.
- Completion bookkeeping can no longer be fooled by a recycled workspace name or a reset branch into declaring lost work "done" — it now requires evidence tied to the actual work.
- A busy moment in the database no longer prints scary errors for a routine self-healing retry — and that retry can no longer stall the whole coordination loop for half a minute.

---

## POST 2 — Dev perspective (top-level)

**Was:** the daily log filename was computed once at startup; dedup queries hit the parser raw; the epic gate keyed on recorded SHAs that cleanup could invalidate; reminder expiry retried synchronously on the tick loop.
**Now:** date-aware rotation with a lock-free fast path, parse-failure-tolerant dedup, evidence-based gate reconciliation, and budget-bounded retries.

- **Log rotation:** a date-aware MakeWriter rotates a live daemon at local-midnight on write/flush; steady-state writes take no mutex and no timezone lookup (atomic day marker); a failed rotation open falls back to the old handle instead of dropping lines; cleanup keys on mtime, protects the active path even at retention 0, and survives concurrent unlinks.
- **Dedup resilience:** memory-overlap queries that fail Tantivy's QueryParser (the engine turned out to be Tantivy, not FTS5 as reported — same mechanism) retry with individually quoted/escaped literal terms; clean queries take the original path byte-for-byte; both production failure shapes are pinned as regressions.
- **Epic-gate reconciliation:** a recorded anchor that no longer matches any live ref is reconciled against live local+origin branch ancestry with the divergence surfaced in a decision note — but supersession now requires task-specific proof (anchor reachable from the live tip, or patch cherry-equivalent on the parent), so recycled branch names and hard-resets cannot clear genuinely stranded work. The branch-based merge remediation works after worktree cleanup.
- **Reminder expiry:** the busy-retry moved off the unbounded shared schedule to a 100ms budget on the tick path (measured worst case before: ~32s tick stall); local contention defers to the next tick, external busy logs WARN, terminal failures keep ERROR.
