### Fixed — CI

- **Retain own-build diagnostics on trusted runners** — Lane own-test builds use the trusted `cas-lane-light` runners and retain complete build/test output alongside the watchdog heartbeats. Self-hosted setup bootstraps the pinned Rust toolchain in the runner home when needed and skips sccache. Hosted setup, ring/browser placement and timeout policy are unchanged.
- **Run existing refusal audits before the ring** — Production Rust deliveries include the existing gate-registry audit in their own-test selection even when no import edge leads to that module. Integration-only fixture edits keep their existing build scope.
