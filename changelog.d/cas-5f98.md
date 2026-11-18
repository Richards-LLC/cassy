### Fixed — Factory

- **Local database fixtures in JavaScript checks** — Capped JavaScript tests accept an explicit password-free loopback PostgreSQL URL through `--test-env TEST_DATABASE_URL=...` or `--test-env <PREFIX>_TEST_DATABASE_URL=...`, and pass it to discovery and every test child.
- **Conditional JavaScript test identities** — `describe.runIf` and `describe.skipIf` retain suite titles. Explicit skipped prerequisite registrations do not require passing results; runnable tests still do. Unknown conditional aliases retain their provenance for broader evidence selection.
- **QA ledger consistency** — CLI preflight and close use the same task identity from a suffixed ledger heading and its coverage header. Readable absolute captures inside the ledger directory are accepted, with canonical containment still enforced. Ledger initialization reads the caller's worktree registry and fallback build revision when it belongs to the target repository.
- **QA follow-up deduplication** — An evidence-comment failure on an existing match is reported without creating a duplicate follow-up task.
- **Traceable changed-test receipts** — Selected changed identities record `reason: changed-test` while retaining their existing matching semantics.
