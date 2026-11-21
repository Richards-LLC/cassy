### Fixed — Factory

- **macOS worker kill stops a runaway worker** — `cas factory kill` on macOS
  now verifies the Claude harness recorded beside the `cas serve` helper,
  including the npm `claude.exe` client, instead of refusing with a PID
  identity mismatch. It stops and kills the harness's whole process tree,
  including Bash tool shells in their own process groups, and leaves unrelated
  processes alone. macOS workers also start with a soft process limit below
  the per-user ceiling, so a recursive shell runs out of process slots before
  the operator's own shells do.
