> **Disposition (2026-08-07, cas-ab75):** RESOLVED — filed as [#59](https://github.com/pippenz/cas/issues/59) (closed completed). Fix verified on `main`: `0da3fbff` (cas-2702, queue keeps draining past a wedged provision) plus `c2109a67` "feat(factory): make spawn lifecycle and worker assignment queryable" — spawn state now advances queued → provisioning → launched → registered/failed, so a spawn that provisions a worktree but never launches a CLI surfaces as FAILED instead of silence. Archived.

> Migrated to GitHub Issues: [#59](https://github.com/pippenz/cas/issues/59)

# BUG: spawn_workers creates worktree but never launches worker CLI after shutdown_workers count=0

**Reported:** 2026-07-31, factory session `gabber-studio-agile-dragon-52`, supervisor swift-bear-25 (Claude).

## Symptom
After issuing `coordination action=shutdown_workers count=0` (shutdown ALL) at 13:11 UTC (request 366), every subsequent `spawn_workers` request in the same session (requests 367 @ 13:57, 368 @ 14:01, both `count=1 isolate=true cli=codex model=gpt-5.6-sol effort=medium task_id=cas-8f06`) got stuck half-done:

- Worktree IS created (`.cas/worktrees/quiet-swan-82`, `.cas/worktrees/strong-bear-16`, correct branch `factory/<name>` at the epic base) — so the spawn daemon is alive and servicing the queue.
- Worker CLI process is NEVER launched: no codex process (verified via `ps`), no agent registration (`agent_list` shows only the shutdown agents + supervisor), no entry in `.cas/factory-process-groups/` (dir empty), `worker_status` says "Workers: None active", task pre-assignment never fires.
- No error surfaced to the supervisor; `spawn_workers` returns a normal "Queued spawn request" and then nothing.
- Session daemon log (`~/.cas/logs/factory/gabber-studio-agile-dragon-52/daemon.log`) has no entries at spawn time (last write hours earlier); `.cas/logs/factory-session-2026-07-31.log` records only `workers_spawn_queued`.

Earlier the same day, in the SAME session, spawns worked instantly (requests 361/362 → 3 workers, 365 → 1 worker). The only relevant state change between working and broken spawns was the shutdown-all request.

## Hypothesis
The `shutdown_workers count=0` directive is sticky/unexpired and the launcher applies it to newly spawned workers (kill-on-boot or skip-launch), or the shutdown handler tears down the per-session launcher loop while leaving worktree provisioning running.

## Impact
Factory cannot spawn any further workers for the rest of the session; supervisor had to hand-dispatch a raw subagent inside the Cassy-provisioned worktree to keep a user-facing release moving.

## Repro sketch
1. Spawn workers, let them finish, `shutdown_workers count=0`.
2. `spawn_workers count=1 isolate=true ...` in the same session.
3. Observe worktree created, no process, no registration.

## Asks
- Make spawn-after-shutdown-all work (clear the directive once satisfied, or scope it to workers alive at issue time).
- Surface launch failures to the supervisor (spawn state: queued → provisioning → launched/FAILED+reason) instead of silent half-completion.
