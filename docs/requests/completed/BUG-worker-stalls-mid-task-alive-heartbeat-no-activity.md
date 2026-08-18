# BUG: Factory worker stalls mid-task — heartbeat alive, zero activity, no work produced, and no signal to the supervisor

## Resolution

Fixed in `cas-9829`. Heartbeat and activity are already two distinct signals in the director's `AgentSummary` (`latest_activity` vs `last_heartbeat`), but nothing turned "alive + assigned + in_progress + no activity for N minutes" into a signal — this closes that gap.

**Detection** (`cas-cli/src/ui/factory/director/events.rs`): `DirectorEventDetector::detect_changes_at`, inside the existing per-agent loop's "has a current task" branch, now checks a fresh heartbeat (`FRESH_HEARTBEAT_SECS`, same gate `WorkerIdle` uses) together with `agent.latest_activity` age against a new configurable `stall_threshold_secs`. A new `DirectorEvent::WorkerStalled { worker, task_id, elapsed_secs, escalate }` fires:
- `escalate: false` on first detection in a stall streak — a one-shot auto-nudge.
- `escalate: true` if the worker is *still* stalled on the next detection after the nudge — escalates to the supervisor.
- The streak (`stall_nudged`/`stall_escalated` sets on the detector) clears once activity resumes, so a fresh stall re-nudges instead of staying silently suppressed.

**Prompt routing** (`cas-cli/src/ui/factory/director/prompts.rs`): the non-escalating nudge targets the worker directly (re-injects guidance to post a progress/blocker note or close the task); the escalation targets the supervisor with the worker/task/elapsed time and a `worker_status` pointer. Gated by a new `AutoPromptConfig.on_worker_stalled` flag (default `true`), mirroring the other event toggles. Desktop notifications (`notification.rs`) only fire on escalation, to avoid alert fatigue on the low-stakes first nudge.

**Config** (`cas-cli/src/config/settings.rs` `FactoryConfig` + `crates/cas-factory/src/config.rs`): `[factory] stall_threshold_secs` in `.cas/config.toml`, default `cas_factory::DEFAULT_STALL_THRESHOLD_SECS` = 300s (5 minutes). Threaded from `Config::load` through `cli/factory/mod.rs` / `cli/factory/daemon.rs` into `cas_factory::FactoryConfig`, and from there (or from a fresh `Config::load` in the fork-first daemon init path) into `DirectorEventDetector::set_stall_threshold_secs`.

**`worker_status` render** (`cas-cli/src/mcp/tools/service/factory_ops.rs`): a worker holding a task lease whose last observable activity is at/past `stall_threshold_secs` (or has no activity at all in the query window) now renders `⚠ STALLED` instead of the soft "may be investigating or idle" hedge that made the original bug easy to skim past. New pure helper `is_worker_stalled` backs both the render and its unit tests; the config lookup and lease check reuse existing MCP-tool plumbing (`Config::factory()`, `AgentStore::list_agent_leases`).

**Tests**: `events_tests/tests.rs` (nudge-then-escalate, streak reset on resumed activity, fresh-heartbeat gate, configurable threshold), `prompts.rs` (nudge targets worker, escalation targets supervisor, config toggle, unknown-worker guard), `notification.rs` (notification only on escalation), `factory_ops.rs` (`is_worker_stalled` unit tests), `config/settings.rs` (TOML round-trip + override), `factory_mcp_ops_test.rs` (end-to-end `worker_status` render). `cargo test --no-fail-fast` exit 0.

**Not implemented** (left for a follow-on if needed): a periodic *unprompted* re-check independent of the director's normal 2s refresh tick — the fix rides the existing `detect_changes_at` cadence, which is sufficient since the daemon already ticks continuously while a worker is registered.

---

**Filed by:** supervisor (silent-dragon-3), gabber-studio factory session
**Date:** 2026-07-02
**Severity:** Medium-High — silent stalls waste wall-clock and only surface if the supervisor manually polls; the worker looks healthy the whole time.
**CLI/model in play:** worker spawned with `cli=codex`, `model=gpt-5.5`, `isolate=true`.

---

## Summary

Worker `lively-crow-97` ACK'd task cas-0b7d with a detailed plan, then **stopped**. It kept heartbeating (liveness green, ~6s ago) but produced **no activity for 10+ minutes**, made **no file changes**, and **never synced its factory branch to the epic tip** it needs for the task. `worker_status` even annotates it: `last activity: none in last 10m (may be investigating or idle)`.

Nothing in the system flagged this as a problem — the worker is "alive," so the director's idle/blocked nudges didn't fire. The only way I found it was manually running `worker_status` and noticing the stale-activity line + unchanged worktree HEAD.

## Evidence

`worker_status` for the stalled worker:

```
• lively-crow-97 (heartbeat: 6s ago)
    git: factory/lively-crow-97 @ 07a4f2f5f [clean] [pushed]
    last activity: none in last 10m (may be investigating or idle)
    session: codex-lively-crow-97-...
```

- Worktree HEAD `07a4f2f5f` is the PREVIOUS task's commit (cas-26c8). For cas-0b7d it was told (and ACK'd) to first sync to the epic tip (which carries the cas-6fe4 backend contract) — it never did.
- Tree is `[clean]` — zero uncommitted work, so this isn't "deep in a long edit." It simply stopped after printing a plan.
- Heartbeat stayed fresh throughout, so every liveness signal said "healthy."

## Why this is a bug (gap, not just model flakiness)

The underlying "Codex worker prints a plan then goes quiet" is a model/CLI reliability issue. But the **Cassy-side gap** is what makes it costly:

1. **Heartbeat ≠ progress.** Liveness is measured by heartbeat, which keeps ticking even when the agent has produced no tokens/tool-calls/file-changes for many minutes. A worker mid-task with a fresh heartbeat but a flat activity timeline is indistinguishable from a healthy one to every automated consumer.
2. **No stall detection / no supervisor alert.** The director notifies on `worker_idle` / `worker_blocked` / `worker_died`, but there is no `worker_stalled` signal for "alive + assigned + in_progress + no activity for N minutes." That's exactly the state that most needs a nudge, and it's the one state that stays silent.
3. **`worker_status` knows but doesn't escalate.** The tool already computes and prints `none in last Xm` — the information exists; it just isn't turned into an event the supervisor receives. A supervisor who doesn't happen to poll never sees it.

## Suggested fixes

- Track a **last-activity timestamp** (last tool call / file write / token output) separately from heartbeat, and add a `worker_stalled` event that fires when `now - last_activity > threshold` while the worker is alive AND has an in_progress task. Route it to the supervisor like the other director notifications.
- Make the threshold configurable (e.g. `[factory] stall_threshold_secs`), defaulting to a few minutes.
- Optionally, auto-nudge once on stall (re-inject the current task prompt) before escalating to the supervisor, since a single re-poke often unsticks these.
- In `worker_status`, when `last activity: none in last Xm`, mark the row visually (⚠ STALLED) rather than the soft "may be investigating or idle" — that hedge reads as fine and is easy to skim past.

## Workaround used this session

Manual `worker_status` poll → spotted the stale-activity line + unchanged HEAD → re-poked the worker with an urgent re-dispatch of cas-0b7d (sync-to-epic-tip + implement). If it doesn't resume, the fallback is shutdown + respawn (losing nothing, since the tree was clean).

## Related

- Pairs with `BUG-worker-close-with-empty-worktree-phantom-close.md` (same session): both are "the worker looks done/healthy but produced nothing," and both are only catchable by a supervisor manually diffing/polling. Together they argue for **activity-based** (not attempt/heartbeat-based) truth signals in the factory.
