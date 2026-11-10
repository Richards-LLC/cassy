### Fixed — Factory

- **Retire expired transcript copies quietly** — An aged Commander transcript replay no longer creates a supervisor blocker, including copies from the current session. Durable outbox retries and alerts for undelivered operator asks or decisions continue.
