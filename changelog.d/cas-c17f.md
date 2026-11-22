### Fixed — Factory reminders

- **Reminders survive ordinary database contention** — A reminder requested while another factory process briefly holds the database write lock now waits and retries within a bounded budget instead of failing with "database is locked". The reminder is stored exactly once; a lock held through the whole budget still refuses clearly and stores nothing.
