### Fixed — Factory

- **Slow worker spawn requests return a receipt** — Was: the factory could time out without a key even though the spawn later queued. Now: read the returned receipt before deciding what to do. A queue admission is not confirmation that the worker started.
