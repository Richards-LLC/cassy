### Added — Factory

- **Supervisors stay reachable during long commands** — Was: a supervisor could sit in a foreground Cargo build, release script, npm/pnpm test or build, sleep loop or ssh wait for minutes, and operator and Commander messages waited until it returned. Now: the supervisor's Bash hook refuses such a call unless it runs with `run_in_background: true`, and names the remedy: run it in the background or delegate it to a worker. Calls with an explicit timeout above 60 seconds count as long; short commands pass untouched.
