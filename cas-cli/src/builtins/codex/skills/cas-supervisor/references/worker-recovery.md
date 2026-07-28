# Worker Recovery — Triage and Failure Modes

## Authoritative liveness (cas-e98e)

`worker_status`, `agent_list`, and the FACTORY pane share one dual-signal classifier:

> **Live = (Active/Idle + heartbeat &lt; 30s) OR live OS harness process for that agent.**

**Authoritative for shutdown / re-spawn decisions:**

1. **OS process** (highest) — if Grok/Claude/Codex is still running, the worker is alive even when heartbeat lagged (`[alive — heartbeat stale]` / `active,alive-heartbeat-stale`). Do **not** shut down, unregister, or re-spawn.
2. **Heartbeat freshness** — within ~30s → live. Past that with no process → not live.
3. **Supporting:** last activity / transcript age, worktree dirty, active leases, `is-wedged`.
4. **Never** act on `Workers: None active` or `Filtered stale` alone — confirm `ps`/worktree/`is-wedged` first (cas-3e56 residual: false-empty roster nearly killed mid-turn Grok). Use `gc_cleanup` to purge dead registry rows.

Prompt-queue poison remediation requires an explicit age cutoff and supervisor sign-off:
`mcp__cs__coordination action=gc_cleanup force=true older_than_secs=86400` terminally
abandons only pending prompts older than one day and preserves the rows for forensics.
Run `gc_report` first. `force=true` without `older_than_secs` retains the legacy,
destructive whole-queue clear and should not be used for targeted recovery.

## Is the worker actually dead? (cas-4513 triage)

Before you run `shutdown_workers` on a pane that *looks* broken, spend 60 seconds on triage. The supervisor TUI is not ground truth for worker liveness — the most common false-positive failure mode is a worker that's mid-way through a long tool call or showing Claude Code's Bun/React-Ink crash screen (which leaves the process alive with an unresponsive UI). Destructive recovery on a live worker rips its worktree out from under itself and turns a recoverable hang into a real crash.

**Step 1: classify.** `cas factory is-wedged <worker>` returns one of five states plus evidence and exits with a differentiated code:

| Exit | State | What it means | Recovery |
|---|---|---|---|
| 0 | `alive` | PID up, transcript fresh, no crash signature — worker is running. | Wait. |
| 1 | `wedged` | PID up, transcript fresh, Bun/React-Ink crash signature matched. | `cas factory kill` + respawn. |
| 2 | `starved` | PID up, transcript cold (>60s since last write). Likely scheduler-starved or hung on a tool call. | Wait another 2 minutes, then re-classify. |
| 3 | `dead` | PID gone, AND a second signal corroborates it (transcript stale AND worktree not recently edited). | Cleanup only — no kill needed. |
| 4 | `unverified` | PID probe says gone, but the transcript is still fresh or the worktree was recently edited — a contradiction. | **Do not treat as dead.** Run `cas factory debug <worker>` and check the worktree before doing anything destructive — this is what a stale/wrong tracked pid looks like while the real worker is still alive (cas-f781). |

The Bun/React-Ink crash signature is the visual fingerprint captured in the cas-4513 discovery note 2026-04-23 15:11 UTC: the pane fills with minified source paths like `/$bunfs/root/src/entrypoints/cli.js`, React-Ink `createElement("ink-box", ...)` enumerations, and a JS stack trace. The Bun event loop does NOT exit on unhandled rejection, so the PID stays alive and a daemon-faked heartbeat stays fresh — without the transcript grep you cannot distinguish this from a live worker mid-call.

**Why `dead` now requires two signals (cas-f781).** A live worker's tracked pid can end up pointing at the wrong process — e.g. an MCP-server child self-registering over the real `claude --agent-name <worker>` pid. When that happens, the pid probe alone reads "gone" while the real worker is still alive and writing. `is-wedged` now corroborates a pid-gone reading against transcript mtime and worktree edit recency before calling it `dead`; a single contradicted signal reports `unverified` instead. Never auto-reset a lease off `unverified` — investigate first.

**Step 2: read the transcript tail.** `cas factory debug <worker> --tail 20` prints the last N JSONL entries from `~/.claude/projects/*/<session>.jsonl` without touching the TUI. This is the canonical "what did the worker just do" signal — use it to decide whether the wedged state has salvageable in-flight work before killing.

**Step 3: recovery.** Only after `is-wedged` reports `wedged` or `dead` — never off `unverified`:

- **Wedged:** `cas factory kill <worker>` — SIGKILL (SIGTERM is observed not to exit cleanly on the Bun wedge) and reset any leased tasks (release lease + status→Open + clear assignee, same semantics as `mcp__cs__task action=reset`). Idempotent on an already-dead process. Then respawn.
- **Starved:** do not kill. Come back in 2 minutes; if it re-classifies as `wedged`, proceed to the kill path.
- **Dead:** no kill needed. The `kill` verb is still safe to run (`skipping SIGKILL` + task reset runs); or manually `mcp__cs__task action=reset id=<task>`.
- **Unverified:** do not kill and do not reset the lease. Run `cas factory debug <worker>` and inspect the worktree manually; re-run `is-wedged` once you've confirmed which process is actually the worker.

