### Fixed — Verification

- **No-code closes get an independent verifier** — Was: a no-code close
  dispatch told the supervisor to spawn a Task/Agent `task-verifier`, which
  shares the supervisor's MCP caller and cannot record, so the dispatch ran to
  its deadline. Now: Cassy's daemon-owned verifier reviews the exact close
  dispatch as it does merge candidates, a held verdict notifies the owner, and
  retry notices name only a retried close or merge or the supervisor's direct
  verdict.
  Rejected-verdict, timed-out, spawn-refusal and non-worker close relays now
  name the same supported routes instead of a Task/Agent spawn.
