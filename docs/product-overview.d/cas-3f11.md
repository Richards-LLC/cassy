## Core Concepts

### JavaScript stability checks

The capped worker runner accepts `cas factory worker-check --cas-root "$CAS_ROOT" -- vitest path/to/file.test.ts`. Substitute `jest` or `playwright` for the runner and select that runner's test files. Paths are relative to the repository root. It resolves each file's package and uses its locally installed runner in an isolated worker or managed review checkout.

Only new or changed test functions are selected from the delivery diff. Vitest and Jest run the selected names five times; Playwright uses five repetitions with retries disabled. The first failure stops execution and names the failing test. A successful run records the selected identities, five consecutive passes and elapsed time against the clean commit. Park reads that evidence without running tests.

Dynamic or parameterized changed titles, out-of-line callbacks and test factories that cannot be mapped safely to runtime identities require an explicit source-to-runtime resolution instead of broadening execution to unchanged tests.
