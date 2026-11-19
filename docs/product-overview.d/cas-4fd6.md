## Core Concepts

- A verifier launch refuses an owner whose recorded process is no longer running, even if its row is still marked active. The factory daemon names each launch refusal with a leading code, and the requesting side retries by that code: owner, scope, terminal-task, unsupported-daemon and inactive-handoff refusals are final, store contention and dropped jobs wait without spending an attempt, and anything else retries a bounded number of times. A refused owner is named by agent id, role and status, never by its full row.
