### Added — Friction ledger

- **Delivery review and intervention detail** — The ledger shows review rounds by delivery cycle and verdict provenance, wakes by authenticated notice kind, and Commander/manual supervisor interventions. Backfill preserves sourced manual repairs and enriches legacy facts without counting them twice. Commit-to-main measures individual deliveries inside a release train.

### Fixed — Release checks

- **Registry validation in clean checkouts** — Legacy validators use the registry's explicit policy without requiring a local Cassy store. Missing policy still needs configuration, and strict policy still requires a validator supporting base comparisons.
