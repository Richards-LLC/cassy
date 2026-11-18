### Fixed — Cloud

- **Recover rejected credentials once** — Artifact publication, peer messages, message status and agent discovery re-read the current login after the server rejects an invalid API key, or verify and persist the configured environment credential through the normal token-login path. Other 401 responses preserve the server cause without retrying. A failed credential recovery names the expired Cloud login and its login command; uncertain writes are never replayed.

### Fixed — Developer checks

- **Keep fixtures outside the checkout** — Capped test scratch uses a short directory outside Git and `.cas` ancestry. Credential fixtures also protect real login files when a test uses a production-shaped token, keeping no-Git tests and artifact fixtures isolated.
