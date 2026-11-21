### Fixed — Factory

- **Merge-queue ejections record their failing tests again** — The CI watcher
  now strips the job, step and timestamp prefix and gh's colour codes from
  `gh run view --log-failed` lines, and reads both libtest `test <name> ...
  FAILED` and nextest `FAIL [..]` lines, so the class-audit gate fires after an
  ejection.
