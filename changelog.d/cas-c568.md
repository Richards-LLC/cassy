### Fixed — Factory

- **Preserve current-session delivery warnings** — Undelivered transcript mirrors authored during the current supervisor session still raise a delivery warning, including decisions carried in status replies. Prior or ended supervisor-session mirrors expire quietly, while explicit operator asks retain their watchdog alerts and durable operator delivery keeps retrying.
