### Fixed — Factory

- **Link quiet peer answers** — A peer reply carrying an exact `in_reply_to` now resolves its outgoing ask even when delivered quietly. Answered questions no longer remain open in the merge gate; unrelated quiet and status messages retain their existing behavior.
