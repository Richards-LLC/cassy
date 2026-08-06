# Slack draft — 2026-08-06 v2.43.0 (supervisor push-signals, messaging honesty, sync safety, status truth, review-gate integrity)

Channel: #cas-internal (C0B44GUKDK2). Two top-level posts per runtime rubric.

## Post 1 — User

Live on main — **User**

The status page stopped crying wolf and started telling you who's actually waiting on you — and finished work now announces itself instead of parking silently until someone thinks to look.

- **Was → Now:** a finished task whose close needed a merge just vanished — everyone idled until a human checked in → the event now reaches the coordinator as a push signal the moment it happens.
- **Was → Now:** the STALLED warning fired constantly at perfectly healthy sessions (and the two real problems were exactly what it couldn't see) → it's replaced by an evidence-based "not waking" check, workers waiting on a merge are labeled "WAITING ON YOU" with the next action, and every row shows how much unread mail a worker actually has.
- **Was → Now:** work lists silently showed only the first 10 items, newest first — so hours went by with urgent items buried under fresh low-priority ones → lists sort by priority, print the true total, and say exactly how many rows were held back.
- **Was → Now:** a fleet-wide sync could rebase someone mid-work and strand their uncommitted changes with no warning → sync refuses dirty or busy checkouts unless forced, and any stash problem notifies both sides with the recovery reference.
- **Was → Now:** after your work was merged, the system said you were "behind" — behind your own landed commits — and blocked your next assignment on it → behindness is measured by content, so your own merged work never counts against you.
- **Was → Now:** with the review service degraded, an empty review could look identical to a clean one and pass the quality gate → a review that didn't actually run is now rejected as a failure, loudly.

## Post 2 — Dev

Live on main — **Dev**

Close-rejection events push to the supervisor; wake nudges distrust the registry and read the pane; behindness is tree-equality + cherry-pick instead of rev-list counting; the review close gate requires every mandatory persona lane; the workflow parity guard finally runs where CI can see it.

- **Was → Now:** MERGE REQUIRED close rejections died in worker_activity → delivered to the supervisor within the daemon tick as urgent inbox/injected-turn signals.
- **Was → Now:** the idle-nudge trusted registry "activity" (an automated git checkpoint inside a 120s window counted as busy) and acks were inferred from replies → pane/transcript evidence gates wakes (sustained silence, clean composer, no outstanding tool call — the tool-call veto prevents answering a worker's open approval dialog), vetoed nudges retry, and confirmation_source distinguishes explicit_ack from inferred_from_reply.
- **Was → Now:** sync_all_workers rebased dirty mid-task worktrees; failed stash pops stranded WIP silently → dirty/in-progress worktrees refuse without force; stash-pop failures notify worker + supervisor with the stash ref (naming commands git will accept for a SHA: apply, not pop).
- **Was → Now:** worker_status counted any live lease as in-progress work (leases outlive close by up to 30 min) and STALLED assumed continuous execution → task state re-read at render, NOT-WAKING keyed on unconsumed mail past threshold, unread-inbox depth keyed on worker-seen rather than daemon-delivered.
- **Was → Now:** ready/blocked/available capped output silently (default sort newest-first buried older P0s); sort params on available were advertised but inert → priority-first defaults, honest totals, withheld-row footers, and honoured (or explicitly rejected) sort params across the family.
- **Was → Now:** `rev-list --count` behindness counted the worker's own landed merge, deadlocking follow-on assignment against the merge that would cure it → tree equality first, then `rev-list --no-merges --cherry-pick --right-only`; squash-merged lanes covered (patch-id matching is many-to-one blind); the sanctioned assignee-match merge-authorization path gained its first tests.
- **Was → Now:** review envelopes with personas_run=0 — or 1, since only `security` is Claude-hosted and the four always-on lanes ride the Codex transport — passed the close gate as clean → any missing mandatory lane rejects, computed by set difference against orchestrator dispatch, not self-reported skips. Stated limit: accident detector for honest producers, not a forgery defence.
- **Was → Now:** the rendered workflow copy drifted from the shipped builtin for a week with the byte-parity test red in a Node suite nothing ran → guard duplicated into `cargo test`, names the first divergent line and repair direction; CODEX_PERSONA_EFFORT deliberately restored to 'high' (provenance verified via `git log -G`; `-S` is blind to value edits).
- Orphaned `rustc`/`rustdoc` with no parent to report to are reapable in gc_report/gc_cleanup (`cargo` deliberately excluded — an adopted cargo is routinely a live build); build-jobs derate tracks real fleet size; the original issue's OOM premise was measured false and corrected on the record (`oom_kill 0`; the SIGKILLs were self-inflicted).
- Epic stacking (C on B on A) surfaces the full unlanded ancestry at creation and in epic_status, derived from git topology (merge-base --is-ancestor), not persisted bookkeeping.
- Lands GitHub issues #101–#112 via `Fixes` trailers on the merge.
