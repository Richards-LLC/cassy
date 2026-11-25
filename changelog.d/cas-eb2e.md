### Fixed — Factory

- **Raw GitHub merges honor verifier holds** — The agent hook holds merges with a pending or rejected review at the current PR head, and visible GraphQL merges whose targets cannot be verified. A supervisor may override a resolved rejection with a recorded reason; pending and unresolved holds cannot be overridden.
