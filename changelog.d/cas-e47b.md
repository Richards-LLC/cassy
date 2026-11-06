### Added — Delivery health

- **Merge queue drift advice** — For projects with an active merge queue, setup, doctor and release preflight report a queue that builds fewer than five entries in parallel, merges more than one entry at a time, or groups other than ALLGREEN, with a ruleset-preserving repair command. Checks only report; unavailable GitHub access never blocks a release.
