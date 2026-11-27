## Core Concepts

Managed-file sync defers on protected consumer branches without writing project files or changing the Git index or refs. Run `cas update --sync` from a repair worktree to apply the shared project's managed files on a topic branch, then review and publish them through a normal PR.
