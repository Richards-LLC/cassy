### Changed

- Lane CI for a factory push now runs the train's impact-selected Rust tests for that delivery, so a change that breaks a test outside its own module fails before merge. The changed modules' own tests run first; the remaining selected tests run in up to three hosted shards only after those pass. Deliveries with no provable scope (build policy, scripts or workflow changes) keep relying on the train suite.
