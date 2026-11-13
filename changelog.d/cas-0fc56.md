### Changed — Release validation

- **Train suite runs once per release, at the freeze** — Merges into a release train no longer start the full train suite or cancel the one in progress; lane CI already proves each delivery. Freezing the train starts exactly one run at the frozen tip, its green receipt is what the cut consumes, and a red still raises the train-red notice. Auto-cut now freezes a ready train first and cuts on that run's green.
