### Fixed — Factory

- **Slow close progress** — Task closes that outlive the MCP response budget return normal progress with a durable continuation ID. Read the task for the matching final outcome without retrying the close; checked parks remain awaiting merge.
