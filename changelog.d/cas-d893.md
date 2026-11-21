### Fixed — Factory

- **Keep parked close publication alive during shutdown** — Worker shutdown and recycle wait for active close publication and its outcome writer. Failed owner publication stays in a durable daemon retry journal, with one actionable supervisor notice, without requiring another close.
