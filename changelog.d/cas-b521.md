### Fixed — Commander

- **Queued commands show their real outcome after the session ends** — when a
  supervisor exits, its session is ended, or its daemon crashes with a command
  still queued, Commander now reads the command's retained receipt from the
  hub and shows Not sent instead of Unconfirmed. The read is authenticated,
  limited to the device that sent the command, read-only, and waits while the
  recording daemon may still be running.
