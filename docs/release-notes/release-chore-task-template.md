# Release chore task template

Use this brief when a factory worker prepares a release tag. The worker must
not be assigned the tag push or the release build: the factory pre-push guard
rejects `refs/tags/*`, and workers do not run Rust builds.

## Worker deliverable

1. Confirm the release PR landed and the intended version and commit match
   fresh `origin/main`.
2. In the assigned guarded worker worktree, if HEAD is the landed commit,
   run `./scripts/release.sh --publish-tag`. It exits with `HANDOFF REQUIRED`
   before any audit build or remote push and creates the annotated tag locally.
   If the worker branch is at another commit, do not switch to an unguarded
   worktree: verify the version at the landed commit, create the annotated
   local tag there with `git tag -a <tag> -m <tag> <landed-sha>`, and hand it
   off. The script refuses a mismatched HEAD before creating a tag.
3. Send the supervisor the tag name, full landed commit SHA, local annotated
   tag object SHA (`git rev-parse refs/tags/<tag>`), and the printed handoff
   command. State explicitly that the remote tag and publication are pending.

## Supervisor/operator completion

1. Verify that the named commit is still `origin/main` and that the local tag
   is annotated and peels to it. Use an unguarded release worktree; do not
   disable the worker guard in the worker worktree.
2. Run `./scripts/release.sh --publish-tag` there. It performs the audit and
   pushes the tag, then GitHub owns publication. If the worktree uses a
   separate clone, create the annotated tag there on the verified commit.
3. Record the remote tag, matching Release workflow, published asset and
   latency receipts before marking the release complete or announcing it.

The worker task closes after its handoff; publication remains a distinct
supervisor/operator duty.
