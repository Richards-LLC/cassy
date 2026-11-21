### Fixed — Factory tests

- **Release-path flake removed** — deleted
  `supervisor_single_turn_tests::cas_1e7e2_directed_burst_debounces_into_one_body_turn`.
  It failed in lane CI run 44933603487 attempt 2 (ring 2/3, 3.98s, panics at
  `supervisor_single_turn_tests.rs:173` and `:1517`), was unattributed, and
  passed on rerun and at base c537ccdbf50b.
