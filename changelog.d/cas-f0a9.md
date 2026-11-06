### Fixed — Factory

- **Train failures surface before the cut** — Release-train tips reuse the existing Rust CI suite, keeping complete logs and naming failing tests and their merge/task range. A known red holds the ordered merge queue until green or an audited supervisor decision. Release preparation requires a green starting-tip receipt and replaces the local pre-cut full run.
