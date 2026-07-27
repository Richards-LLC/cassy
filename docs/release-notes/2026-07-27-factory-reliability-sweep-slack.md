# v2.29.0 — factory reliability sweep (Slack drafts)

Channel: #cas-internal (C0B44GUKDK2). Two top-level posts, per `docs/RELEASE_SLACK_RUBRIC.md`.

---

## Post 1 — USER

**Live on production — v2.29.0**

Agents used to go quiet: still running, still reporting healthy, but no longer receiving anything you sent them. Now instructions reach every agent, every time — and when something really is stuck, you get told instead of having to notice.

**Was → Now**

- **Was:** an agent could stop receiving your messages part-way through a session while looking perfectly healthy — a message to the whole team could reach nobody at all. **Now:** every agent gets its fair share of the queue, so one blocked agent can no longer starve everyone else of instructions.
- **Was:** "message delivered" meant the message had been written down, not that anyone read it. **Now:** delivery reporting reflects whether it actually arrived, so silence is distinguishable from being ignored.
- **Was:** you'd get urgent "this agent is stalled, kill it" warnings about agents that were working normally — often ones running a long test suite exactly as intended. **Now:** work in progress is recognised as work, and those warnings only fire when something is genuinely stuck.
- **Was:** finished work could get trapped — the system asked for a step that had already been done, then refused to accept the result. **Now:** completed work closes out the first time.
- **Was:** cleaning up after a piece of work could delete files that were never saved anywhere else, and could break shortcuts elsewhere on your machine that pointed at them. **Now:** cleanup refuses and tells you what would be lost.
- **Was:** a routine safety check cried wolf on a file the system creates itself, so the warning got ignored — including the times it was real. **Now:** it only objects to genuine unsaved work, and names the files.
- **Was:** a leftover safety lock could block you from committing in your own repository long after you'd finished. **Now:** it stays scoped where it belongs and old ones are cleared automatically.

---

## Post 2 — DEV

**Live on production — v2.29.0**

Message delivery had head-of-line starvation across recipients: one target's backlog consumed the entire poll window every tick, so unrelated live recipients received nothing — including urgent traffic. Fixed, plus nine other reliability and data-safety defects.

**Was → Now**

- **Was:** the prompt-queue poll used a flat `ORDER BY priority, id LIMIT n` across the whole session. Any recipient accumulating `n` never-resolving rows (retryable pending reasons keep `processed_at` NULL) permanently filled the window, and messages to other recipients never appeared in the batch at all. **Now:** rows are ranked `ROW_NUMBER() OVER (PARTITION BY target, priority ORDER BY id)` and ordered by `(priority, rank, id)`. Priority stays dominant; within a band every recipient's oldest row is considered before any recipient's second. No-op for the single-recipient case. Reproduced against the pre-fix query before fixing.
- **Was:** delivery status was stamped on transport write, so `delivered` meant "bytes written", never receipt. **Now:** an undelivered-duration is derived from unacknowledged receipt and surfaced to the sender.
- **Was:** close-time gates resolved the parent branch through a different path than the merge gate, falling back to a hardcoded `main`. On a branch based on anything else this made the "already merged" check always false — an unresolvable close loop — and computed diff stats across the entire trunk divergence (~110KB, over the tool-result limit). **Now:** one resolver for all five gates, no hardcoded fallback, output capped with the confirmation line first.
- **Was:** stall and merge alerts asserted state captured when they were queued, never re-checked at send time; they fired for already-merged branches and for processes mid-tool-call. **Now:** both re-validate against live state, embed the evidence they were computed from, and share one liveness function with the diagnostic command — so the alert and the tool you'd use to check it cannot disagree.
- **Was:** worktree removal treated untracked files as safe to discard and forced past git's own refusal. Untracked files exist only in that directory — no blob, no index entry, no reflog. **Now:** operations that delete the directory block on untracked content; operations that preserve it still only warn. Offending paths are named.
- **Was:** removal didn't check for external symlinks resolving into the worktree, so cleanup could leave dangling links elsewhere on the filesystem. **Now:** they're detected and reported through the existing cleanup result types, independent of force.
- **Was:** the dirty check treated any porcelain output as blocking, including a hook directory the tool creates itself — training operators to force past it habitually. **Now:** tracked modifications block, untracked warns, self-generated artifacts are excluded.
- **Was:** the worker commit guard was written to the shared common hooks dir, which linked worktrees share with the main checkout, and nothing removed it. **Now:** it's scoped per-worktree via `core.hooksPath` with worktree-local config — survives abrupt termination — and legacy copies are migrated away on install without touching project hooks.
- **Was:** `reopen` refused blocked items and silently dropped the supplied reason. **Now:** blocked is accepted, the reason is captured in the audit trail, and the rejection for other states names the alternative.

Full workspace suite green: 102 result blocks, 0 failures.
