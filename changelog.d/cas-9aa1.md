### Fixed — Shared file notices

- **Resumed workers do not collide with themselves** — File-edit notices
  recognize the same worker and task across context resets, including retired
  registrations, while retaining warnings for distinct owners.
