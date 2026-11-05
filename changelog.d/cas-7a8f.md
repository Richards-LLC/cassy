### Fixed — Merge publication

- **A post-merge regeneration commit no longer blocks the landing check** — When `worktree_merge` regenerated generated artifacts (reference history, the task-verifier projections or the hub-web bundle) on top of a delivery merge, landing verification read that commit as later target progress and refused to publish. The local target was left ahead of origin until a supervisor pushed by hand. Verification now checks the delivery merge beneath such commits, and any other commit on top is still refused.
