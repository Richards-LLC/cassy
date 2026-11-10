### Fixed — Factory

- **Bound transcript delivery alarms to recent decisions** — Ask/blocker mirrors and answers linked to operator input get one batched alert only after five minutes without a device receipt and before an hour has passed. Old replay copies and informational turns stay quiet, with durable deduplication and outbox retries preserved.
