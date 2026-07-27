# v2.30.0 — lease correctness + wave-2 bug sweep (Slack drafts)

Channel: #cas-internal (C0B44GUKDK2). Two top-level posts, per `docs/RELEASE_SLACK_RUBRIC.md`.

---

## Post 1 — USER

**Live on production — v2.30.0**

Work that ran longer than about half an hour could quietly be treated as abandoned — while someone was still actively doing it. Now long work stays owned by whoever is doing it, and the things that used to strand finished work no longer do.

**Was → Now**

- **Was:** anything taking more than roughly half an hour was silently marked as unclaimed, even though it was being actively worked on and reporting in the whole time. Anyone starting a new session could then reset it to "not started", so the same work could be done twice with nothing warning you. **Now:** ongoing work keeps its claim for as long as it's genuinely alive.
- **Was:** finished work could get parked "waiting to be merged" with no way forward — if that merge couldn't actually happen, nobody could pick it back up and nothing said anything was wrong. It looked complete. **Now:** parked work can be resumed, and work stuck on a conflict is clearly shown as unfinished rather than done.
- **Was:** one merge hitting a conflict left everything in a half-finished state, so the next completely unrelated merge failed with an error describing the *previous* problem. Everything queued up behind it until a human noticed. **Now:** a failed merge cleans up after itself and tells you exactly which files clashed.
- **Was:** sending someone an urgent note — even just "save your work, you're running low on room" — could leave them unable to hand in work they'd already finished. **Now:** an urgent message no longer blocks anyone from finishing what they already own.
- **Was:** the status display could insist something was stalled at the same moment the dedicated health check said it was fine and working. **Now:** both read from the same evidence, so they can't contradict each other.

---

## Post 2 — DEV

**Live on production — v2.30.0**

`renew_lease` had zero production call sites. Task leases expired after 30 minutes under live, heartbeating holders — status and assignee left intact — so anything long-running became silently reclaimable. Fixed at the store layer, plus four other correctness defects found alongside it.

**Was → Now**

- **Was:** nothing renewed a task lease. The nominal renewal site was an MCP handler wrapper, but the daemon's production liveness ticker calls `store.heartbeat()` directly and bypasses it — which also meant worktree-lease renewal had never actually run either. Past the 30-minute default the lease flipped to expired while `task.status` and `task.assignee` stayed set. **Now:** both lease kinds renew inside the same transaction as the heartbeat write. Dead-holder recovery is unaffected: staleness already revokes all of a holder's leases within ~30–75s, independent of any lease's own expiry, so the timer was contributing nothing but false negatives.
- **Was:** nine consumers read lease state; two were materially wrong past the expiry window. Orphan cleanup — which runs on *every* session start of *any* agent — reset live, actively-worked items back to open with the assignee still attached, needing no misbehaviour to fire. Verification-skip resolution silently bypassed the verifier on long items closed on a holder's behalf, believing the holder unresolvable. **Now:** correct as a consequence of renewal rather than by individual patch; both were confirmed by constructing the expired-lease-with-live-holder state rather than reasoning from call sites.
- **Was:** a conflicting merge was detected by scanning stderr for conflict markers, but git writes them to stdout. Real conflicts fell through to a bare "failed to execute git command" and left `MERGE_HEAD` and a partially-staged index behind, so every subsequent unrelated merge died on "you need to resolve your current index first". **Now:** both streams are scanned, any failure aborts cleanly, conflicts are pre-flighted with `merge-tree --write-tree` before the working tree is touched, and the error names the conflicting paths.
- **Was:** work parked awaiting a merge could not be restarted, so a branch that couldn't merge had no exit at all — the state that means "done pending a formality" was indistinguishable from "stuck with unfinished work". **Now:** parked work is restartable, conflicted parks are flagged distinctly with the conflicting paths, and the holding branch name is recorded at park time so commits survive holder loss.
- **Was:** the urgent-stop halt vetoed the holder's own close, and the verification entry point had no ownership exemption at all — producing a genuine mutual deadlock where close was refused by the halt and the step the halt told you to take was refused by verification. **Now:** the exemption covers a caller's own in-progress work and both entry points, strictly ownership-scoped — it never clears the halt itself and never bypasses merge, verification or review gates.
- **Was:** the status view computed "stalled" from activity age alone and could contradict the dedicated liveness check for the same process at the same instant. **Now:** it consumes the same in-flight evidence that check uses, and distinguishes idle-holding-work from idle-with-nothing-assigned.

Full workspace suite green on the release commit: 102 result blocks, 0 failures.
