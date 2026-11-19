## Core Concepts

Knowledge build previews expose the complete pending source plan in distillation order. Previously, `cas knowledge build --dry-run --json` printed text and dry-run output showed only counts. It now emits a JSON report with `pending_sources`, `sources_scanned`, `sources_pending`, `sources_skipped` and `sources_deferred`; ordinary dry-run output lists the ordered paths too. The plan includes paths beyond `--max-sources`, while `sources_deferred` counts those beyond the pass limit. Both previews leave knowledge pages and source ledger rows unchanged and make no model calls.
