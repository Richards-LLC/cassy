### Fixed — CI

- **Green release-train full set** — Script suites no longer inherit the calling job's `FULL_SET`, `BASE_SHA` or queue-step recovery, so their fixtures classify their own diffs. The Commander build is no longer wrapped as a retried test step, a script step run by two jobs is two publication slots, and a red full set still records its FAIL receipt.
