### Fixed — Task delivery

- **Park against remote-only train refs** — The test-shape gate resolves the delivery target before checking assertions, so workers with only an origin train ref can park. Missing targets report `TEST_ASSERTION_TARGET_UNRESOLVED` instead of a prose assertion violation.
