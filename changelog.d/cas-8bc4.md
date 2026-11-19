### Fixed — Task start

- **Starting a task whose epic branch was never pushed now names the missing branch** — Was: `task action=start` passed an empty start point to git and failed with "start point '' does not resolve to a commit", naming nothing. Now: start refuses with the missing target branch name and the remedy (ask the supervisor to create or push it), and creates no task branch. Measured: the new test fails at the delivery base with the empty start point error and passes with this change.
