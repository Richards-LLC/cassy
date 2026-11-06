### Fixed — Factory

- **Peer asks hold only their related merges** — Untyped peer messages no longer create merge holds. A later message from the same peer clears an explicit ask even without a reply ID, including split decisions and review verdicts; unrelated task merges remain available.
