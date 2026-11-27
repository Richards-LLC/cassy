## Core Concepts

Managed-file sync defers on protected consumer branches without writing project files or changing the Git index or refs. Run `cas update --sync` from a repair worktree to apply managed files, including configured hooks, from the shared project on a topic branch. Review and publish the changes through a normal PR.
