### Fixed — Factory

- **Reclaim recorded candidate binaries** — Artifact GC can remove candidate executables 48 hours after their task closes when the task ledger records their matching SHA-256 and version, keeping identity records and reporting reclaimed bytes. Open tasks retain their candidates.
