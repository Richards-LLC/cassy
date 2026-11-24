### Fixed — Task close

- **Park evidence requires only cited files** — Was: a close reason that used
  the words "target" or "target/owner" made the park require a missing file,
  every fetched artifact copy was published again on each park, and each retry
  added another evidence-sync note. Now: only concrete cited files are
  required, fetched copies are skipped, a retry replaces its evidence note,
  and two restores of the same peer artifact no longer collide.
