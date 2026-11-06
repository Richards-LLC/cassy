### Fixed — Factory

- **Leave room for Unix sockets in capped test runs** — Worker and verifier
  temporary files now use short private scratch directories instead of deep
  target paths. Retirement and verifier GC validate checkout ownership before
  reclaiming them; socket-only candidate fixtures use the short socket helper.
