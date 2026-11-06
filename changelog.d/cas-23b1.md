### Fixed — Factory

- **Reuse Rust proof after builtin doc corrections** — Editing builtin Markdown or its reference history after a passing Rust test no longer requires rerunning that test to park. Contract and doc-hygiene checks still cover the documents; other changed test or build inputs still invalidate the receipt.