**Process resolution (cas-f781).** `cas factory kill` does not blindly trust the agent store's tracked pid — that value can be stale or wrong (the MCP-server-child self-registration bug above). Before killing, it scans the live process table for a process whose own argv (`--agent-name <worker>`) or environment (`CAS_AGENT_NAME=<worker>`, for Codex workers) identifies it as the target worker, and prefers that resolved pid over the tracked one. The summary line calls this out explicitly: `process-table scan resolved a live process for `<worker>` at pid N (agent-name match) — overriding stale tracked pid M`. The kill itself targets the process **group**, not a single pid, since workers are spawned as session leaders and may have forked children of their own.

**Lease reset only fires after confirmed death (cas-f781).** The task lease is no longer released as an unconditional side effect of running `kill`. It resets only when death is independently confirmed: the pid was already gone before the attempt, or SIGKILL was delivered and a short post-kill poll confirms the process actually died. If the kill was refused (fingerprint mismatch / no fingerprint, see below) or the process demonstrably survived the signal, the summary says `skipping lease reset for `<worker>` — worker death not confirmed` and the lease is left alone — a still-alive worker never has its task yanked out from under it.

**PID-recycling guard.** For the tracked-pid fallback path (no live process-table match), `cas factory kill` refuses to SIGKILL unless the `/proc/<pid>/stat` starttime fingerprint recorded at agent registration (cas-ea46 / cas-b157) still matches the process at that PID. On a busy host the kernel can recycle a PID between registration and kill, so without this guard we could SIGKILL an unrelated process. If the fingerprint mismatches, the summary says `pid N SKIPPED: starttime fingerprint mismatch (PID recycled). Pass --force to override.` — investigate before using `--force`. Legacy agents (registered before cas-ea46) have no fingerprint and also require `--force`. A process resolved via the live agent-name scan skips this gate — the argv/environ match is itself a direct identity proof.

**Anti-pattern:** "pane looks broken → `shutdown_workers`". That pathway has destroyed in-progress work multiple times (silent-owl-56 2026-04-23 shipped cas-4181 through what looked like a crashed pane). The `is-wedged` / `debug` / `kill` triad replaces it.

## Verify Lifecycle Notifications Before Acting

Director and task-lifecycle notifications are hints, not ground truth. A known bug (tracking pointer `cas-dbbe`) produced false `task_completed` notifications around task start, including five false completions in one session. Before closing, reassigning, respawning, or merging because of a notification:

- Run `mcp__cs__task action=show id=<task-id>` and trust the task status over the notification text.
- Check the worker branch tip or worktree commits before assuming work exists: `git -C .cas/worktrees/<worker> log --oneline -5`.
- Check liveness with `mcp__cs__coordination action=worker_status` before declaring a worker idle or dead.

## Worker Failure Recovery

Workers fail in production. These are recurring observed failure modes and their recovery procedures. Each has occurred in real factory sessions.

### Dead or Silent Worker

**Signature:** Worker stops responding to messages. No progress notes, no commits, no heartbeat updates. Task stays `in_progress` indefinitely.

**Diagnosis:**
1. Check worker status: `mcp__cs__coordination action=worker_status`
2. Look for stale heartbeat (last activity timestamp far in the past) or missing entry — but **do not treat `Workers: None active` or `Filtered stale` as death alone** (cas-3e56: live Grok workers were omitted while mid-turn; prefer `[alive — heartbeat stale]` + OS/`ps`/worktree check before re-spawn)
3. Check worker activity log: `mcp__cs__coordination action=worker_activity`

**Recovery:**
1. Check the worker's worktree for partial work: `git -C .cas/worktrees/<worker> log --oneline main..HEAD`
2. If commits exist, cherry-pick salvageable work to the base branch before cleanup
3. Release the dead worker's lease: `mcp__cs__task action=release id=<task-id>`
4. Shut down the dead worker: `mcp__cs__coordination action=shutdown_workers count=0` (then respawn the count you need)
5. Spawn a fresh worker: `mcp__cs__coordination action=spawn_workers count=1 isolate=true`
6. Reassign the task to the new worker. If partial work was cherry-picked, include that context in the assignment message so the new worker builds on it rather than redoing it.

### Injected but Unwoken Worker

**Signature:** Heartbeat is fresh, worktree is clean, and there is zero activity for 10+ minutes after a supervisor message. The prompt was injected into the worker session, but the worker did not acknowledge or act. This is most often triggered by long multi-line payloads sent to Codex workers.

**Diagnosis:**
1. Confirm a fresh heartbeat with `mcp__cs__coordination action=worker_status`
2. Confirm no work started: `git -C .cas/worktrees/<worker> status --short`
3. Check prompt delivery state:
   ```bash
   sqlite3 .cas/cas.db "SELECT id, processed_at, acked_at FROM prompt_queue WHERE target='<name>' ORDER BY id DESC LIMIT 5"
   ```
   If the latest relevant row has `processed_at` set and `acked_at` NULL, the prompt was injected but not acknowledged.

