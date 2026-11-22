### Fixed — Factory worker checks

- **Playwright helpers run through the specs that import them** — A changed Playwright helper with no tests of its own, such as a journey support file, no longer widens to a run of its whole package. `worker-check -- playwright <helper>` runs the specs that import the helper, directly or through other helpers, once and capped. That receipt satisfies park for the helper. A helper that no spec imports, or that a unit test also imports, keeps its previous proof; a type-only `@playwright/test` import does not make a Vitest consumer a Playwright spec (GH #2146).
