## Core Concepts

### Authorized takeover of another worker's branch

A worker whose registered worktree is checked out on another worker's branch may restore files there with `git checkout -- <paths>`. That cleanup is what an authorized takeover needs before adoption. Commits, merges, pushes and ref rewrites on that branch are still refused. `transfer adopt_branch=true` takes its source from the predecessor's parked, handed-off or recorded-start branch, never from the receiver's own lane, including when the task is already assigned to the receiver. Adoption still requires a clean worktree. The refusal tells the worker to save edits with `git stash push -u` first and restore them on the adopted branch.
