## Core Concepts

- **Recorded delivery identity** - Cassy treats commits it recorded for a task, and commits a supervisor granted with `merge_source`, as the task's own at start, close and rebind, provided they sit under an anchor the task still holds. Replacement, rebound and retargeted deliveries are no longer refused just because their own cycle commits name another task in the subject line. A branch that only has a retired anchor or a bare target-sync merge still counts as undelivered.
