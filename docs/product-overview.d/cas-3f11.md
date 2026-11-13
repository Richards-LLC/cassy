### JavaScript stability checks

The capped worker runner accepts `cas factory worker-check -- vitest path/to/file.test.ts`, `jest path/to/file.test.js` or `playwright path/to/file.spec.ts`. Paths are relative to the repository root. It resolves each file's package and uses its locally installed runner.

Only new or changed test functions are selected from the delivery diff. Vitest and Jest run the selected names five times; Playwright uses five repetitions with retries disabled. The first failure stops execution and names the failing test. A successful run records the selected identities, five consecutive passes and elapsed time against the clean commit. Park reads that evidence without running tests.

Dynamic or parameterized changed titles that cannot be mapped safely to a runtime test name require an explicit resolution instead of broadening execution to unchanged tests.
