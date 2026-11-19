### Fixed — Knowledge

- **Preview knowledge sources in order** — `cas knowledge build --dry-run --json` now returns JSON with counts and every pending source path in distillation order. Plain dry-run output lists the same plan, so the first five sources are visible before a bounded build. Neither preview invokes a model or changes knowledge pages or source ledger rows.
