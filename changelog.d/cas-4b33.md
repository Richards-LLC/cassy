### Changed — Factory

- **Shared docs are generated at the release cut** — Was: every delivery had to
  edit `docs/qa/journeys.yaml`, `.claude/CODEMAP.md` and
  `docs/PRODUCT_OVERVIEW.md`, park refused with CODEMAP DRIFT, PRODUCT_OVERVIEW
  DRIFT or registry upkeep findings, and parallel deliveries conflicted on those
  files. Now: deliveries add their own `docs/qa/bug-families/<slug>.yaml` and
  `docs/product-overview.d/<task-id>.md` fragments, `scripts/release.sh` folds
  them in once, the `journey-catalog` lane lint checks the registry and
  fragments, and park, the supervisor task-create gate and the stop hook no
  longer check CODEMAP.
