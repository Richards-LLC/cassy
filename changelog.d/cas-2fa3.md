### Fixed — Factory tests

- **Release-path flake deleted** — removed
  `cas_7b6b_reset_before_writer_runs_preserves_durable_rows`: lane CI run
  44922789927, ring 3/3, it panicked at `queue_control_tests.rs:562` after
  10.3s, then passed on rerun and at base cfe55015. Deleted per the standing
  no-quarantine order; no production code changed.
