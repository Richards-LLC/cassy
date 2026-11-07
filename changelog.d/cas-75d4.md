### Fixed — Merge queue

- **Run shards despite bad quarantine metadata** — Duplicate or malformed quarantine entries warn with their issue number and are skipped. Read failures run tests without exclusions, and valid GitHub issue links accept repository name casing. Actual test failures keep their failing result.
