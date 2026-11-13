## Core Concepts

### Changed-test stability at park

Every factory project uses the existing focused-proof gate to require five
consecutive passing executions of its delivery's new or changed tests. Receipts
bind the repository, exact delivery commit and source test identities. Existing
one-pass crate evidence remains useful for unchanged tests. A later overlapping
failure retires earlier stability evidence at that commit.

The existing documentation, test, fixture and CI only delivery exemption remains
in force. In particular, a rejected QA round can reopen after a test-only
correction when its product change has already reached the target. A delivery
that also changes product source still needs its changed tests' five-pass proof.

The capped Rust runner resolves Cargo roots and source modules, runs the requested
selection once, then repeats only its changed functions four more times within
the same admission and memory cap. JavaScript adapters use static source names
and runner reports; Playwright uses repeat-each with retries disabled. A failure
stops execution. Park only reads the resulting receipt and names missing tests.
