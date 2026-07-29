# BUG: worker reports are re-delivered to the supervisor, and supervisor replies sit in `awaiting_ack` forever

**Filed:** 2026-07-28
**Reporter:** supervisor `wild-condor-51`, factory session `Penguinz-witty-viper-34` (project: Penguinz, host: soundwave)
**Severity:** High — not corrupting, but it burned enough worker context to force a mid-epic rotation.

## Summary

Across a single ~7-hour epic (`cas-1d55`, 9 tasks, 2 workers) the same worker report was delivered to the supervisor **nine times** as a fresh injected turn, in every case describing state the supervisor had already changed. In parallel, **every** supervisor reply reached `stage: delivered / pending_reason: awaiting_ack` and never advanced to confirmed.

The two halves compound. The worker does not observe the reply, so it re-sends its full report. The supervisor answers again. Neither side is wrong; both are acting on stale state.

## Observed

Representative cycle:

1. Worker finishes `cas-b769`, sends a ~600-word structured report ending "MERGE NEEDED".
2. Supervisor merges, verifies, replies (message id 625).
3. `message_status 625` → `stage: delivered`, `pending_reason: awaiting_ack`, `wake: unobserved`, `reaction: unobserved`, `confirmed_at: null`.
4. The identical worker report arrives again as a new injected turn.
5. Supervisor re-checks state, confirms nothing outstanding, replies again.

Message ids exhibiting `delivered / awaiting_ack / confirmed_at: null` in this session: 611, 615, 621, 625, 629, 633, 634, 638, 640, 643, 646, 652, 657, 662, 666. That is every message sent.

By the end the lag exceeded one full exchange: the worker sent a **correction to the supervisor about state the supervisor had already corrected** — it reported `9b013f2` as needing a merge after it had been merged as `1d00b7f`, and separately reported `cas-c266` as outstanding after it had been closed. On one occasion the supervisor had to issue an `urgent=true` interrupt to stop a worker re-running ~10 minutes of NEF hashing it had already completed and committed.

## Impact

- **Context exhaustion.** Worker `fast-panther-40` hit `context: near-limit (~391k tk)` and had to be rotated out mid-epic. A large share of that was re-narrating completed work into messages that were already answered. The replies it never saw were themselves long, because the supervisor was answering points the worker had already superseded.
- **Wasted round trips.** ~9 duplicate cycles, each carrying a multi-hundred-word report and a multi-hundred-word reply.
- **Near-duplicate execution.** Without the urgent interrupt, one worker would have redone a completed, committed, merged task.
- **`worker_idle` false positives compound it.** A `worker_idle` notification arrived for a worker whose `last activity` was 4 seconds prior and which completed its task minutes later. Adjacent to the already-filed `BUG-stall-detector-false-positive-ignores-worker-tool-calls`.

## Suspected shape

`message_status` distinguishes `delivered` (transport handoff succeeded) from confirmed receipt, and its own help text warns they are not the same — so the ack path is known-weak by design. What appears to be missing is the consequence: nothing appears to mark a queued worker report as *superseded* once the supervisor has responded to it, so it stays eligible for redelivery.

Two independent things worth checking:
1. Why `message_ack` never fires for a supervisor→worker message that the worker demonstrably acts on. The workers DID eventually act correctly on every instruction, so the content arrives — only the ack does not.
2. Why an already-delivered worker→supervisor report is re-injected rather than dropped.

## Suggested fix

1. Have the recipient ack on read, and surface unacked-but-acted-upon as a distinct state rather than leaving `confirmed_at: null` indefinitely.
2. De-duplicate worker→supervisor reports: if a report has already been injected and the supervisor has since sent a message to that worker referencing the same task, do not re-inject it.
3. Cheap mitigation that would have removed most of the cost here: include the recipient's last-seen message id in the injected turn, so both sides can see they are one exchange apart and answer briefly instead of re-sending in full.

## Workaround in use

Supervisor now verifies live state (`git merge-base --is-ancestor`, `task show`, `epic_status`) before responding to any worker report, and treats every incoming report as possibly stale. Workers were instructed to put durable detail in task notes and committed docs rather than in messages, and to send short status lines. That works, but it is a discipline compensating for the transport rather than a fix.

## Related

- `BUG-stall-detector-false-positive-ignores-worker-tool-calls.md` — same family: factory status signals reporting a state the worker is not in.
- `BUG-stock-worker-defaults-contradict-shipped-model-routing-policy.md`
- `FEATURE-code-review-personas-off-claude-only-model-enum.md`
