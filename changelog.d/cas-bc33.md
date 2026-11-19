### Fixed — Factory

- **Faster changed-test discovery at close** — Reuse Rust inventories within one close evaluation and compile discovery patterns once, avoiding repeated full-crate scans for each delivered file. Missing tests, uncertain inputs and five-pass evidence retain their existing gates. Timing logs identify the task, delivery head and changed-test discovery phase.
