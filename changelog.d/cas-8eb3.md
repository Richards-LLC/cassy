### Fixed — Peer delivery refs

- **Follow an anchored peer re-park automatically** — Merge preparation fast-forwards a stale local factory ref when origin matches the task anchor. Divergent local work still refuses G-96, and the existing checkout and concurrent-update protections apply.
