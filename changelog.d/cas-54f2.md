### Fixed — Factory

- **Active capped runs avoid false stall nudges** — The director recognizes live registered commands for the worker's current task after their launch tool returns. Finished commands restore ordinary stall detection, and completion still reaches the worker once.
