### Fixed — CI

- **Retain own-build diagnostics on trusted runners** — Lane own-test builds use the trusted `cas-lane-light` runners and retain the complete build/test output alongside the existing watchdog heartbeats. Ring and browser jobs keep their existing placement, and timeout policy is unchanged.
- **Run existing refusal audits before the ring** — Production Rust deliveries include the existing gate-registry audit in their own-test selection even when no import edge leads to that module. Integration-only fixture edits keep their existing build scope.
