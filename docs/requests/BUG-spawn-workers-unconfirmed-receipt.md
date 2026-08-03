> Migrated to GitHub Issues: [#60](https://github.com/pippenz/cas/issues/60)

# BUG: `spawn_workers` receipt does not confirm worker liveness

**Component:** factory spawn queue, MCP coordination API, factory daemon logging  
**First observed:** 2026-07-27 on soundwave with CAS 2.30.0 (`811377c`)  
**Rechecked:** 2026-07-31 in factory session `Penguinz-proud-crane-49`

## Original reproduction — functional failure on 2026-07-27

Two MCP calls returned success-shaped enqueue receipts:

- request 43: `spawn_workers count=1 isolate=false cli=codex model=gpt-5.6-sol effort=medium`
- request 44: the same request without `task_id`

Each response said `Queued spawn request for 1 worker(s) (request ID: N)`. After more than 25 seconds, `worker_status` still showed no active workers, there were no Codex processes, and `cas list --json` showed a healthy/attachable factory daemon with `workers: []`. Both the session's `daemon.log` and `daemon-trace.log` were zero bytes, leaving no evidence that the requests were received, attempted, rejected, or failed.

## Recheck — functional symptom did not reproduce on 2026-07-31

In `Penguinz-proud-crane-49`, spawn requests 75 and 76 returned the same success-shaped queued-request receipts, then did produce live workers:

- `tender-hawk-14`
- `clever-octopus-61`

`worker_status` reports both workers live with current heartbeats, clean factory worktrees, and active Codex sessions. This rejects the strong hypothesis that MCP-only supervisors can never have a spawn-queue consumer. It does not identify the intermittent/lifecycle cause of the 2026-07-27 failure.

The current session still has no daemon audit trail. At the time of this recheck, both files were exactly zero bytes and had not changed since session startup at 2026-07-31 08:45:25 EDT:

- `~/.cas/logs/factory/Penguinz-proud-crane-49/daemon.log`
- `~/.cas/logs/factory/Penguinz-proud-crane-49/daemon-trace.log`

There is also an observability mismatch: `worker_status` sees the two live workers, while `cas list --json` reports `workers: []` for this factory session. Therefore `cas list` is not a sufficient post-spawn liveness guard today.

## Remaining valid requirements

1. **Root cause:** still unresolved. Today's success disproves only the universal “no consumer without an attached TUI” theory; the original queue-consumption failure remains unexplained.
2. **API honesty:** still required. `Queued spawn request ...` confirms only queue insertion and has the same shape whether a worker later registers or nothing happens. The API should either wait for bounded liveness confirmation or explicitly return an unconfirmed request handle and an actionable failure/timeout state.
3. **Daemon audit logging:** still required. Every received spawn request and terminal outcome (registered, launch failed, cancelled, or timed out) must leave an audit record. A running daemon that has processed successful spawns must not leave both daemon logs empty.
4. **Reproduction/demo:** today's functional path passes because live workers appear, but callers still cannot infer that from the enqueue receipt. The demo should prove either confirmed liveness or a plain actionable error.

## Working supervisor-side guard

Until the API confirms liveness, call `worker_status` after every `spawn_workers` request and do not report dispatch complete until the expected worker appears with a live heartbeat. This guard was used successfully three times on 2026-07-31. Do not rely on the enqueue receipt alone, and do not use `cas list --json` as the sole check while its worker list can be stale.
