## Core Concepts

- **Supervisors stay reachable** — the supervisor's Bash hook (gate H-49) refuses a foreground call expected to run past a minute: Cargo builds and tests, release scripts, npm/pnpm test or build, loops with sleep, ssh running any of those, `timeout` above 60 seconds, an explicit tool timeout above 60 seconds, and the other commands in one known-long list. The remedy is `run_in_background: true` or delegation to a worker. Short commands, backgrounded calls, subagents and Codex or Grok shells are not affected.
