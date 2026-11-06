### Fixed — MCP startup

- **Fail-fast initial backend** — `cas serve` validates its store and reports readiness before the client handshake while replacement backends remain inactive until approved. Local worker-target refusals retain their scope diagnostic without requiring a Cloud login.
