# 2026-07-31 — Compounding reliability release — #cas-internal posts

## Post 1 — User

**Live on production — User**

Your factory stopped taking anyone's word for anything. It used to be possible for finished work to look delivered when it wasn't, for status displays to show stale information, or for a build cache to quietly eat the whole disk overnight — now every hand-off is checked against exactly what was reviewed, status is either current or clearly marked unavailable, and disk space guards itself.

- Was: a merge could land on a branch that had moved since review, and everything still reported success. Now: the merge target is verified against the exact reviewed state; if anything moved, the merge steps back cleanly and tells you, with nobody's work destroyed.
- Was: server status panels could show a mix of old and new information after a config change or crash. Now: status updates land all-or-nothing, and anything stale is labeled unavailable instead of pretending to be fine.
- Was: build caches grew without limit — one overnight run filled the entire disk and took the session down with it. Now: caches have a size watermark and clean themselves up safely, never touching source files or anything still in use.
- Was: the server names you saw on screen couldn't always be used to manage those servers. Now: what's displayed is what works, and sensitive raw names stay private.

## Post 2 — Dev

**Live on production — Dev**

Trust boundaries around verification, delivery, and close are now enforced end-to-end: authority is server-derived, receipts bind to exact reviewed state, and every multi-step transition is atomic or fails closed.

- Was: any client could register itself into supervisor authority and mint verification verdicts. Now: public registration can only create worker-tier identities; supervisor-direct authority requires server-internal provenance, and re-registration preserves the durable role.
- Was: delivery merges compared the target tip at preflight only. Now: a canonical repo+ref advisory lock plus compare-and-swap holds through the merge, first-parent topology is verified against the reviewed SHA, and drift rolls back via `update-ref` CAS with a typed recoverable state.
- Was: completion receipts could be replayed or replaced across proof cycles, and lease-release failures still projected success. Now: replacement receipts reject against any active proof, terminal replay after reopen is a strict no-op, and incomplete handoffs return a typed retryable state.
- Was: verification dispatch creation raced across processes, and timed-out dispatches could return the wrong row. Now: creation is transaction-serialized and idempotent, and the timeout path returns exactly the row it marked.
- Was: verifier-authored evidence could carry embedded absolute paths and separator-obfuscated secrets into durable storage. Now: the sanitizer catches embedded/Markdown/file-URL/Windows/UNC paths and whitespace-obfuscated auth material without echoing rejected content.
- Was: proxy catalog and health snapshots published as sequential renames — crash between writes exposed mixed generations. Now: one fingerprinted generation-atomic envelope backs catalog, health, SessionStart, and preflight, with explicit empty/unavailable states.
- Was: worker build caches were unbounded. Now: watermark-gated GC with live-process/recent-write/retention exclusions, symlink containment, atomic quarantine, and machine-readable dry-run.
- Direct update-to-closed now resolves the post-mutation work target and enforces the same verification-type/merge/delivery gates as normal close; task reopen is failure-atomic in one SQLite transaction including its lifecycle outbox.
