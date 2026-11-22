## Core Concepts

### Retained worker publication counts

Factory GC reports, epic status and startup retained-worktree diagnostics distinguish `ahead_of_own_remote` from `on_no_remote`. The former counts commits ahead of the same worker branch on the selected push remote (origin when configured, otherwise the first remote); the latter counts commits absent from all remote-tracking refs. A missing own tracking ref reads `missing`, an unsuccessful probe reads `unknown`, and a repository without remotes reads `no_remote` with a separate `ahead_of_local_target` count. Reports use local tracking refs without fetching or modifying the checkout.

A checkout remains visible when its own remote is behind even if all its commits are already published on another remote branch. Clean checkouts with no local work remain absent; a missing own remote is never presented as zero commits ahead.
