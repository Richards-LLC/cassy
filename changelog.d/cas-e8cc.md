### Fixed — Release automation

- **Backfill respects frozen trains and close holds** — Historical work skips frozen successor trains. Dry-runs evaluate release receipt admission, report dedicated-review, CI-override and retired-park holds, and mark remaining ordinary close gates as pending. Apply reports use each actual close result and retain refusal codes. A delivery published in a later release is reconciled under that tag instead of being counted as unmerged work.
