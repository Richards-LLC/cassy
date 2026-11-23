### Fixed — Factory worker checks

- **Journeys that import a shared fixture are prescribed Playwright** — A Playwright spec that imports `test` from a local fixture such as `./journey`, rather than from `@playwright/test`, sat beside a Vitest config, so park prescribed a Vitest run that could never pass it. Now the Playwright config that lists the spec decides, and a plain unit test in the same package still gets Vitest (GH #2294).
