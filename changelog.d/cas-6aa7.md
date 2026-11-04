### Fixed — Factory coordination

- **Keep delivery alerts with the owning factory** — PR failures, merged
  delivery wakes and close-timeout alerts now follow the worker's local
  registration and factory session. A task synced from another machine no
  longer wakes the watching machine's supervisor about that worker.
