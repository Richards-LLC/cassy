### Fixed — CI

- **A failing queue shard names its failing tests** — Was: `scripts/ci-queue-flakes.py` ended a real failure with `FAIL: 0 flaky test(s)`, which hid the failing tests and read as a gate fault. Now: it prints `FAIL: N failing test(s): <names, first 10> (+M more); receipt <path>`. FLAKY and PASS lines are unchanged.
