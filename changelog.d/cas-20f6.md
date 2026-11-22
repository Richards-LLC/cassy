### Fixed — Factory

- **Directed message batches arrive as one turn** — A batch of messages to the supervisor that is waiting for its last delivery claim now keeps all of its messages. Before this fix, a queue refresh could send one of those messages early as its own turn, and the supervisor was woken twice for one batch.
