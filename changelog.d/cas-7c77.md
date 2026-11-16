### Fixed — CI

- **Reuse unchanged full-set job inputs after repairs** — Commander web and journey proof can survive Rust-only repairs, and Rust proof can survive unrelated web source repairs. Shared dependencies and unknown inputs still invalidate proof; the full-set gate retains every required job and records the original producer.
