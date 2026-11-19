### Fixed — Factory

- **Worker recycle continuity** — Startup cleanup preserves assignments while a same-name replacement starts and reclaims its lease. Codex context notices and worker status ignore rollout usage written before the replacement registered, instead of attributing the predecessor's occupancy to it.
