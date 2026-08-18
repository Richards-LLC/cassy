# is-wedged can't resolve codex worker transcripts → false "starved" + director stall-alert spam

**Date:** 2026-07-21
**Reporter:** warm-falcon-13 (supervisor, ozer project, session ozer-nimble-panda-75)
**Severity:** major (no data loss, but the monitoring loop cries wolf — 4 false stall alerts in one session, and a hasty supervisor following the kill guidance would destroy in-flight work)

## Symptom
With `cli=codex` workers (`gpt-5.6-sol`), the director emits "stalled ~5m, alive heartbeat, no activity, auto-nudge did not unstick" for workers that are demonstrably mid-inference. Triage tooling can't see codex activity:

```
$ cas factory is-wedged worker-android
state: starved
  pid: 111084 (alive: true)          <-- this is `cas serve`, NOT the worker
  transcript: <unresolved>
  transcript mtime age: <unknown>
  worktree recent-edit age: <unknown>

$ cas factory debug worker-android
[ERROR] no transcript found for worker `worker-android`
(session codex-worker-android-2f828ac6-...)
```

Ground truth at that exact moment: `ps aux | grep codex` showed four wrapper+vendor-binary pairs at 3.8–6.6% CPU (actively inferencing), and two of the four "stalled" workers had already pushed commits to their factory branches.

## Two distinct defects
1. **Registered PID is wrong for codex workers.** `is-wedged` inspects PIDs 111084/111599/130021 — all `cas serve` processes — so "alive: true" is vacuous and CPU/state signals come from the daemon, not the worker.
2. **Transcript resolution doesn't know the codex session layout.** `codex-worker-<name>-<uuid>` sessions resolve to nothing (`~/.codex/sessions/` uses a different scheme), so mtime-age classification degrades to "starved" whenever the worker goes >N min without a Cassy tool call — which codex workers routinely do during long read/plan/inference stretches.

## Impact
- Director fires stall alerts on a timer for healthy workers (observed for all 4 workers within the first ~15 min of an epic).
- The alert text steers supervisors toward `cas factory kill`; the only guard is the supervisor independently knowing to check `ps`.
- 5-minute "no activity" is far too aggressive a threshold for codex-CLI workers even once transcripts resolve.

## Expected
- `is-wedged` tracks the actual codex child process (wrapper or vendor binary) per worker.
- Codex transcript/rollout files resolved (or an explicit "codex: transcript introspection unsupported — classifying by process CPU + worktree writes" downgrade path).
- Classification should incorporate process CPU and git activity (branch tip movement) before declaring starved; stall threshold per-CLI.

## Workaround (documented in ozer project memory 2026-07-21-4)
`ps aux | grep codex` — vendor binary at 3–7% CPU = inferencing; check factory branch tips for commits; ignore "starved" verdicts lacking corroboration.


## Completion

- **completed:** 2026-07-21
- **epic:** cas-887b — Factory reliability: open docs/requests bugs → main
- **completed_by:** cas-c655
- **status:** Fixed on epic tip; report archived from `docs/requests/`.
