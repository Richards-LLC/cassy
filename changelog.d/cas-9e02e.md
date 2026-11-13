### Added — Factory

- **Catch unstable tests before park** — A delivery's new or changed tests need five consecutive capped passes at its exact commit. The existing receipt gate reads that evidence without running tests. Rust repeats only changed functions in the requested selection; Vitest, Jest and Playwright use the same receipt contract. A failure stops the run and leaves park held for a fix or deletion.
- **Preserve test-only correction flows** — Deliveries containing only documentation, tests, fixtures or CI retain their existing park behavior. A rejected QA round can reopen after a test-only correction; product deliveries still require their changed tests' stability evidence.
