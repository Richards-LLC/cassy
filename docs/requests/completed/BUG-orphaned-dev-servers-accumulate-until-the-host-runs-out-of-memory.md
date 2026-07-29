# BUG: long-lived child processes (dev servers) are never reaped, accumulating until the host exhausts RAM and swap

**Filed:** 2026-07-28
**Reporter:** supervisor `wild-condor-51`, factory session `Penguinz-witty-viper-34` (host: soundwave, 62 GiB RAM)
**Severity:** High — took a 62 GiB workstation to 70% RAM and 100% swap. Second observed recurrence.

## Summary

**33 concurrent copies of the same Nuxt dev server** were found running on soundwave, all orphaned to `systemd --user`, none with a controlling terminal. Together with their children: **36 processes holding 18.4 GiB RSS**.

Host state at discovery:

```
Mem:   62Gi total   44Gi used   9.2Gi free   18Gi available     (~70%)
Swap:  15Gi total   15Gi used   329Mi free                       (exhausted)
```

Swap was completely full, which is the condition that produces the desktop stalls tracked separately on this host. `pkill -f /Petrastella/` reclaimed ~18 GiB immediately (44 GiB → 26 GiB used, 9.2 GiB → 27 GiB free).

## What was running

All 33 were the same command, differing only in age:

```
/home/pippenz/.nvm/versions/node/v22.22.2/bin/node --enable-source-maps \
    /home/pippenz/Petrastella/ozer/apps/frontend/...
```

- **PPID 3331 for every one of them** = `/usr/lib/systemd/systemd --user`. Their original parent died; the Node grandchildren survived and were reparented to systemd rather than reaped.
- **No controlling terminal** on any of them (`ps -eo tty` = `?` across the board), so nothing was watching them.
- **Ages spread 42 minutes to 2h07**, roughly evenly — i.e. a new copy appearing every few minutes and none ever exiting. This is accumulation, not a single leak.
- Individually 1.2–2.3 GiB RSS, several also holding 125–335 MB of swap each.

## Why this is filed against CAS (and the caveat)

Circumstantial but consistent:

- `/home/pippenz/Petrastella/ozer/` contains **5 CAS factory worktrees** (`.cas/worktrees/`), and `~/.cas/` holds **21 factory sockets** referencing that project. It is a heavily factory-driven repo.
- `apps/frontend/package.json` declares `"dev": "nuxt dev"` — exactly the process shape that accumulated.
- The accumulation cadence (one every few minutes, over hours) matches agent-driven task execution far better than it matches a human starting dev servers by hand. No person starts 33 dev servers in two hours.
- Prior recurrence is already recorded in this operator's notes: the same pattern previously consumed ~26 GiB, with the same remedy (`pkill -f /Petrastella/`).

**The caveat, stated because it weakens the case:** the orphaned processes' paths pointed at the **main checkout** (`/home/pippenz/Petrastella/ozer/apps/frontend`), not at any `.cas/worktrees/<worker>/` path. If workers were launching them from inside isolated worktrees, I would have expected worktree paths. That is consistent with non-isolated workers (`isolate=false`, which share the main working directory) or with a worker `cd`-ing to the main checkout, but I did not capture the parent chain before killing the processes, so I cannot prove CAS spawned them.

**What would settle it:** on the next occurrence, capture `ps -eo pid,ppid,lstart,args` for the orphans *before* killing, and correlate their start times against factory session start/end times. If they cluster at worker boot, it is CAS.

## Impact

- Workstation driven to 70% RAM and **100% swap** (329 MiB of 15 GiB remaining).
- Desktop stalls from swap-in — the host had 14 GiB of pages stranded in disk swap with nowhere to page out to.
- Recurs. This is the second observed instance; the remedy is manual each time and the operator has to notice first.

## Requested fix

1. **Reap child processes on worker/session teardown.** Whatever a worker starts should die when the worker does — process group kill, or a cgroup/scope per worker so teardown is guaranteed rather than best-effort. Orphaning to `systemd --user` means nothing will ever clean them up.
2. **If long-lived servers are legitimately needed during a task**, track them and stop them at task close, the same way worktrees are tracked and cleaned.
3. **Consider a guard**: if a worker is about to start a dev server on a port/command that already has N instances running, warn or refuse. 33 copies of one dev server is never intentional.
4. Surface it in `gc_report` if that is the natural home — orphaned worker descendants are exactly the kind of thing a GC pass should notice.

## Related

- `BUG-worker-messages-redelivered-and-supervisor-replies-never-acked.md`
- `BUG-stall-detector-false-positive-ignores-worker-tool-calls.md`
- `BUG-stock-worker-defaults-contradict-shipped-model-routing-policy.md`
- `FEATURE-code-review-personas-off-claude-only-model-enum.md`

## Resolution (2026-07-29)

Factory worker panes already started as `setsid(2)` session/process-group
leaders, but CAS did not persist those PGIDs and factory exit killed only the
direct pane children. CAS now records each worker's PGID, factory session, and
Linux process-start fingerprint under `.cas/factory-process-groups/` at spawn
and respawn. Worker shutdown, crash/rotation, factory exit, and worker-lane
worktree cleanup reap the whole fingerprint-validated group. Factory exit now
uses tree kill for every PTY rather than direct-child kill.

`gc_report` surfaces aged, still-live process groups whose factory session or
worker lane is dead. `gc_cleanup force=true` revalidates the process-start
fingerprint immediately before `killpg(2)` and reclaims only matching groups;
dead or recycled records are removed without signaling the recycled process.
Regression tests use synthetic shell children, including a long-lived child
whose original process-group leader has already exited. No real dev server is
started by the test suite.
