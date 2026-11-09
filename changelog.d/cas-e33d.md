### Fixed — Factory

- **Report batch close holds after worker retirement** — Covered deliveries still use ordinary close gates, and any remaining gate now queues a durable supervisor action. Replayed evidence avoids duplicate outcome notes and hold notices.
