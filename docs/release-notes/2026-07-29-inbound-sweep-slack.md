# Slack drafts — v2.34.0 (2026-07-29)

Channel: #cas-internal (C0B44GUKDK2)
Two distinct TOP-LEVEL posts. Not threaded. Post after the tag is pushed.

STATUS: POSTING APPROVED by operator 2026-07-29 (pre-authorized in session).

---

## POST 1 — User perspective (top-level)

**Was:** typing a message could get hijacked mid-keystroke by an incoming update, stray dev servers quietly ate the machine's memory, and a merge conflict could freeze work with no way out.
**Now:** your keyboard is yours, helper processes die with their owner, and every stuck state has a recovery path.

- Typing in the control pane is no longer interrupted or stolen by incoming messages. Non-urgent messages wait until your draft is sent; only true emergencies break through.
- Background processes started during work (dev servers and the like) are now tracked and shut down when their session ends. Previously they could pile up unnoticed until the machine ran out of memory and swap.
- A merge that genuinely conflicts no longer freezes the work in a "waiting" state nobody can touch — it reopens for fixing, with the cause spelled out. And a failed merge no longer leaves the repository broken in a way that made every later merge fail with a misleading error.
- Message delivery now tells the truth: "delivered" means it actually reached the recipient, repeated identical messages are no longer silently swallowed, and nothing is re-sent to someone who already handled it.
- A worker sitting on assigned work it never started is now flagged after a few minutes instead of looking "maybe busy" indefinitely.

---

## POST 2 — Dev perspective (top-level)

**Was:** several delivery and lifecycle signals reported state that hadn't happened yet — delivered-before-write, dedup returning stale row ids, a "graceful" kill that was immediate, close gates keyed on superseded SHAs.
**Now:** outcomes are truthful, bounded, and recoverable.

- **Delivery integrity:** PTY injection returns a distinct deferred outcome while an operator draft is pending; queue rows stay durable-pending until a real pane write; broadcast accounting counts only actual writes; deferral is bounded (30s) with nothing retained in RAM across pane teardown or respawn. Arrow/navigation keys no longer count as draft text.
- **Messaging semantics:** consumed messages confirm their acks; delivered reports become terminal instead of re-injectable; dedup is scoped to recent unacked duplicates and returns a visible suppressed-duplicate outcome instead of impersonating a fresh enqueue; confirmation is scoped to the actual counterparty; the two per-send full-table scans got partial covering indexes.
- **Process reaping:** workers own a setsid process group recorded with a start-time fingerprint that is re-validated at kill time (Linux /proc and macOS proc_pidinfo; unverifiable identity fails closed). Graceful teardown gets a real bounded TERM grace before KILL. GC reports orphan groups and never signals a group whose owner is canonically alive.
- **Merge-flow exits:** a conflicted parked task reopens to in-progress with stale anchors invalidated; conflict-detection errors fail toward the exit instead of re-locking; a conflicting shared-checkout merge aborts cleanly, names the conflicting files, and pre-existing mid-merge residue is detected up front.
- **Close evidence:** merged-before-close work with no captured anchor can present a commit receipt — full SHA, ancestor of the parent branch, non-empty diff, attributed to the task's own time window — validated merge-aware and logged for audit. Fabricated or historical SHAs are rejected by name.
- **Policy defaults:** omitted spawn parameters resolve to the shipped routing policy's models, and the warning names the resolved spec as a policy fallback instead of routine hygiene advice.
