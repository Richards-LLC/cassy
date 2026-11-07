### Fixed — Factory spawn

- **Spawn survives a brief store stall** — Was: a spawn request failed when its task-queue binding missed the store wait budget during an in-place upgrade. Now: the binding retries once under a fresh bounded budget; a second miss still fails.
