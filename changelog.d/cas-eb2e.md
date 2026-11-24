### Fixed — Factory

- **Raw GitHub merges honor verifier holds** — A pull request cannot merge through the agent hook while its current head has a pending or rejected Cassy review. A supervisor may override a rejection with a recorded reason; pending reviews must finish.
