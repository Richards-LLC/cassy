### Fixed — Factory

- **Keep failed merges off the shared target** — Worktree merges prepare their commit in a detached checkout, publish against the observed origin SHA, and then advance the local target. A failed push leaves the target at its previous tip so later lanes can continue. Closes #1794.
