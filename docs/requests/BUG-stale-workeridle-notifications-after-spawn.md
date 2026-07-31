# BUG: WorkerIdle notifications fire in the spawn→assignment window and arrive as false alarms

**Observed:** 2026-07-31, Woodworking factory session `Woodworking-strong-octopus-57`, cas 2.38.1 (3c0e189). Supervisor agent `young-wolf-6`.

## Symptom

After `spawn_workers`, a worker registers and is briefly taskless while the supervisor assigns it. The idle watcher fires during that gap. The supervisor then receives "ready and waiting for tasks" / "idle with no assigned tasks" messages **after** the assignment has already landed and the worker is actively executing.

Worst instance this session: **six** notifications for three workers (`young-bear-65`, `tender-koala-51`, `vivid-spider-44`) — each got both a "ready" and an "idle" message — all arriving after every one of them was `InProgress`. Verified against live state at the time:

```
cas-eb29  InProgress   tender-koala-51  (last activity 8s ago)
cas-eda5  InProgress   vivid-spider-44  (last activity 1s ago)
cas-d60b  InProgress   young-bear-65    (last activity 0s ago)
```

An earlier single-worker spawn produced the same false pair.

## Why it matters

The notification text instructs the supervisor to assign work — the exact action that has already been taken. Acting on it means either re-assigning a busy worker or messaging one mid-turn and interrupting correct work. Not acting on it means learning to discount idle signals, which is worse: this session ALSO produced a *genuine* stuck-worker condition (`⚠ ASSIGNED BUT UNSTARTED: cas-d283 assigned 741s ago`). Real and false signals are being delivered through the same channel with the same urgency, which trains the supervisor to ignore the channel that carries the real one.

## Workaround used

Treat every idle notification as unverified: query `task list status=in_progress` plus `worker_status` before responding, and only act if the worker genuinely has no task. Costs two tool calls per false alarm.

## Suggested fix

- Debounce: require a worker to be taskless for longer than a spawn→assign round trip (or N consecutive checks) before emitting WorkerIdle.
- Suppress WorkerIdle for workers whose registration is younger than that threshold — a just-spawned worker is expected to be taskless.
- Re-validate at delivery time: drop the notification if the worker has acquired a task between enqueue and delivery.
- Distinguish "never had a task" from "finished a task and is now free" in the message text, so the genuinely-actionable case is separable at a glance.