**Recovery:**
1. Ensure the work exists as an assigned task with full spec and acceptance criteria.
2. Send a short urgent wake that points only at the task:
   ```
   mcp__cs__coordination action=message target=<worker> urgent=true summary="Task <id> assigned" message="Task <id> is assigned. Run mcp__cs__task action=show id=<id>."
   ```
3. Do not kill or respawn. There is no evidence of a dead process or dirty worktree; the fix is a durable task plus a short wake.

### Pre-compaction Triage via worker_status context indicator (cas-573c)

`mcp__cs__coordination action=worker_status` now includes a `context:` line per worker:

```
  • bright-leopard-9 (heartbeat: 8s ago)
    context: approaching (~112k tk)
    session: f90d2ee1-...
```

**Bands:**

| Band | Tokens | Action |
|---|---|---|
| `ok` | < 100k | Normal — no action. |
| `approaching` | 100k–159k | Note it. Remind the worker to commit any WIP. |
| `near-limit` | ≥ 160k | Act immediately — see recovery steps below. |

**Pre-compaction recovery (context: near-limit):**
1. Send: `mcp__cs__coordination action=message target=<worker> message="Your context is near the limit. Commit any in-progress work immediately (git add / git commit), then report what you committed."`
2. Wait for the commit confirmation (watch `mcp__cs__coordination action=worker_activity`).
3. If the worker is mid-task and not responding: check the worktree manually: `git -C .cas/worktrees/<worker> log --oneline HEAD~5..HEAD`
4. Once work is committed: shut down the worker cleanly and respawn with a fresh context.

**Why the indicator may be absent:** The context line is read from the tail of the worker's session transcript. A newly spawned worker that hasn't produced an assistant message yet will show no `context:` line — this is expected. The line appears after the worker's first response.

### Garbage Output (Context Exhaustion)

**Signature:** Worker output degrades into garbled multi-language text (Russian/Chinese characters mixed with English, repeating pseudo-words like "updofficial/action/official", BPE fragment nonsense). May be followed by a generic "violates Usage Policy" API error. This is token sampling collapse from an exhausted context window, not a real policy violation.

**Triggering conditions:** Long iterative fix-test-rerun loops, heavy stack trace volume in tool results, extended sessions with rapid context churn (20+ file edits in a short window). The `context: near-limit` indicator in `worker_status` fires before this stage — if you act on `near-limit`, you typically avoid reaching the garbled-output stage.

**Recovery:**
1. **Do NOT send revision instructions.** The worker's context is poisoned — any further messages make it worse, not better.
2. Shut down the affected worker immediately. Do not attempt to salvage the session.
3. Check the worker's worktree for any commits made before degradation: `git -C .cas/worktrees/<worker> log --oneline main..HEAD`
4. Cherry-pick any good commits. Discard anything committed after degradation began (inspect diffs carefully — degraded output may have produced syntactically plausible but semantically wrong code).
5. Spawn a fresh worker with a clean context.
6. Reassign the task. If the task involves iterative test-fix loops, add guidance to the assignment: "periodically commit working state" so partial progress survives if degradation recurs.

### Verification Jail Deadlock

**Signature:** Worker reports `VERIFICATION_JAIL_BLOCKED` and cannot close tasks or use tools. The jail check fires agent-wide — one task's pending verification blocks ALL tool usage across all tasks for that worker.

**Note:** Factory workers are exempt from verification jail as of commit `bba6fbf`. If this failure mode appears, the running CAS binary is older than that fix.

**Diagnosis:**
1. Confirm the worker is actually jailed (not just reporting a stale error)
2. Check whether the running `cas` binary includes the jail exemption fix: verify the binary was rebuilt after `bba6fbf` landed

**Recovery (binary is current — exemption should apply):**
1. Rebuild CAS: `~/.cargo/bin/cargo build --release` and restart the `cas serve` process
2. Respawn workers — they will pick up the new binary

**Recovery (binary is outdated or rebuild is not feasible mid-session):**
1. Close the jailed task with an audit trail: `mcp__cs__task action=close id=<task-id> reason="Supervisor close — verification jail deadlock. Work verified at <commit-sha>. Worker jailed, CAS binary predates bba6fbf exemption fix."`
2. If `close` is also blocked, use direct sqlite as last resort:
   ```sql
   UPDATE tasks SET status='closed', pending_verification=0 WHERE id='cas-XXXX';
   UPDATE task_leases SET status='released' WHERE task_id='cas-XXXX' AND status='active';
   ```
3. After clearing the jail, message the worker that they can proceed with remaining tasks.
4. File a note on the epic that the binary needs rebuilding before the next session.

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

**If workers still wedge under these caps:** the scheduler storm is not the bottleneck. Likely candidates, in order of follow-up cost: (1) `sccache` shared across workers (cas-0bf4 Phase 2), (2) review-persona concurrency cap (cas-0bf4 Phase 3), (3) operational — spawn fewer workers.
