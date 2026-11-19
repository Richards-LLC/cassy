### Fixed — Factory

- **Worker recycle continuity** — Startup cleanup preserves assignments while a same-name replacement starts and reclaims its lease. Codex context notices and worker status exclude usage older than an actual recycle, while retaining current-session usage written before registration and usage from workers that have not recycled.
