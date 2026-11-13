### Fixed — CI

- **Keep subprocess contracts in source-change lanes** — Rust impact selection now follows shared integration helpers and retains `command()` callers when any `cas-cli/src/` input changes. Existing lane receipts list the discovered subprocess suites, alongside the selected tests and elapsed time.
