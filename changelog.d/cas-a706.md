### Fixed — Factory

- **Replacement workers no longer inherit a block** — Was: shutting down a worker that had blocked its task, then spawning a replacement onto it, relayed the old block as the new worker's. Now: releasing a worker's task clears its blocked hold and answer.
