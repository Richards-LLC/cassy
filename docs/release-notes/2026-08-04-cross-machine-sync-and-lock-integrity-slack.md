# 2026-08-04 — v2.39.0 cross-machine sync + lock integrity — #cas-internal posts

## Post 1 — User

**Live on production — User** (v2.39.0)

Was: work done on one machine quietly never reached any other one — tasks, memories and bug reports piled up locally while the sync that was meant to move them had been doing nothing for weeks. Now: sync actually runs, and when it cannot, it says so.

- If you use CAS on more than one computer and found that switching machines meant starting from an empty-looking project, this is the cause. Nothing was deleted. The queue of unsent work was building up untouched, and the part of the program responsible for sending it was never being called at all.
- Because a silent failure is worse than a loud one, a failed send now records the error and the attempt count instead of leaving the queue looking untouched. Silence used to be indistinguishable from success.
- Worktrees that appeared to be locked by nobody now release properly. If you ever hit an operation that refused to proceed because something was "in use", with no sign of what was using it, that lock was being held open by a background process that had already finished its work.
- Bug reports written on one machine now reach the project instead of sitting unread on the machine that wrote them. Reports go to a configured issue tracker, and the report is written to disk before anything is sent, so a failure to file can no longer lose what you wrote.
- Closing out finished work no longer rejects the short commit references that every ordinary git command prints. Previously the rejection message looked identical to an unrelated problem, which sent people looking in the wrong place.
- Reviewing delivered work and asking for changes is now a supported action. Previously the only way to send something back was a recovery command meant for abandoned work, which discarded who was working on it.
- Notifications no longer describe things that have already changed by the time you read them.

## Post 2 — Dev

**Live on production — Dev** (v2.39.0)

Was: the automatic sync path performed a personal push and then a pull, with no team-queue drain in between, so team-scoped rows were never attempted at all. Now: the drain runs between push and pull, failures are recorded per row, and the queue reflects reality.

- The diagnostic signature is worth knowing: every queued row showed `retry_count = 0` and `last_error = NULL`. That combination means *never attempted*, not *failing* — an auth, network or payload problem leaves evidence on at least one row. Anyone debugging a stalled queue should read that pattern first and skip the transport entirely.
- Filesystem locks were released by closing the descriptor. POSIX `flock` releases only when every descriptor sharing an open file description closes, and `fork` hands the child a duplicate — so a parent dropping its guard released nothing while any forked child survived, and `FD_CLOEXEC` does not help because it acts on `exec`, not `fork`. Four call sites now issue an explicit `LOCK_UN` before close, in a non-panicking `Drop`; five others already did it correctly, so the fix converges on the pattern that was already the majority.
- The temp-file name used by the atomic-write helper combined only the target name, the process id and a wall-clock timestamp. Same-process writers could collide when clock resolution is coarse, and the loser's unconditional cleanup deleted the winner's file. Naming now uses a process-local atomic counter, and cleanup is armed only after `create_new` proves ownership — the failure mode degrades to a harmless retry rather than corrupting a peer.
- Merge targets are now resolved from the task being merged, then the assignee's tasks, then explicit authorization, then refusal. Session focus is a display affordance and is deliberately absent from that chain; merges into a closed target are refused outright.
- The `mcp-server` feature is removed rather than repaired. Building without it compiled out the server while the terminal layer still launched it, so a "successful" build produced a binary that advertised orchestration and exposed no tools. A flag that cannot yield a working binary is not a flag. Removing it also un-hid roughly 800 tests that had been silently excluded from every run.
- Queued messages are revalidated against live state immediately before transport, so a request describing a merge that already landed is suppressed and replaced with guidance rather than delivered. Uncertainty always delivers: only positive proof of staleness suppresses.
- Continuous integration ran for the first time in this project's history. It had been registered and inactive for four months, and enabling it immediately surfaced a linker misconfigured since April, a toolchain mismatch on macOS, tests that only passed where a particular CLI happened to be installed, and a process-environment race between concurrent tests. Two of those had broken on the same day in April and stayed invisible for three months. The pipeline now returns in about twenty minutes, with the expensive release-profile gate moved off the per-change path and onto merges and the nightly schedule.
