### Fixed — Factory task ownership

- **Preserve peer task status during session startup** — Startup no longer reopens another machine's tasks merely because they have no local lease. Orphan recovery requires a previous local holder matching the assignment and retains tasks that change during recovery.
