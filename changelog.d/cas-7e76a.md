### Fixed — Authorized branch takeover

- **Cleanup before adoption is allowed** — A worker standing on another worker's branch in its own registered worktree can now restore files with `git checkout -- <paths>`. Commits, merges, pushes and ref rewrites there are still refused. The misbinding refusal now says how to save uncommitted edits before the supervisor adopts the branch.
- **Adoption finds the predecessor's branch** — `transfer adopt_branch=true` no longer picks the receiver's own fresh task branch as the source when the task is already assigned to the receiver. It adopts the predecessor's parked, handed-off or recorded-start branch.
