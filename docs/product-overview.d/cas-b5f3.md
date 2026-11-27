## Core Concepts

- **Worker spawn receipts** — If a spawn outlives the response deadline, read its receipt instead of retrying. Once it has a queue request ID, check worker status to see whether that worker registered or failed to launch.
