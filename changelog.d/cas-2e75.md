### Fixed — CI

- **Keep known flakes out of release queue checks** — Quarantined six tracked flaky tests through the existing issue labels, with exact test identities retained in queue receipts. The verifier duration test now compares the JSON ledger measurement at integer-millisecond precision instead of requiring exact floating-point equality.
