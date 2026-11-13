### Fixed — Commander

- **Read conversation history while the daemon is busy** — history uses its own read-only WAL connection, so daemon writes cannot make the conversation fail to load or force repeated retries. Phase status appears only when a phase or a waiting question is known.
