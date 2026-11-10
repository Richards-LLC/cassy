# Worker Recovery — Triage and Failure Modes

Contents: [Authoritative liveness](#authoritative-liveness) · [Is the worker actually dead?](#is-the-worker-actually-dead) · [Verify lifecycle notifications](#verify-lifecycle-notifications-before-acting) · [Retry policy](#retry-policy-by-failure-mode) · [Failure modes](#worker-failure-recovery)

## Authoritative liveness

`worker_status`, `agent_list` and the FACTORY pane share one classifier and print its evidence. A live OS harness process outranks a stale heartbeat: do not shut down, unregister or respawn that worker. Never act on `Workers: None active` or `Filtered stale` alone; a false-empty roster can hide a live worker mid-turn. Purge dead registry rows with `gc_cleanup`.

For a poisoned prompt queue, run `gc_report` first, then `factory action=gc_cleanup force=true older_than_secs=86400`, which abandons only pending prompts older than a day. Never pass `force=true` without `older_than_secs`: that clears the whole queue.

## Supervisor restart

When your pane exits on its own (`/exit`, a crash, a kill), the factory shuts down and every worker pane stops with it. Only a requested restart spares them. Tell the operator this plainly before recommending a restart.

- **New `cas serve` binary:** ask the operator to use the harness's MCP reconnect control (Claude Code: `/mcp`). Your pane and the workers keep running; use `recycle_worker` for stale workers.
- **A new harness version or a fresh context:** record a handoff with `memory action=remember entry_type=handoff`, then run `factory action=restart_supervisor`. The daemon starts a new session with the factory's boot harness, model and effort; workers keep their tasks, panes and leases, and a `/model` change does not carry over. If your session still runs after 10 seconds, the restart did not happen: a refusal arrives as a message, and silence means the daemon predates the action.
- **A crash, or a daemon without `restart_supervisor`:** treat it as a factory restart. Have each worker commit and push or park and write `HANDOFF.md` under each task's artifacts directory, record your handoff, then after the restart and preflight respawn the workers by name on their tasks; a respawn by name reuses the kept worktree.

## Disposable verification and walk checkouts

Create review scratch with `cas factory scratch-checkout --cas-root <root> --task-id <task-id> --head <full SHA>` (add `--dispatch-id` for a dispatch) and use the printed path, not an ad-hoc clone. Run named Rust tests with `cas factory verifier-test` and the same owner arguments. `gc_cleanup force=true dry_run=false` removes idle checkouts two hours after their task or dispatch ends; live leases and process users keep theirs.

## Is the worker actually dead?

The supervisor TUI is not ground truth. The usual false positive is a worker mid-way through a long tool call, or showing Claude Code's Bun/React-Ink crash screen while its process lives. Destructive recovery on a live worker rips its worktree out from under it.

1. **Classify:** `cas factory is-wedged <worker>` prints a state and its evidence.

   | State | Recovery |
   |---|---|
   | `alive` | Wait. |
   | `wedged` (crash signature, process up) | `cas factory kill <worker>`, then respawn. |
   | `starved` (process up, transcript cold) | Wait on an event reminder, never a timed poll. Classify again on the next `worker_idle` or task event. |
   | `dead` (process gone, corroborated) | No kill needed; `task action=reset id=<task-id>` or `cas factory kill`. |
   | `unverified` (signals contradict) | **Do not treat as dead.** Run `cas factory debug <worker>` and inspect the worktree; do not kill or reset the lease. |
   | `approval-hang` | `cas factory approve <worker>` or `cas factory deny <worker> --reason "<why>"`. |

2. **Read the transcript tail:** `cas factory debug <worker> --tail 20`. It reads the worker's own config dir, so a worker spawned with `config_dir=~/.claude-alt` writes under that directory. Use it to decide whether in-flight work is salvageable before killing.
3. **Recover** only on `wedged` or `dead`. `cas factory kill` resets the leased tasks only once death is confirmed; investigate before passing `--force`.

Never answer "pane looks broken" with `shutdown_workers`; that has destroyed in-progress work.

## Verify Lifecycle Notifications Before Acting

Director and lifecycle notifications are hints. Before closing, reassigning, respawning or merging on one, read `task action=show id=<task-id>`, check the worker's commits (`git -C .cas/worktrees/<worker> log --oneline -5`) and check liveness with `factory action=worker_status`.

## Retry policy by failure mode

Classify the failure before you retry. The same brief on the same cause fails the same way, so the failure mode decides what changes.

| Failure mode | Signature | Retry |
|---|---|---|
| Harness crash or transient error | `wedged` or `dead`; garbage output; a transient API or network error | Respawn with the same brief and point the new worker at what landed. |
| Capacity | Rate limit, usage cap, model unavailable | Retry after the reset, or use the lane's fallback recipe ([model-selection.md](model-selection.md)). |
| Scope too large | Context ran out; out-of-memory | Split the task and give a fresh worker the smaller brief. |
| Model tool errors | The worker keeps mis-calling or looping on a tool | Respawn the same brief on a different model. |
| Wrong approach | Wrong problem solved, design rejected, brief misread | Rewrite the brief to name what was wrong; use a fresh worker. |
| Environment or tooling | Missing binary, denied path, broken worktree, stale Cassy binary, CI infrastructure | Fix the environment first; no retry fixes it. |
| Same failure twice | A retry fails the same way | Stop. Write down the premise you retried on and escalate, or file the CAS bug ([filing-cas-bugs.md](filing-cas-bugs.md)). |

Cap it at two retries per task, then escalate. Before reassigning a dead worker's task, write the report it never sent:
`task action=notes id=<task-id> note_type=blocker notes="BLOCKED <tip sha or none>: worker died (<failure mode>); landed: <commits or nothing>"`.

## Worker Failure Recovery

### Silent Worker

No messages, notes, commits or heartbeat; the task stays `in_progress`. Run `factory action=worker_status`, then the triage above. Salvage committed work with `factory action=worktree_merge id=<worker> task_id=<task-id>` before any cleanup.

### Injected but Unwoken Worker

Fresh heartbeat, clean worktree, no activity for 10+ minutes after a message. Delivery and acceptance are separate:

1. Read `task action=show id=<task-id>`: a successful `task action=start` is acceptance. A clean worktree alone does not prove inactivity.
2. Read `coordination action=message_status notification_id=<id>`. It reports transport handoff, wake observations and confirmation separately; its queue columns are `processed_at, acked_at`, and neither proves acceptance or execution. `queue_ack` acknowledges durable supervisor notifications and `message_ack` prompt-message receipts; neither replaces `task action=start`. A missing prose ACK is not a recovery trigger.

Recovery: make sure the work is an assigned task with full acceptance criteria, then send a short urgent wake that points at it:

```text
coordination action=message target=<worker> urgent=true summary="Task <id> assigned" message="Task <id> is assigned. Run task action=show id=<id>."
```

Do not kill or respawn: there is no evidence of a dead process or dirty worktree.

### Stalled Spawn Queue

On `SPAWN QUEUE STALLED`, `FACTORY DAEMON LOOP WEDGED` or `SPAWN IN FLIGHT FOR`, run `factory action=restart_spawn_queue`. It keeps the session and every pane and messages you what to re-issue. If `worker_status` still shows the loop wedged, file a CAS bug quoting the phase and wait channel it names.

### Context pressure

`worker_status` prints a `context:` band per worker once it has answered at least once. At `approaching`, remind the worker to commit WIP. At `near-limit`, act now:

1. `coordination action=message target=<worker> summary="Context near limit — commit now" message="Commit any in-progress work now, then report what you committed."`
2. Wait for the commit on a `task_*` or `worker_idle` event reminder, or check `git -C .cas/worktrees/<worker> log --oneline HEAD~5..HEAD`.
3. Once work is committed, `HANDOFF.md` written under the task's artifacts directory and the worker idle, run `factory action=recycle_worker target=<worker>`; it keeps the name, recipe and worktree. `RECYCLE RECOMMENDED` names the same command for an idle Codex worker.

### Garbage Output (Context Exhaustion)

Output degrading into mixed-language or pseudo-word text, sometimes followed by a "violates Usage Policy" error, is sampling collapse from an exhausted context, not a policy violation. Acting on `near-limit` usually prevents it.

1. Send no revision instructions; more messages make it worse.
2. Shut the worker down: `factory action=shutdown_workers worker_names=<worker>`.
3. List its commits (`git -C .cas/worktrees/<worker> log --oneline <epic-branch>..HEAD`) and inspect them closely: degraded output can look plausible and be wrong. Record the good commits in a task note.
4. `task action=reset id=<task-id>`, spawn a fresh worker with `task_id=<task-id>`, point it at the good commits, and ask it to commit working state periodically.

### Resource-Contention Worker Crashes

Several workers wedging together in the crash-screen state while the host's 5-minute load exceeds its CPU count is scheduler starvation from parallel `cargo` builds, not memory pressure. Factory mode caps each worker's `CARGO_BUILD_JOBS` and runs it under `nice`; the spawn receipt shows both. Tune `[factory] worker_build_jobs` (default `auto`) and `nice_cargo` in `.cas/config.toml`: lower the job cap when running more workers than the host has cores for. If workers still wedge under the caps, spawn fewer workers.
