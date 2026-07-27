---
from: ozer factory — session ozer-warm-owl-12, supervisor lively-dolphin-39
date: 2026-07-27
priority: P1
cas_task: cas-9599 (already filed; this adds a concrete cost case)
---

# Normal-priority messages never reach an idle worker — only `interrupt` delivers

Already filed as cas-9599. Filing again because this session produced a clean, expensive demonstration of the failure mode: it wedged a worker for ~30 minutes and left its work stranded three merges behind the epic.

## Symptoms

`coordination action=message` to a worker that is idle-and-waiting reports `Message queued — queued for next poll (target is registered)`. It is never delivered. The worker continues waiting for the answer that was already sent. Only `action=interrupt` (urgent=true) actually reaches it.

The pathological case is precisely a worker blocked *on a supervisor answer*: it goes idle waiting, which is the exact state in which it cannot receive the answer.

## Concrete evidence

`quiet-spider-15`, task cas-77aa, blocked on a one-word copy approval:

- Worker asks for confirmation of a CEO-facing string.
- Supervisor sends approval as a normal message — **msg 4716**. Queued. Never delivered.
- Worker re-asks, explicitly stating it is blocking final commit.
- Supervisor re-sends as **msg 4724 (interrupt)** — delivered immediately, worker unblocked.

Roughly 30 minutes lost on a question already answered. Second-order damage was worse than the delay: while wedged, the worker's branch stayed at base `staging` while three other tasks merged into the same files (cas-5a7a, cas-09e6, cas-93f1). Its work is now a reconciliation job (cas-8cbc) instead of a clean merge.

Same session, same pattern with `zealous-marten-56` on cas-8f6a — a normal-priority merge confirmation went undelivered and the follow-up had to be an interrupt.

## Workaround applied

Send anything a worker might be blocking on as `action=interrupt`. This is bad practice as a default: interrupt is documented to discard in-flight reasoning, so using it routinely trades one failure mode for another. It is currently the only reliable channel.

## Likely root cause

Delivery appears tied to a poll that a genuinely idle worker is not performing, or to an inbox drained only on turn boundaries the worker no longer reaches. "Registered" is being treated as "will poll", which does not hold for a worker parked awaiting input.

## Proposed fix

- a) **Deliver on idle transition (leaned).** When a worker goes idle with queued messages, flush the queue immediately — idle is the safest possible moment to deliver, since there is no in-flight reasoning to disturb.
- b) Escalate automatically: if a normal message is undelivered after N seconds and the target is idle, upgrade it to interrupt semantics. Idle makes the upgrade harmless.
- c) At minimum, stop reporting success. `Message queued (target is registered)` reads as delivered. Surface `undelivered_after` in `message_status` and warn the sender, so a supervisor knows to escalate rather than assuming the worker is ignoring them.

(c) alone would have saved most of the 30 minutes here — I believed the message had landed and read the silence as the worker being slow.

---

## Resolution

**Resolved 2026-07-27 — shipped in cas v2.29.0 (`origin/main`).** Task: cas-893c, cas-7210

Two defects. (1) A Claude recipient under agent-teams got `DeliveryChannel::TeamsInbox`, a file write only read at a turn boundary — an idle worker has no turn boundary, so only urgent/PTY delivery worked. Fixed with a PTY nudge for teams-inbox recipients (`24c77f0`, merge `0c6d73b`). (2) Head-of-line starvation in `peek_for_targets`: a flat `ORDER BY priority, id LIMIT 10` let one target's never-resolving backlog consume the entire poll window every tick, starving all other targets including urgent traffic. Fixed via `ROW_NUMBER() PARTITION BY (target, priority)` in `816d32b` (merge `d95df62`).
