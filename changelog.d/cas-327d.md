### Fixed — Commander

- **One bad frame no longer drops the whole connection** — If Commander fails while handling a single message from a machine (for example while drawing it), it now reports the error and keeps the connection, instead of tearing it down. Before, the same message arrived again after every reconnect, so every supervisor on that machine kept showing "Reconnecting".
- **The hub says why a Commander connection closed** — The hub log now records why each Commander machine connection ended (the browser closed it, the command backlog filled, a write failed, the device was revoked or failed its recheck), so a reconnect loop is diagnosable from the hub alone.
