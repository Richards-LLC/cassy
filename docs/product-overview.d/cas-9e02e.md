## Core Concepts

### Changed-test stability at park

Every factory project uses the existing focused-proof gate to require five
consecutive passing executions of its delivery's new or changed tests. Receipts
bind the repository, tested commit and source test identities. Existing
one-pass crate evidence remains useful for unchanged tests. A later overlapping
failure retires overlapping stability evidence at that commit and its ancestors.

Test-only corrections also require five-pass proof. Documentation or changelog
commits preserve evidence through the same input and ancestry checks as crate
proof; target merges use the same resolution checks. Changes to test or build
inputs invalidate old evidence, even when a test keeps its name.

The capped Rust runner resolves Cargo roots and source modules, runs the requested
selection once, then repeats only its changed functions four more times within
the same admission and memory cap. JavaScript adapters use static source names
and runner reports; Playwright uses repeat-each with retries disabled. A failure
stops execution. Park only reads the resulting receipt and names missing tests.
