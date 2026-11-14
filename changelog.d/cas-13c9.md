### Fixed — Factory checks

- **Shared journey stability identities** — Existing Playwright journeys can declare the expanded runtime identities of a local shared callback. The capped worker check validates those declarations against native discovery before collecting five passes with retries disabled; unknown or ambiguous bindings remain refused.
