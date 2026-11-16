### Changed — CI

- **Six Rust suite shards and fewer CI handoffs** — Full validation partitions the Rust suite into six shards, combines classification with runner routing and reuse checks, and collects reports and repair issues in the existing full-set gate. The required Fast Validation job and default-feature queue check continue to gate the release.
