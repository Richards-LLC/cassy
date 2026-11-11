## Core Concepts

- **Doc fragments** - deliveries never edit the shared journey registry map, `.claude/CODEMAP.md` or `docs/PRODUCT_OVERVIEW.md`. They add `docs/qa/bug-families/<slug>.yaml` and `docs/product-overview.d/<task-id>.md` files; the `journey-catalog` lane lint checks them and `scripts/release.sh` folds them in once per release, so parallel deliveries never conflict on shared docs.
