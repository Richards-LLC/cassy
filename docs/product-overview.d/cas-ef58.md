## Core Concepts

- **Reexec behind a busy external lane** - the worker-task-queue reconcile read is replaceable work: while a slow job holds the external lane, resubmits coalesce into one waiting rerun and `cas factory reexec` abandons it instead of refusing.
