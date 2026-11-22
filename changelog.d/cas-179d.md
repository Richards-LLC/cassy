### Fixed — Factory worker checks

- **CI guard helpers prove through their node:test guards** — A changed helper under a test directory that only `node:test` guard tests import, such as a check that reads a GitHub Actions workflow, now runs through those guard tests. Park prescribes `worker-check -- node --test <helper>`, which runs the importing guards once, capped, and park accepts that receipt. Before, the helper widened to its whole package, and park could name the workflow YAML as a Playwright test that the capped runner refuses. A failing guard records no proof (GH #2200).
