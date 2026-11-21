### Fixed — Factory

- **Worker recycles survive a briefly busy store** — Was: when another part of the factory held the shared database for a few milliseconds at the moment a worker recycle (`clear_context`) started, the recycle failed with "retry on a later pass", nothing retried it, and the supervisor had to clear the worker's context again by hand. Now: the request stays queued, the worker keeps running, and the next factory pass completes the recycle.
