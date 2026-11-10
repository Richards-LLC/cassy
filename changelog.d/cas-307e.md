### Fixed — Worker spawning

- **Refresh branches outside a narrow fetch mapping** — Worker base checks now update the requested origin tracking ref explicitly, so a main-only fetch configuration cannot silently start a worker from stale staging code.
