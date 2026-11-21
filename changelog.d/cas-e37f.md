### Fixed — Factory daemon

- **The progress watchdog no longer holds the store writer while it scans** — Was:
  every watchdog sample listed all tasks and ran git while holding the daemon's
  single store writer, up to 8 s at a time, so Commander sends and other writes
  waited behind it. Now: the scan runs on the external lane, and only the
  watchdog's notices and wake edges take the writer.
