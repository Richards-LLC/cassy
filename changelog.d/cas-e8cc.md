### Fixed — Release automation

- **Backfill respects frozen trains and close holds** — Historical work skips frozen successor trains. Dry-runs evaluate release receipt admission, report dedicated-review, CI-override and retired-park holds, and mark remaining ordinary close gates as pending.
