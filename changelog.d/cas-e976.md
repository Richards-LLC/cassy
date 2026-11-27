### Fixed — Factory

- **Peer merges ignore stale worktree records** — A parked delivery that exists only on origin can merge at its recorded commit even when the merge machine still has a worktree record for a missing checkout. Live checkout, source, review, and CI checks still apply.
