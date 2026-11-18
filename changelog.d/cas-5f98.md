### Fixed — Factory

- **Local database fixtures in JavaScript checks** — Capped JavaScript tests accept an explicit password-free loopback PostgreSQL URL through `--test-env TEST_DATABASE_URL=...` or `--test-env <PREFIX>_TEST_DATABASE_URL=...`, and pass it to discovery and every test child.
