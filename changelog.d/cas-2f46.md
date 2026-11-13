### Fixed — CI

- **First failures stay failed** — Removed queue retries, test exclusions and cleanup-to-success conversion. Native failures now create deduplicated P0 repair issues with a next-working-day fix-or-delete deadline, preserved on later observations.
