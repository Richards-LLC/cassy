# Slack draft — 2026-08-05 factory reliability burn-down (close-path, spawning, messaging, containment)

Channel: #cas-internal (C0B44GUKDK2). Two top-level posts per runtime rubric.

## Post 1 — User

Live on main — **User**

The factory stopped fighting you. Finished work actually closes, requested workers actually start, and status messages stop lying about what already happened.

- **Was → Now:** finished tasks could be impossible to close — work delivered in a second repo, zero-diff investigations, or a review asking for amendments would bounce between contradictory refusals with no exit → every one of those paths now closes cleanly or tells you the one real step remaining.
- **Was → Now:** asking for workers sometimes created their folders but never launched anyone, or booted workers idle with no idea what to do → launches are validated up front, arrive with their task attached, and the confirmation reflects whether they're actually alive.
- **Was → Now:** stale alerts — "merge required" after the merge already landed, months-old requests delivered to brand-new workers, the same message arriving twice → signals are built from fresh state and delivered once.
- **Was → Now:** closing out a big branch could silently flip your main checkout onto that branch, so your next commits landed in the wrong place → your checkout stays where you left it.
- **Was → Now:** dev servers and other processes started by workers lingered after the worker was gone, squatting on ports and VRAM → teardown now takes the whole process tree with it, servers you *want* to keep running can be registered to survive on purpose, and a cleanup pass finds anything orphaned.
- New skill: a design-spec skill that generates and maintains a DESIGN.md, plus a release-notes rubric every project can inherit.

## Post 2 — Dev

Live on main — **Dev**

Close-path guards were rebuilt around task-attributed state instead of branch ancestry guesses; the spawn daemon's queue actually drains; messaging is idempotent; worker teardown got real containment tiers with a registry escape hatch.

- **Was → Now:** the close guard bound to the spawn-repo factory anchor and the local staging ref → it scopes to task-attributed commits, honors `target_repo`/`target_branch`, measures against fetched remote refs, accepts abbreviated commit receipts with a clear full-SHA resolution, and `awaiting_merge` gained a sanctioned amendment-required path (`request_changes`) instead of the old proof-lock dead end.
- **Was → Now:** the additive-only gate counted same-task WIP as foreign changes and zero-diff spike closes hit a two-stage review trap → gates scope to the delivering task, and spikes close on their search manifest.
- **Was → Now:** the spawn daemon enqueued requests but the consumer died after `shutdown_workers count=0`; invalid cli/model combos silently defaulted; `task_id` pre-assignment never fired → queue consumer survives, combos are validated at the door, pre-assignment lands, and spawn receipts include liveness.
- **Was → Now:** worker inboxes re-delivered drained messages on the idle-nudge path and coordination signals were built from stale snapshots → per-recipient ack rows with a seen-row lifecycle, and signals recomputed at send time.
- **Was → Now:** epic-close machinery could move the main checkout's HEAD onto the epic branch → merges happen without touching any live checkout.
- Worker teardown kills the spawned process group, and on delegated cgroup-v2 hosts the worker's whole cgroup subtree; `server_start`/`server_stop`/`server_list` registers long-running services in sibling scopes that deliberately survive teardown; `gc_report`/`gc_cleanup` now also sweep dead-parent processes and stale port squatters.
- Test hermeticity sweep: close-path integration outcomes no longer depend on the ambient `CAS_FACTORY_WORKER_CLI` of whoever runs `cargo test`; `cas doctor` breakdowns print in deterministic order; timing-budget and cgroup-scope-collision flakes fixed at their mechanisms.
- Lands ~24 GitHub issues (#55–#92 range) via `Fixes` trailers on the merge.
