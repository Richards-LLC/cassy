### Fixed — CI

- **Reuse unchanged full-set job inputs after repairs** — Frozen-train Commander web and journey proof can survive unrelated Rust repairs, and Rust proof can survive unrelated web source repairs. Shared dependencies and unknown inputs still invalidate proof; queue job receipts keep exact-tree keys, and the full-set gate retains every required job and records the original producer.
