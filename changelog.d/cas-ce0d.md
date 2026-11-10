### Fixed — Factory

- **Detached worker handoffs prepare the successor branch** — Starting a fresh task from a clean detached target now checks out its named task branch and preserves held delivery branches. Detached work, tracked edits and detached task resumes refuse before accepting the start, with branch setup guidance.
