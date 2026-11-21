### Fixed — Factory daemon

- **A verifier still starts when the process that asked for it exits** — Was:
  the retry for a verifier launch ran only inside the requesting worker's MCP
  process, so if that process exited the review waited, unstarted, until its
  deadline expired and a supervisor retried it. Now: the factory daemon starts
  any review in its session that was never launched, once per review.
