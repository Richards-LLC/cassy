### Fixed — Factory

- **Same-tip re-park receipt** — A fully checked close retry at an unchanged durable park now reports COMMITTED while its follow-up continues, without creating a new delivery cycle.
- **Completed close shutdown** — Close continuations release their worker ownership lock before signaling idle, so a finished close no longer briefly refuses worker shutdown.
