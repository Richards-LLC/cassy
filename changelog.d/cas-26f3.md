### Fixed — Developer tooling

- **Admit runtime inputs in behavioral tests** — The test-shape guard no longer mistakes Git helper arguments or Rust `expect` failure messages for exact-text expectations. Returned-text expectations and source needles still need their pin reason.
