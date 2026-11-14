## Core Concepts

- **Returned deliveries stay in branch history** - when a parked delivery is returned for changes or a merge conflict, its tip stays reachable on the branch. The returned task's start lists that tip under close checks, and the worker push guard (H-40) refuses a rebase that would drop it; merging the target in is the supported recovery.
