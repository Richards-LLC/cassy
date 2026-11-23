### Fixed — Director and watchdog notices

- **No repeated idle notices while a worker is being recycled** — After the supervisor runs `clear_context` (or any respawn) on a worker, the director no longer re-sends that worker's idle or "pending delivery" notice every minute while the replacement starts. Notices resume normally once the new worker has registered (GH #1693).
