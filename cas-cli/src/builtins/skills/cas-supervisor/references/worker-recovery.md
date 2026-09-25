# Worker Recovery — Triage and Failure Modes

Contents: [Authoritative liveness](#authoritative-liveness) · [Is the worker actually dead?](#is-the-worker-actually-dead) · [Verify lifecycle notifications](#verify-lifecycle-notifications-before-acting) · [Failure modes](#worker-failure-recovery): silent worker, injected but unwoken, stalled spawn queue, context pressure, garbage output, resource contention.

## Authoritative liveness

`worker_status`, `agent_list`, and the FACTORY pane share one dual-signal classifier:

> **Live = (Active/Idle + heartbeat &lt; 30s) OR live OS harness process for that agent.**

**Authoritative for shutdown / re-spawn decisions:**

1. **OS process** (highest) — if Grok/Claude/Codex is still running, the worker is alive even when heartbeat lagged (`[alive — heartbeat stale]` / `active,alive-heartbeat-stale`). Do **not** shut down, unregister, or re-spawn.
2. **Heartbeat freshness** — within ~30s → live. Past that with no process → not live.
3. **Supporting:** last activity / transcript age, worktree dirty, active leases, `is-wedged`.
4. **Never** act on `Workers: None active` or `Filtered stale` alone — a false-empty roster can hide a live worker mid-turn; confirm `ps`/worktree/`is-wedged` first. Use `gc_cleanup` to purge dead registry rows.

Prompt-queue poison remediation requires an explicit age cutoff and supervisor sign-off:
`mcp__cas__coordination action=gc_cleanup force=true older_than_secs=86400` terminally
abandons only pending prompts older than one day and preserves the rows for forensics.
Run `gc_report` first. `force=true` without `older_than_secs` retains the legacy,
destructive whole-queue clear and should not be used for targeted recovery.

## Is the worker actually dead?

Before you shut down a pane that *looks* broken, spend 60 seconds on triage. The supervisor TUI is not ground truth for worker liveness — the most common false positive is a worker mid-way through a long tool call or showing Claude Code's Bun/React-Ink crash screen (process alive, UI unresponsive). Destructive recovery on a live worker rips its worktree out from under itself and turns a recoverable hang into a real crash.

**Step 1: classify.** `cas factory is-wedged <worker>` returns one of six states plus evidence and exits with a differentiated code:

| Exit | State | What it means | Recovery |
|---|---|---|---|
| 0 | `alive` | PID up, transcript fresh, no crash signature — worker is running. | Wait. |
| 1 | `wedged` | PID up, transcript fresh, Bun/React-Ink crash signature matched. | `cas factory kill` + respawn. |
| 2 | `starved` | PID up, transcript cold (>60s since last write). Likely scheduler-starved or hung on a tool call. | Wait another 2 minutes, then re-classify. |
| 3 | `dead` | PID gone, AND a second signal corroborates it (transcript stale AND worktree not recently edited). | Cleanup only — no kill needed. |
| 4 | `unverified` | PID probe says gone, but the transcript is still fresh or the worktree was recently edited — a contradiction. | **Do not treat as dead.** Run `cas factory debug <worker>` and check the worktree before doing anything destructive; this is what a stale or wrong tracked pid looks like while the real worker is alive. |
| 5 | `approval-hang` | A permission request is parked for a team lead and no child process is running. | Answer it: `cas factory approve <worker>` or `cas factory deny <worker> --reason "<why>"` (`--request <perm-id>` names one request). |

The crash signature is the pane filling with minified paths like `/$bunfs/root/src/entrypoints/cli.js`, React-Ink `createElement("ink-box", ...)` enumerations and a JS stack trace; the PID and heartbeat stay alive, so only the transcript grep tells it apart from a live worker mid-call.

**Step 2: read the transcript tail.** `cas factory debug <worker> --tail 20` prints the last N JSONL entries from `~/.claude/projects/*/<session>.jsonl` without touching the TUI. That path follows the **worker's** config dir, not yours: a worker spawned with `config_dir=~/.claude-alt` writes its transcript under `~/.claude-alt/projects/*/` instead. If the transcript looks missing, check which config dir the worker was spawned into before concluding it never started. This is the canonical "what did the worker just do" signal — use it to decide whether the wedged state has salvageable in-flight work before killing.

**Step 3: recovery.** Only after `is-wedged` reports `wedged` or `dead` — never off `unverified`:

- **Wedged:** `cas factory kill <worker>`, then respawn. It SIGKILLs the worker's process group and resets its leased tasks (same semantics as `mcp__cas__task action=reset`) only once death is confirmed; `cas factory kill --help` documents process resolution and the PID-recycling guard. Investigate before passing `--force`.
- **Starved:** do not kill. Come back in 2 minutes; if it re-classifies as `wedged`, proceed to the kill path.
- **Dead:** no kill needed. The `kill` verb is still safe to run (`skipping SIGKILL` + task reset runs); or manually `mcp__cas__task action=reset id=<task-id>`.
- **Unverified:** do not kill and do not reset the lease. Run `cas factory debug <worker>` and inspect the worktree manually; re-run `is-wedged` once you've confirmed which process is actually the worker.

**Anti-pattern:** "pane looks broken → `shutdown_workers`". That pathway has destroyed in-progress work; the `is-wedged` / `debug` / `kill` triad replaces it.

## Verify Lifecycle Notifications Before Acting

Director and task-lifecycle notifications are hints, not ground truth. A known bug (tracking pointer `cas-dbbe`) produced false `task_completed` notifications around task start, including five false completions in one session. Before closing, reassigning, respawning, or merging because of a notification:

- Run `mcp__cas__task action=show id=<task-id>` and trust the task status over the notification text.
- Check the worker branch tip or worktree commits before assuming work exists: `git -C .cas/worktrees/<worker> log --oneline -5`.
- Check liveness with `mcp__cas__coordination action=worker_status` before declaring a worker idle or dead.

## Worker Failure Recovery

Recurring failure modes and their recovery procedures.

### Silent Worker

**Signature:** Worker stops responding to messages. No progress notes, no commits, no heartbeat updates. Task stays `in_progress` indefinitely.

Run `mcp__cas__coordination action=worker_status`, then the `is-wedged` / `debug` / `kill` triad above. Salvage committed work with `mcp__cas__coordination action=worktree_merge id=<worker> task_id=<task-id>` before any cleanup, and message the replacement worker what already landed.

### Injected but Unwoken Worker

**Signature:** Heartbeat is fresh, worktree is clean, and there is zero activity for 10+ minutes after a supervisor message. Delivery and acceptance are separate: inspect transport receipts, task state, and execution evidence before concluding the worker is stuck.

**Diagnosis:**
1. Confirm a fresh heartbeat with `mcp__cas__coordination action=worker_status`
2. Read `mcp__cas__task action=show id=<task-id>`: a successful start is authoritative assignment acceptance. A clean `git -C .cas/worktrees/<worker> status --short` alone does not prove inactivity.
3. Check prompt delivery state with `mcp__cas__coordination action=message_status notification_id=<id>` (the id the `message` call returned). It reports transport handoff, wake observations and confirmation separately; the queue columns behind it are `processed_at, acked_at`. A set `processed_at` records transport processing; `acked_at` records queue acknowledgement. Neither is assignment acceptance or execution proof. Use `queue_ack` for durable supervisor notifications and `message_ack` for prompt-message receipts; lifecycle relay acknowledgements reconcile linked rows. Neither replaces `task action=start`. Missing prose ACK alone is not a recovery trigger.

**Recovery:**
1. Ensure the work exists as an assigned task with full spec and acceptance criteria.
2. Send a short urgent wake that points only at the task:
   ```
   mcp__cas__coordination action=message target=<worker> urgent=true summary="Task <id> assigned" message="Task <id> is assigned. Run mcp__cas__task action=show id=<id>."
   ```
3. Do not kill or respawn. There is no evidence of a dead process or dirty worktree; the fix is a durable task plus a short wake.

### Stalled Spawn Queue

If `worker_status` shows `SPAWN QUEUE STALLED`, `FACTORY DAEMON LOOP WEDGED`,
or `SPAWN IN FLIGHT FOR`, the factory daemon has stopped processing spawn and
shutdown requests. Run `mcp__cas__coordination action=restart_spawn_queue`. It
keeps the session and every pane. The daemon abandons its in-flight spawn,
drops dequeued actions that have not run, and messages you what to re-issue.
If the loop itself is wedged, its watchdog first kills hung git, gh or ssh
helper processes. Check `worker_status` again. If the loop is still wedged,
file a CAS bug quoting the phase and wait channel that `worker_status` names.

### Context pressure (worker_status `context:` line)

`mcp__cas__coordination action=worker_status` prints a `context:` line per worker, banded by the share of the model's context window in use:

```
  • bright-leopard-9 (heartbeat: 8s ago)
    context: approaching (~112k / 200k tk; ~44% headroom)
```

| Band | Window used | Action |
|---|---|---|
| `ok` | < 50% | Normal — no action. |
| `approaching` | 50–79% | Note it. Remind the worker to commit any WIP. |
| `near-limit` | ≥ 80% | Act immediately — see recovery steps below. |

An idle Codex worker past the recycle threshold also gets a `RECYCLE RECOMMENDED` line naming the command.

**Pre-compaction recovery (context: near-limit):**
1. Send: `mcp__cas__coordination action=message target=<worker> summary="Context near limit — commit now" message="Your context is near the limit. Commit any in-progress work immediately (git add / git commit), then report what you committed."`
2. Wait for the commit confirmation (watch `mcp__cas__coordination action=worker_activity`).
3. If the worker is mid-task and not responding: check the worktree manually: `git -C .cas/worktrees/<worker> log --oneline HEAD~5..HEAD`
4. Once work is committed and the worker is idle: `mcp__cas__coordination action=recycle_worker target=<worker>`. It restarts the same name with its recorded recipe and keeps the worktree.

**Why the indicator may be absent:** The context line is read from the tail of the worker's session transcript. A newly spawned worker that hasn't produced an assistant message yet will show no `context:` line — this is expected. The line appears after the worker's first response.

### Garbage Output (Context Exhaustion)

**Signature:** Worker output degrades into garbled multi-language text (Russian/Chinese characters mixed with English, repeating pseudo-words like "updofficial/action/official", BPE fragment nonsense). May be followed by a generic "violates Usage Policy" API error. This is token sampling collapse from an exhausted context window, not a real policy violation.

**Triggering conditions:** Long iterative fix-test-rerun loops, heavy stack trace volume in tool results, extended sessions with rapid context churn (20+ file edits in a short window). The `context: near-limit` indicator in `worker_status` fires before this stage — if you act on `near-limit`, you typically avoid reaching the garbled-output stage.

**Recovery:**
1. Do not send revision instructions. The worker's context is poisoned — any further messages make it worse, not better.
2. Shut down the affected worker immediately: `mcp__cas__coordination action=shutdown_workers worker_names=<worker>`. Do not attempt to salvage the session.
3. Check the worker's worktree for any commits made before degradation: `git -C .cas/worktrees/<worker> log --oneline <epic-branch>..HEAD`
4. Inspect those diffs carefully — degraded output may be syntactically plausible but semantically wrong. Record which commits are good in a task note.
5. Free the task with `mcp__cas__task action=reset id=<task-id>`, then spawn a fresh worker on it (`task_id=<task-id>`); point it at the good commits in the assignment.
6. If the task involves iterative test-fix loops, add guidance to the assignment: "periodically commit working state" so partial progress survives if degradation recurs.

### Resource-Contention Worker Crashes (cas-0bf4)

**Signature:** Multiple workers wedge around the same time in the Claude Code JS crash-screen state (Bun/React Ink render exception). Host shows `uptime` load avg well above CPU count (5-min avg > 1.0 × num_cpus on a 16-thread box = saturated). Memory is NOT under pressure — this is CPU scheduler starvation, not OOM.

**Root cause:** Each worker's `cargo` builds a per-worktree `target/` with rustc fanning out to `num_cpus` parallel jobs. 4 workers × 16 rustc threads × an autofix pass = scheduler storm → Claude Code event loop starves → Ink render exception → worker wedged in crash-screen state. See task `cas-0bf4` and discovery in `cas-4513`.

**Built-in mitigation (on by default):** Factory mode exports `CARGO_BUILD_JOBS` into each worker's env at spawn and wraps the worker command with `nice -n 10` so cargo runs at a lower priority than the supervisor. Controlled by two config knobs in `.cas/config.toml`:

```toml
[factory]
# Cap on CARGO_BUILD_JOBS exported into workers.
# "auto" (default) = max(2, num_cpus / 4).
# Any numeric string like "4" is exported verbatim.
cargo_build_jobs = "auto"

# When true, prefix each worker spawn with `nice -n 10`.
# Default true. Flip false for single-worker or benchmarking.
nice_cargo = true
```

Shell-level overrides (win over config): `CAS_FACTORY_CARGO_BUILD_JOBS=<N>`, `CAS_FACTORY_NICE_WORKER=1`, `CAS_FACTORY_NICE_LEVEL=<N>`.

**When the defaults are wrong:**
- Running more than 4 workers on a 16-thread host → set `cargo_build_jobs = "2"` (÷4 assumption no longer holds).
- Host has 4–8 cores → the auto-cap floors at 2, which is still `workers × 2` rustc threads; on a 4-worker factory with 4 cores consider `cargo_build_jobs = "1"` manually.
- Host has 32+ threads → `"auto"` is fine; can push higher if wall-time matters.
- CPU-bound but not crashing → flip `nice_cargo = false` to let workers and supervisor compete on equal terms.

**Repro runbook (for verifying the cap works on a given host):** spawn 4 workers on this repo, trigger simultaneous cargo builds in all of them (`cargo test` in each worktree), watch `uptime` over 60 s. 5-min load avg should stay below CPU count. If it still saturates, drop `cargo_build_jobs` one step (e.g. `"auto"` → `"2"`) and re-check.

**If workers still wedge under these caps:** the scheduler storm is not the bottleneck. Likely candidates, in order of follow-up cost: (1) `sccache` shared across workers (cas-0bf4 Phase 2), (2) subagent concurrency cap (cas-0bf4 Phase 3), (3) operational — spawn fewer workers.
