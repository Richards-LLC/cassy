# v2.37.0 — post-reboot sweep — Slack release notes (#cas-internal)

## Thread 1 — User

**Top-level:**
Live on production — **User** — The factory stops asking workers to redo work that's already finished, and multi-persona code review now runs on a faster, cheaper engine that actually reports when a reviewer couldn't run.

**Reply:**
- Finished-but-unmerged work: **Was** → after a task finished and was waiting on its merge, the coordinator could hand that same task back to a worker as if it were new — risking destructive re-runs. **Now** → finished work is never re-offered; the person who owns the merge gets the nudge instead.
- Merge reminders: **Was** → "merge needed" alerts could keep firing after the merge had already landed, using out-of-date evidence. **Now** → alerts re-check the live repository before firing, retract themselves once the merge lands, and say so when local and remote history disagree.
- Missed instructions: **Was** → a busy worker could miss a coordination message with no way to catch up. **Now** → workers can pull their unread messages on demand, and announcements stay visible to every recipient until each one has seen them.
- Code review: **Was** → review personas were stuck choosing between a lower-quality cheap model and an expensive premium one, and a reviewer that failed to run looked identical to a clean pass. **Now** → reviews run on GPT-5.6 Sol with a hardened pipeline; a reviewer that couldn't run is reported as skipped, never as a silent pass. Security review stays on Claude so two independent vendors look at every risky change.

## Thread 2 — Dev

**Top-level:**
Live on production — **Dev** — v2.37.0: dispatch predicates now key off task status (awaiting_merge is never assignable), merge alerts recompute against fresh local+origin refs, prompt-queue gains a per-recipient seen/ack model with migration, and the review-persona fleet moves to `codex exec -s read-only` behind a strict-schema shim.

**Reply:**
- Director dispatch: **Was** → dispatch/rescue/nudge paths keyed off assignment + lease state, so awaiting_merge tasks leaked back into TaskAssigned/WorkerStalled delivery. **Now** → worker-actionable = Open/InProgress only, with delivery-time revalidation regressions for both races.
- Merge-alert evidence: **Was** → computed once from local refs; any stale zero suppressed the alert. **Now** → bounded best-effort fetch of the remote-tracking epic ref at emission, origin-authoritative precedence (a local zero can't mask a remote positive), ref-disagreement disclosed in the alert text.
- Worker inbox: **Was** → prompt-queue rows were daemon-transport-only; broadcast rows vanished for everyone once any recipient acked. **Now** → `inbox_poll` coordination action with per-recipient seen state (new table + forward migration), per-recipient broadcast ack, seen-row lifecycle tied to queue cleanup, checked limit conversion, handler-level test coverage.
- Review transport: **Was** → persona dispatch was Claude-only enum; the first Codex cut 400'd against strict structured outputs and skipped lanes counted as completed reviews. **Now** → shared shim (strict-schema transform, distinct schema-vs-timeout retry budgets, per-persona timeout, bounded process fan-out, portable timeout resolution), skip accounting surfaces degraded reviews explicitly; security persona remains on Claude Opus.
- Harness correctness: **Was** → close-gate remediation hardcoded the Claude MCP alias (unusable from Codex/Grok workers) and a fallback could rewrite Codex→Claude without checking Claude exists. **Now** → remediation renders the caller-harness tool name and requires drain-until-empty inbox polling; fallback validates the target binary and errors clearly when no harness is available.
- Turn tracking: **Was** → `is_turn_in_flight` stuck true for Codex panes after the first turn. **Now** → helper renamed and scoped to Grok (its only reliable signal), with sticky-state regression tests for Claude/Codex.
