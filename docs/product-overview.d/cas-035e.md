## Core Concepts

When G-61 widens a zero-test JavaScript package with no covering specs, `cas factory worker-check -- package-check <package>/package.json` runs the tracked `scripts.check`, or `scripts.test` if there is no check. It records one passing check, not a test or a stability pass.

In `docs/qa/journeys.yaml`, set `worker_check: staging_only` or `worker_check: paid` on a Playwright journey to move its G-61 requirement to release batch QA. For a Playwright spec that is not a journey, add `- {spec: tests/checkout.spec.ts, worker_check: paid}` under the top-level `spec_checks:` list. Paths must be exact and tracked. The same rule applies to direct edits, helper importers, and shared locale fan-out. Unmarked specs retain their existing requirements. Park records the deferred specs, and `cas qa ledger init` with `--covers` or `--release-scope` lists them as owed rather than passed. Ledger init refuses when an included code delivery has no matching deferral record.
