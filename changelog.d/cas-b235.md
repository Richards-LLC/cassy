### Fixed — Factory

- **Keep input from one-shot attach senders** — Closing an attach socket while pane output flows no longer discards unread input. Pane snapshots now report accepted and physically written PTY input bytes to distinguish transport loss from a stalled consumer.
