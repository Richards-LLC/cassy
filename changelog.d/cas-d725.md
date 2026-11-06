### Fixed — Factory

- **Keep main on the release train** — While a release train is open, direct main/trunk worktree merges are refused even with `allow_trunk`; the refusal names the train and the existing `scripts/release.sh --hotfix` command for a one-item hotfix train.
