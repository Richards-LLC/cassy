> Migrated to GitHub Issues: [#58](https://github.com/pippenz/cas/issues/58)

# BUG: factory daemon enqueues spawn requests but never drains the queue

**Observed:** 2026-07-31, project `/home/pippenz/Woodworking`, factory session `Woodworking-silent-cheetah-22`, supervisor `clever-tiger-89`, cas 2.37.0 (4be3086 2026-07-29).

## Symptom

Supervisor issued `coordination action=spawn_workers count=1 isolate=true cli=codex model=gpt-5.6-sol effort=<medium|high> task_id=<id>` four times (request IDs 1–4). Every call returned "Queued spawn request". No worker process was ever spawned; `worker_status` shows "Workers: None active" 25+ minutes later. No worktrees or factory branches created.

## Evidence

`.cas/logs/cas-2026-07-31.log` shows each request reach
`cas::ui::factory::daemon::runtime::queue_and_events: Enqueuing spawn request: Spawn`
(16:27:59, 16:28:03, 16:28:04, 16:52:52) with **no subsequent dequeue/spawn/error log line**. The same daemon successfully PTY-spawned the supervisor at 16:22:10 (`cas_pty::pty: spawning process ... command=claude`), and TUI resize events at 16:49 show the daemon loop is alive — so the daemon is running but the spawn-queue consumer is not firing.

Possibly relevant: repo was `git init`-ed *during* the session (session started with no `.git`; supervisor initialized it and created the epic branch before the first spawn request). If the queue consumer or worktree precheck latched a "not a git repo" state at daemon startup and never re-evaluated, that would explain silent non-processing.

## Expected

Either spawn the worker (dequeue → worktree_create → PTY spawn, with log lines), or fail loudly back to the supervisor with an actionable error. Silent queue accumulation is the worst outcome — supervisor believes workers are booting.

## Second occurrence — new signal (same session, 18:17 UTC)

After a daemon restart the queue drained correctly and three workers ran an epic to completion. The supervisor then issued `shutdown_workers count=0` (shut down ALL workers). A later `spawn_workers` for a new epic enqueued and **again never drained** — same log signature, no dequeue line. This suggests `shutdown_workers count=0` may leave the factory runtime in a terminal/shutdown state that permanently stops the spawn-queue consumer, rather than just stopping current workers. If so, the "daemon predates git init" theory above may be a red herring for occurrence #1 and the real bug is a non-restartable consumer loop.

Suggested check: does the queue consumer task exit (rather than idle) when the worker set drops to zero or when a shutdown-all is processed?

## Repro sketch

1. Start factory TUI/daemon in a directory that is NOT a git repo.
2. In the supervisor session: `git init`, create epic, then `spawn_workers ... isolate=true`.
3. Observe "Queued spawn request" with no worker ever appearing and no error logged.
