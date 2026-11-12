### Fixed — Factory

- **Concurrent receipt handoff** — Exact worker receipt submissions now share the result of an in-progress candidate build instead of receiving an immediate merge hold. Slow builds retain their bounded wait and completion notice.
