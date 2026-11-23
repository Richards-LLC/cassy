## Core Concepts

Orphan process cleanup resolves the project's worktree roots before matching each process's working directory, so a project reached through a symlink (for example macOS `/var` to `/private/var`) still reports and reaps its orphaned worktree processes.
