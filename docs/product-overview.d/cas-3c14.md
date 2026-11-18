## Core Concepts

- **Park proof matches lane CI** — A worker can run lane CI's own impact selection for its diff under the capped runner (`worker-check -- lane-impact`). The receipt records the selected expression and covers each changed crate, so the tests that most often bounce a delivery in lane CI have already passed at park.
