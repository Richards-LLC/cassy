## Core Concepts

- Local factory verifier startup queues a critical external job and returns its dispatch-correlated result to the requesting socket. Authority, session scope and exact dispatch claim remain checked before a verifier starts. Shared SQLite guards record the holding operation, source location, thread and hold duration for contention diagnostics; untracked external mutex owners remain unknown. A late handoff ownership or state change is a typed dispatch refusal, and store contention does not consume launch attempts.
