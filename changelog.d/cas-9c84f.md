### Fixed — Release close

- **Keep close evidence at the shipped commit** — Release finalization validates the captured delivery cycle and its complete effect against the immutable release, including reviewed view transformations and promoted changelog fragments. Later branch edits cannot change that release decision; missing source and independent holds still refuse close.
