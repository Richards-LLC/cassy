### Fixed — Factory

- **Automatic host disk retention** — Sweep expired artifacts, retired worker output and old scratch across registered projects. Preserve live and release evidence, unpushed source and active output; report reclaimed bytes and disk pressure in preflight.
- **Review evidence preservation** — Keep managed reviews explicitly cited by other live tasks or retained releases, reject linked artifact namespaces before resolving paths, and include managed review bytes and retained paths in sweep and preflight totals.
