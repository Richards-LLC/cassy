# Account usage-limit notice

- Reader: the factory supervisor coordinating a worker whose harness exhausted its account.
- Decision: leave the worker waiting on its current account until its reported reset; an unavailable reset time is explicit.
- Surface: one durable supervisor attention message per transcript limit episode, naming worker, current account, held task IDs and reset windows.
- Machine output: migration 287 keeps its episode/detail/optional historical request ID schema. New episodes have no request ID and enqueue no recycle. Legacy queued `limit_recycle` recipes are refused before shutdown.
- Constraints: plain text without ANSI; full local date, 12-hour time and timezone for every known reset, alongside the existing relative window summary. No account selection, config directory rewrite, transcript copying or automatic respawn after a limit.

## Recovery contract

The worker's harness stops work at the provider limit and waits. Cassy records the
limit episode and reports it; worktree, branch and task bindings remain in place.
The key contains factory session, worker name and first transcript evidence, so
registration changes and daemon restarts do not repeat a supervisor notice.
Historical episode records remain readable. A pre-upgrade queued automatic switch
ends failed without stopping the current worker.

The notice reads only the current account's usage. It never recommends an alternate
account. Picker recommendations and `spawn_workers` choosing a recommended account
for a NEW spawn without an explicit `config_dir` remain unchanged: the operator
can still select an account at launch.

## Evidence

The daemon regression exercises actual Codex rollout reset data, both providers,
duplicate observations, daemon restarts, supervisor recovery guidance and refusal
of a persisted legacy switch. Spawn store coverage proves durable recording without
queueing, including historical records. Migration 287 retains its upgrade test.
Real subscription exhaustion is NOT EXERCISED; no live account is driven to its limit.
