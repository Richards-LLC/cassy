### Fixed — CI

- **Green release-train full set** — Script suites no longer inherit the calling job's `FULL_SET`, `BASE_SHA` or queue-step recovery, so their fixtures classify their own diffs. Visual QA writes its CI report outside the tested tree, a script step run by two jobs is two publication slots, and a red full set still records its FAIL receipt.
