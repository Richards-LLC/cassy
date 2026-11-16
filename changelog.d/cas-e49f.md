### Fixed — Release trains

- **Promote the chosen release epic** — Freezing an open epic now returns a complete promotion command. One retry validates the lane, sets its train label and integration target, and freezes it so automatic release cutting can see it. Divergent open train lanes refuse with their epic ids before changing state.
