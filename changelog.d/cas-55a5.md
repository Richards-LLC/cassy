### Fixed — Factory

- **A live worker is no longer listed as retired work on macOS** — The session-start retained-work banner matched a worker to its checkout by comparing paths as text. On macOS a worker records its checkout under `/var`, while Git lists `/private/var`, so a live worker's checkout looked unowned and appeared as recoverable retired work. Paths that resolve to the same checkout now match. Retired workers' local work is still reported.
