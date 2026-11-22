## Core Concepts

The local quarantine ledger hides rows whose home project cannot be established, and a decision applies to the row that existed when it was taken. Because the ledger is keyed by bare task id, an entry older than a local task stamped with this project's identity is stale: pull applies that task's peer updates, its queued pushes are kept, and it stays on the board. Rows that already existed when they were quarantined, and ids whose local row belongs to another project or has no origin, remain quarantined.
