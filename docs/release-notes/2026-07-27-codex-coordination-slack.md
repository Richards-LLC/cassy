# v2.31.0 — Codex worker coordination + honest liveness signals (Slack drafts)

Channel: #cas-internal (C0B44GUKDK2). Two top-level posts, per `docs/RELEASE_SLACK_RUBRIC.md`.

DRAFT — do not post until the release is pushed to `origin/main` and tagged.

---

## Post 1 — USER

**Live on production — v2.31.0**

Sending an urgent correction to an assistant could silently kill it. It stayed alive, kept reporting in, and never did another thing — and every message you sent afterwards vanished into it. Now an urgent correction lands, gets read, and gets acted on.

**Was → Now**

- **Was:** interrupting someone mid-task to redirect them could leave them permanently unreachable. The interruption landed, their current work stopped, and the correction you sent never arrived — it sat unsent, half-typed, where nobody could see it. Every later message you sent just piled onto the same invisible draft instead of being delivered. From the outside it looked like they were simply ignoring you. **Now:** the correction waits for the right moment to land, arrives whole, and gets acted on — and they stay reachable afterwards.
- **Was:** the only way out was to shut the assistant down and start a fresh one, losing whatever it had been doing. **Now:** no restart needed; an interrupted assistant carries on with the new instructions.
- **Was:** the status view could report someone as stalled while they were actively working — editing files, running tests, making progress every single minute. The "last activity" time simply stopped moving. Acting on that reading meant shutting down healthy work mid-flight. **Now:** activity reflects what someone is actually doing, not just the last time they happened to check in.
- **Was:** you could be told a colleague was "idle and waiting for work" when they were in the middle of an assigned task, or told to merge something that had already been merged minutes earlier. These notices arrived long after they stopped being true, but still read as current. **Now:** notices that have gone out of date are withdrawn before you see them, and the ones that reach you are still true.
- **Was:** a search that found nothing looked exactly like a search that ran correctly and found nothing to report. **Now:** a search that matched nothing anywhere is called out, so a broken query can't be mistaken for a clean result.

---

## Post 2 — DEV

**Live on production — v2.31.0**

Urgent interrupt delivery to Codex panes was racing the harness's own teardown: the trailing submit CR landed mid-transition and was silently swallowed, leaving the redirect as an unsent composer draft that every subsequent delivery appended to. Live-reproduced against the real binary, not inferred. Plus the liveness and alerting fixes found alongside it.

**Was → Now**

- **Was:** `interrupt_and_inject` broke the turn, slept a flat 1200 ms sized against Claude Code's cancel latency, then wrote the payload and a bare CR. On Codex that CR could arrive while the TUI was still tearing down from the cancel, where it was silently dropped — turn aborted, nothing delivered, payload stranded in the composer. Later deliveries concatenated onto that stranded text rather than submitting fresh prompts, which is the mechanism behind "alive but permanently deaf". The settle function computed an output-quiescence snapshot and then discarded it unused. **Now:** the wait actively drains the pane and polls for genuine output stability before injecting, bounded by a floor plus a 4 s cap that still injects on timeout. Claude and Grok keep the original flat-sleep path untouched. The multi-line-payload theory was tested live and disproven — payload shape was never the cause; `supports_textbox_submit` now gates the two paths as a real consumer instead of being a flag nothing read.
- **Was:** worker liveness age was derived solely from the event store. Harnesses without hook support emit events only on their own tool calls, so a heads-down session froze the clock at its last check-in — and the same freeze hit hook-capable harnesses during any dense stretch of short calls, since the gap between calls is what matters, not the harness. Sessions were reported stalled while writing their transcript every minute with a two-second-old heartbeat beside the stale reading. **Now:** activity age folds transcript freshness for all harnesses via the same primitive the dedicated liveness check already trusts, taking whichever signal is fresher — it can only ever read fresher, never staler, so a genuinely dead session is still caught. Per-harness freshness windows are unchanged. The existing in-flight-tool-call suppression was verified correct and deliberately left alone; it covers one long outstanding call, which is a different shape.
- **Was:** inbox alerts were a write-once append, revalidated against live state exactly once at write time, with the read flag never set by production code. Recipients poll only at their own turn boundaries, so a notice generated while true sat unchallenged for minutes and was read long after it went stale — including merge prompts citing a superseded branch tip while labelled as live evidence. **Now:** these rows are tagged with what they concern and swept each refresh tick, retracting any that no longer hold, using the same predicate the delivery-time revalidation uses so the two cannot disagree. Still-valid notices are untouched and already-read rows are never rewritten.
- **Was:** an investigation could close on a prose conclusion with no way to distinguish "searched and found nothing" from "the query was malformed and matched nothing anywhere". **Now:** an optional search manifest of commands and hit counts can be attached at close; any step reporting zero hits raises a warning rather than passing silently. Opt-in and advisory — it never blocks a close, and work that doesn't supply one is untouched.

Full workspace suite green on the release commit.
