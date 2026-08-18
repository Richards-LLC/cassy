---
date: 2026-07-21
author: factory supervisor (happy-newt-28)
status: bug
---

# BUG: `cas factory is-wedged` resolves the wrong factory session → false "starved" verdicts on healthy workers

## Summary

`cas factory is-wedged <worker>` (and `cas factory status` / `cas factory debug`)
resolved a DIFFERENT factory session than the one the worker belongs to, producing
`transcript: <unresolved>` and a **starved** classification for a worker that was
actively working and posting task notes at that very moment. The same bad signal
drove the director's stall alerts ("alive heartbeat, no activity, auto-nudge did
not unstick"), and led the supervisor to fire an urgent interrupt that discarded a
healthy worker's in-flight turn.

## Environment

- Multiple factory sessions exist for the same project directory. The active one
  (supervisor + 2 codex workers) was session A; a stale/other session B also
  exists for the same project path.
- Worker: codex CLI worker, registered and heartbeating in session A, mid-task
  with fresh task notes every 2-5 minutes.

## Repro / evidence

1. Worker posts task progress notes at T+0, T+3, T+5 (visible via
   `task action=show`).
2. Director emits a WorkerIdle stall alert for the worker at ~T+5 ("no activity
   5m, auto-nudge did not unstick").
3. `cas factory is-wedged <worker>` returns:
   - `state: starved`
   - `pid: <n> (alive: true)`
   - `transcript: <unresolved>`
   - `transcript mtime age: <unknown>`
   - `worktree recent-edit age: <unknown>`
4. `cas factory debug <worker>` errors: "no transcript found for worker ... Try
   `cas factory status`".
5. `cas factory status` prints a session banner for a DIFFERENT session id than
   the one the worker/supervisor are registered in (session B, "Agents: 0"),
   while the worker's own MCP-side heartbeat/notes prove liveness in session A.

## Expected

- `is-wedged` should resolve the worker's own session (or accept a
  `--session` arg), find its transcript, and classify from real evidence.
- If the transcript CANNOT be resolved, the verdict should be **unknown /
  no-evidence**, NOT "starved" — an unresolved transcript is a measurement
  failure, not worker inactivity. "Starved" invites the operator to interrupt
  or kill a worker that may be mid-turn.
- The director's stall monitor should not emit "no activity" while the worker
  is writing task notes through the same Cassy instance.

## Impact

- False stall alerts spam the supervisor for every long-running healthy worker.
- Supervisors acting on the "starved" verdict interrupt (turn loss, as here) or
  kill (full context loss) healthy workers. The tool's own guidance ("only kill
  if is-wedged reports Wedged or Dead") is undermined when the classifier
  fabricates verdicts from missing data.

## Suggested fix

1. Session resolution: key the transcript lookup off the worker's agent record
   (session id is in the agent store) instead of a cwd-based "current session"
   guess that can land on a sibling session for the same project.
2. Classification honesty: when transcript/worktree evidence is `<unresolved>` /
   `<unknown>`, return `state: unknown (no evidence)` and say explicitly that
   interrupt/kill decisions must not be made from this output.
3. Stall monitor: incorporate task-note recency (already in the task store) as
   an activity signal before emitting WorkerIdle.

## Completion

- **completed:** 2026-07-22
- **resolution:** Items 1–2 (wrong-session resolution, starved-with-no-evidence) were already fixed the morning this report landed — cas-c655/cas-de95/cas-900b (epic cas-887b, on main since v2.28.0): session resolution keys off the worker's own agent record and an unresolved transcript now classifies as `unverified`, never `starved`. Item 3 (task-note recency as an activity signal) shipped in v2.28.1: `task action=notes` now emits a `TaskNoteAdded` event and the director stall monitor counts it as worker activity, end-to-end tested through the real handler.
