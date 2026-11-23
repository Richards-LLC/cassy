## Core Concepts

### Task-bound QA metadata

Task QA predictions read the registry and product overview from the assigned worker checkout, including linked worktrees sharing a primary Cassy store. The checkout must share Git repository identity with the task's effective work target. Tasks without a worker checkout use the resolved target repository; a checkout from another repository produces a diagnostic instead of supplying unrelated metadata.

Task show provides current registry/impact context independently of historical creation notes, with structured QA impact on full reads. Task start recomputes prediction for its response and saved start advisories. Finding label validation and priority normalization use the same task-bound main journey declarations; an unrelated primary-checkout journey still fails validation. Metadata is read locally without changing the registry, checkout or historical notes.
