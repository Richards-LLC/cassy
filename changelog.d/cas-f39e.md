### Fixed — Merge response receipts

- **Return merge state before the MCP deadline** — Slow candidate preparation and CI queries continue outside the response worker. Merge calls return a durable state receipt after a short wait, followed by a saved result and supervisor notice; fast calls retain their existing result.
