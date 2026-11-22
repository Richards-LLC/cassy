### Fixed — Factory

- **Supervisor routing explains unavailable owners** — Worker messages use the same registry snapshot for owner selection and diagnostics. Expired or ambiguous owners receive precise failure data, and stale session metadata cannot route a message into another factory.
