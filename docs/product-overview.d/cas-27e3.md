## Core Concepts

- **Bounded supervisor startup** - Supervisor startup checks parked integrated deliveries for 10 seconds at most. Deliveries it did not reach are listed with their commit receipts, and close still checks each one. A compacted session keeps its registration and does not repeat the check.
- **Claude supervisor context window** - Claude's status line input reports the session's context window, which Claude transcripts omit. Cassy records it for supervisor sessions, so the automatic handoff and clear start at 80% of that window, below Claude's own compaction point.
