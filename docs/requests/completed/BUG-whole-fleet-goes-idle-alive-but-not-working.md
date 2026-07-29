---
from: ozer factory — session ozer-warm-owl-12, supervisor lively-dolphin-39
date: 2026-07-27
priority: P0
cas_task: cas-5706
---

# Whole worker fleet goes "alive but not working" — heartbeating, zero activity, assigned tasks never started

## Resolution (v2.33.0 verification)

Resolved as triaged by `cas-5706`.

- **Assigned tasks never started:** shipped prompt-queue isolation prevents one
  target's undeliverable backlog from excluding fresh worker briefings
  (`crates/cas-store/src/prompt_queue_store.rs:1458-1504`), while bounded retry
  and terminal abandonment prevent failed rows from remaining permanent poison
  (`crates/cas-store/src/prompt_queue_store.rs:744-831`). Startup cleanup now
  waits for the roster to populate before abandoning stale targets
  (`cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs:407-428`).
- **Interrupts queued but unconsumed:** urgent sends enter the same durable queue
  at Critical priority
  (`cas-cli/src/mcp/tools/service/agent_search_system/message.rs:283-297`,
  `:450-465`), receive the per-target isolation above, and then use the
  interrupt-and-inject path
  (`cas-cli/src/ui/factory/daemon/runtime/queue_and_events.rs:786-812`).
  Codex delivery now waits for real post-Esc output quiescence before submitting
  the redirect (`crates/cas-mux/src/mux.rs:871-889`, `:938-987`), preventing the
  swallowed-submit/permanent-deafness failure.
- **No escalation for assigned-but-unstarted workers:** still reproducible in
  v2.33.0. `worker_status` only marks workers stalled when they have an active
  lease or an `InProgress` task; an assigned `Open` task still renders
  `none in last 10m (may be investigating or idle)`
  (`cas-cli/src/mcp/tools/service/factory_ops.rs:936-990`,
  `:2257-2291`). The fix is tracked by `cas-78bf`.

The historical incident did not retain prompt-queue row telemetry, so the exact
causal identity cannot be proven after the fact. The shipped changes do map to
and prevent the reported delivery signatures; the remaining observability gap
is explicitly tracked rather than silently treated as fixed.

Single most expensive failure of the session. Happened **twice** in about ninety minutes, to two independently spawned fleets, and no alert fires for it.

## Symptoms

All workers report healthy heartbeats (5–30s) while doing nothing at all:

```
• zealous-marten-56 (heartbeat: 7s ago)
  last activity: none in last 10m (may be investigating or idle)
```

Simultaneously:
- `task list status=in_progress` returns **zero tasks**, despite every worker having an assignee set.
- Supervisor messages queue but never take effect, including `action=interrupt`.
- Worktrees are **clean** and all branches **pushed** — so this is not a crash mid-work.

The parenthetical "(may be investigating or idle)" makes a wedged fleet indistinguishable from a busy one. There is no threshold at which it escalates.

## Concrete evidence

**Fleet 1** — 5 workers (codex `gpt-5.6-sol`, effort medium, isolate=true), spawned ~15:17 UTC. Productive for roughly 40 minutes, then all five simultaneously showed no activity for 10m+ with three tasks assigned-but-unstarted. Two `interrupt` messages to `tender-hound-11` over ~30 minutes produced no change; it stayed on a P2 while a P0 sat unstarted.

**Fleet 2** — 4 workers, same spec, spawned ~16:55 UTC. Same signature within ~25 minutes: all four heartbeating, `in_progress` count zero, four assignments untouched, four interrupts unconsumed.

Both fleets were shut down and respawned. Both times the replacement fleet picked up work within ~90 seconds, confirming the tasks and assignments were fine and the workers were the problem.

## What it cost

Roughly an hour of wall-clock across the session, plus every supervisor turn spent re-briefing and re-interrupting workers that could no longer receive anything. The operator noticed before the tooling did — "the workers appear to not be working" and later "zero workers are working, we are making no progress". Both times the supervisor had been treating individual stalls as one-off rather than recognising a fleet-wide pattern, because nothing aggregates it.

## Contributing hypothesis (unproven)

Both wedges followed the same shape: worker completes a task → pushes → goes idle → receives a long supervisor briefing (400+ words with heavy markdown) → never recovers. The third fleet was deliberately given one-to-three-line messages with all substance left in task descriptions, and did not exhibit the failure in its first cycle. That is suggestive, not conclusive — a single observation.

If message size or delivery is implicated, this likely shares a root with `BUG-normal-priority-messages-never-reach-idle-workers.md`: in both, an **idle** worker is the one that cannot receive.

## Proposed fix

- a) **Alert on it (leaned, and independent of root cause).** A worker with an assigned task, a live heartbeat, and zero activity past a threshold is a defect state, not an ambiguous one. Emit a supervisor notification. Today the only signal is a human noticing. `worker_status` already computes "none in last 10m" — it just never escalates.
- b) Distinguish idle-with-assignment from idle-without. "(may be investigating or idle)" is acceptable for an unassigned worker and misleading for one holding a P0.
- c) Track message consumption. If a worker has queued or interrupt-delivered messages it has not acted on after N seconds, surface that — it is the earliest observable symptom.
- d) Investigate whether large injected messages can wedge a codex worker session, and cap or chunk injection if so.
- e) Consider a supervisor-side `worker_ping` that verifies liveness by round-trip rather than heartbeat. Heartbeat currently proves the process exists, not that it can still act.

Recommend (a) regardless of what causes it: a fleet can be completely dead for ten minutes with every indicator green, and the only reason it was caught either time is that the operator asked.
