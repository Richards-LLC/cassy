### Added — Factory

- **Catch unstable tests before park** — A delivery's new or changed tests need five consecutive capped passes, including test-only corrections. The existing receipt gate reads that evidence without running tests. Rust repeats only changed functions in the requested selection; Vitest, Jest and Playwright use the same receipt contract. A failure stops the run and leaves park held for a fix or deletion.
- **Preserve proof after documentation changes** — Stability shares crate proof's input and ancestry checks, so a documentation or changelog commit can retain its five-pass receipt. Test and build changes require fresh evidence; target merges keep the existing resolution checks.
