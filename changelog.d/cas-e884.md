### Added — Testing skills

- **Playwright suites get a static staleness scan before a rewrite** — Was:
  agents ran an inherited Playwright suite and triaged the red, so specs that
  passed while testing nothing went unnoticed. Now: the Playwright authoring
  skill ships a dependency-free scan. It resolves each spec's routes, API
  paths, test ids, classes and `data-*` selectors against the current sources
  (Nuxt pages, static/Vite roots, NestJS and axum routes, or a route list). It
  grades each spec CURRENT, UNSURE, STALE or DEAD with a file:line receipt,
  and suggests a kind and an action for each spec.
