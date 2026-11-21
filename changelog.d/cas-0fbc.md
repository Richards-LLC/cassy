### Added — Verification

- **Carry failing tests from verifier findings into repair checks** — Blocking rejected findings can name tests in `triage.failing_tests`. Their valid identifiers merge with the task's recorded failures in the verdict transaction, so later repair requests must name a covering test class. Older verdicts remain readable; invalid names are dropped without rejecting the verdict.
