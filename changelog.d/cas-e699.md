### Fixed — Factory worker checks

- **Widened JavaScript receipts count the runner's passing tests** — A widened Jest or Vitest proof counted distinct test names, so tests sharing a name in one file were counted once and the receipt disagreed with the runner (2955 against Jest's 2957 passed). The receipt now records the passing results the runner reported and labels them as passed tests. Skipped tests stay out of the count (GH #2246, #2166).
