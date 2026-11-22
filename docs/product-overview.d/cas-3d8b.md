## Core Concepts

The local quarantine ledger hides rows whose home project cannot be established. Because it is keyed by bare task id, an entry that names a task stamped with this project's identity is treated as stale: pull applies that task's peer updates, its queued pushes are kept, and it stays on the board. Entries for ids with no local owner, or whose local row belongs to another project or has no origin, remain quarantined.
