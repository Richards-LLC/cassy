### Fixed — Factory

- **Merges to a queued main no longer fall back to a local merge** — Was: when the merge-queue check for the default branch failed, `worktree_merge` treated it as "no queue". It then tried a local merge, which either refused over unrelated edits in the main checkout or merged locally and was rejected on push. Now: a failed check refuses before anything changes, says the queue state is unknown and asks for a retry. A repository with no merge queue, or one not on GitHub, still merges locally.
