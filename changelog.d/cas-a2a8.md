### Fixed — CI

- **Script-suite logs stay out of the checkout** — The parallel script-suite runner wrote per-suite logs under `target/ci-script-suite-logs` in the repository it ran in, which left fixture checkouts dirty and made the fast-rows pre-push hook refuse a clean push. Logs now go to a temporary directory (under `RUNNER_TEMP` in CI); each suite's complete output is still replayed at once and failing suites still repeat their last lines after the summary.
